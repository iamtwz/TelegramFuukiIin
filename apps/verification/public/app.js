import { createView, loadTurnstile, renderChallenge } from './view.js';
import { readLaunch, encodeSubmission } from './protocol.js';

const telegram = window.Telegram?.WebApp;
const view = createView(telegram);
let launch;
let destroyWidget = () => {};
let generation = 0;
let busy = false;
let submitted = false;

function removeWidget() {
  generation++;
  destroyWidget(); destroyWidget = () => {};
}
function submit(token, version) {
  if (submitted || busy || version !== generation) return;
  busy = true; view.busy(true); view.say('submitting');
  try {
    // Telegram delivers the real sender to Rust. URL/JS user claims are never trusted.
    telegram.sendData(encodeSubmission(launch, token));
    submitted = true;
    removeWidget();
    view.say('sent');
    view.retry.hidden = true;
  } catch {
    removeWidget();
    view.say('return_failed'); view.retry.hidden = false;
  } finally { busy = false; view.busy(false); }
}
function renderWidget() {
  if (!launch || busy || submitted || !window.turnstile) return;
  removeWidget();
  const version = generation;
  destroyWidget = renderChallenge(view, {
    sitekey: launch.sitekey, action: 'join', cData: launch.session,
    theme: view.theme, language: view.widgetLanguage, size: 'flexible',
    callback: token => submit(token, version)
  });
}
async function start() {
  if (busy || submitted) return;
  view.retry.hidden = true;
  if (!telegram || telegram.platform === 'unknown') { view.say('open_in_telegram'); return; }
  try { launch = readLaunch(location.search); }
  catch { view.say('invalid_link'); return; }
  telegram.ready(); telegram.expand();
  removeWidget();
  busy = true; view.busy(true); view.loading(true); view.say('loading');
  try { await loadTurnstile(); }
  catch {
    view.loading(false); view.say('captcha_unavailable'); view.retry.hidden = false;
    return;
  } finally { busy = false; view.busy(false); }
  renderWidget();
}
view.onChange(renderWidget);
view.retry.addEventListener('click', start);
start();
