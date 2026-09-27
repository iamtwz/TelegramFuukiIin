import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
const assets = new URL('../apps/verification/public/', import.meta.url);
const previewScript = `
import { createView, loadTurnstile, renderChallenge } from '/view.js';
const view = createView();
let destroy = () => {};
let busy = false;
function render() {
  if (busy || !window.turnstile) return;
  destroy();
  destroy = renderChallenge(view, {
    sitekey: '1x00000000000000000000AA', action: 'join',
    theme: view.theme, size: 'flexible', language: view.widgetLanguage,
    callback: () => view.say('preview_passed')
  });
}
async function start() {
  if (busy) return;
  destroy(); destroy = () => {};
  busy = true; view.busy(true); view.retry.hidden = true; view.loading(true); view.say('loading');
  try { await loadTurnstile(); }
  catch {
    view.loading(false); view.say('captcha_unavailable'); view.retry.hidden = false;
    return;
  } finally { busy = false; view.busy(false); }
  render();
}
view.onChange(render);
view.retry.addEventListener('click', start);
start();
`;
createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1:8787').pathname;
  response.setHeader('Cache-Control', 'no-store');
  response.setHeader('Referrer-Policy', 'no-referrer');
  response.setHeader('X-Content-Type-Options', 'nosniff');
  if (request.method !== 'GET' || !['/', '/verify', '/style.css', '/preview.js', '/view.js', '/i18n.js'].includes(path)) {
    response.writeHead(404); response.end(); return;
  }
  try {
    if (path === '/preview.js') {
      response.setHeader('Content-Type', 'text/javascript; charset=utf-8');
      response.end(previewScript); return;
    }
    if (['/style.css', '/view.js', '/i18n.js'].includes(path)) {
      response.setHeader('Content-Type', path.endsWith('.css') ? 'text/css; charset=utf-8' : 'text/javascript; charset=utf-8');
      response.end(await readFile(new URL(path.slice(1), assets))); return;
    }
    let html = await readFile(new URL('index.html', assets), 'utf8');
    html = html.replace('<script src="https://telegram.org/js/telegram-web-app.js" defer></script>', '')
      .replace('<script type="module" src="/app.js"></script>', '<script type="module" src="/preview.js"></script>');
    response.setHeader('Content-Type', 'text/html; charset=utf-8');
    response.end(html);
  } catch {
    response.writeHead(500); response.end('Preview unavailable');
  }
}).listen(8787, '127.0.0.1', () => console.log('Turnstile UI preview: http://127.0.0.1:8787/'));
