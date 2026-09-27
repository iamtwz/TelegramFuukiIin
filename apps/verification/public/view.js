import { locales, resolveLocale, translate } from './i18n.js';

export function createView(telegram) {
  const selector = document.getElementById('language');
  const menu = document.getElementById('language-options');
  const options = [...menu.querySelectorAll('[role="option"]')];
  const retry = document.getElementById('retry');
  const close = document.getElementById('close');
  const media = matchMedia('(prefers-color-scheme: dark)');
  let saved;
  try { saved = localStorage.getItem('fuuki-iin.language'); } catch { /* Storage is optional in a WebView. */ }
  // Telegram's unverified language hint is presentation-only, never authorization.
  let locale = resolveLocale(new URLSearchParams(location.search).get('lang'), saved,
    telegram?.initDataUnsafe?.user?.language_code, navigator.languages, navigator.language);
  let state = 'loading';
  const listeners = new Set();
  const theme = () => telegram?.platform && telegram.platform !== 'unknown'
    ? telegram.colorScheme || 'light' : media.matches ? 'dark' : 'light';
  function paint() {
    document.documentElement.lang = locale;
    document.documentElement.dataset.theme = theme();
    document.title = translate(locale, 'brand');
    document.getElementById('title').textContent = translate(locale, 'title');
    document.getElementById('status').textContent = translate(locale, state);
    document.getElementById('language-label').textContent = locales[locale].label;
    selector.setAttribute('aria-label', translate(locale, 'language'));
    menu.setAttribute('aria-label', translate(locale, 'language'));
    for (const option of options) option.setAttribute('aria-selected', String(option.dataset.locale === locale));
    retry.textContent = translate(locale, 'retry');
    close.textContent = translate(locale, 'close');
  }
  function changed() { paint(); for (const listener of listeners) listener(); }
  function hideMenu(restoreFocus = false) {
    menu.hidden = true;
    selector.setAttribute('aria-expanded', 'false');
    if (restoreFocus) selector.focus();
  }
  function showMenu(index = options.findIndex(option => option.dataset.locale === locale)) {
    if (selector.disabled) return;
    menu.hidden = false;
    selector.setAttribute('aria-expanded', 'true');
    options[index].focus();
  }
  function choose(option) {
    if (!option || selector.disabled) return;
    locale = resolveLocale(option.dataset.locale);
    try { localStorage.setItem('fuuki-iin.language', locale); } catch { /* Keep this session's choice. */ }
    hideMenu(true);
    changed();
  }
  selector.addEventListener('click', () => menu.hidden ? showMenu() : hideMenu());
  selector.addEventListener('keydown', event => {
    if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault();
      showMenu(event.key === 'Home' ? 0 : event.key === 'End' ? options.length - 1 : undefined);
    }
  });
  menu.addEventListener('click', event => choose(event.target.closest('[role="option"]')));
  menu.addEventListener('keydown', event => {
    const index = options.indexOf(document.activeElement);
    if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault();
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? options.length - 1
        : (index + (event.key === 'ArrowDown' ? 1 : -1) + options.length) % options.length;
      options[next].focus();
    } else if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault(); choose(options[index]);
    } else if (event.key === 'Escape') {
      event.preventDefault(); hideMenu(true);
    } else if (event.key === 'Tab') hideMenu(true);
  });
  const control = selector.parentElement;
  document.addEventListener('pointerdown', event => {
    if (!control.contains(event.target)) hideMenu();
  });
  control.addEventListener('focusout', event => {
    if (!control.contains(event.relatedTarget)) hideMenu();
  });
  telegram?.onEvent?.('themeChanged', changed);
  media.addEventListener('change', changed);
  paint();
  return {
    retry, close,
    get locale() { return locale; },
    get theme() { return theme(); },
    get widgetLanguage() { return locales[locale].turnstile; },
    text: key => translate(locale, key),
    say(key) { state = key; paint(); },
    loading(value) {
      document.getElementById('challenge-loading').hidden = !value;
      document.getElementById('challenge').setAttribute('aria-busy', String(value));
    },
    busy(value) { if (value) hideMenu(); selector.disabled = value; retry.disabled = value; },
    onChange(listener) { listeners.add(listener); }
  };
}

let loading;
let loadAttempt = 0;
export function loadTurnstile() {
  if (loading) return loading;
  const attempt = new Promise((resolve, reject) => {
    let script;
    let settled = false;
    const callbackName = `fuukiIinTurnstileLoaded${++loadAttempt}`;
    const finish = error => {
      if (settled) return;
      settled = true; clearTimeout(timer);
      delete window[callbackName];
      if (script) { script.onload = null; script.onerror = null; }
      if (error) { script?.remove(); reject(new Error('captcha_unavailable')); }
      else resolve();
    };
    const timer = setTimeout(() => finish(true), 12000);
    if (typeof window.turnstile?.render === 'function') finish(false);
    else {
      // The SDK's onload callback runs after initialization, unlike script.onload.
      window[callbackName] = () => finish(typeof window.turnstile?.render !== 'function');
      script = document.createElement('script');
      script.async = true;
      script.src = `https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit&onload=${callbackName}`;
      script.onerror = () => finish(true);
      document.head.append(script);
    }
  }).catch(error => { loading = undefined; throw error; });
  loading = attempt;
  return loading;
}

// Preview and production share the same loading, visibility and failure handling.
export function renderChallenge(view, options) {
  const container = document.getElementById('challenge');
  let widget;
  let disposed = false;
  let waiting = true;
  let timer;
  const stopWaiting = () => {
    waiting = false; clearTimeout(timer); observer.disconnect(); view.loading(false);
  };
  const fail = key => {
    if (disposed) return;
    stopWaiting(); view.say(key); view.retry.hidden = false;
  };
  const observer = new ResizeObserver(() => {
    const rect = container.getBoundingClientRect();
    if (disposed || !waiting || rect.width < 300 || rect.height < 60) return;
    stopWaiting(); view.say('ready');
  });
  view.retry.hidden = true; view.loading(true); view.say('loading');
  observer.observe(container);
  timer = setTimeout(() => fail('captcha_unavailable'), 15000);
  try {
    widget = window.turnstile.render(container, {
      ...options, appearance: 'always', retry: 'never', 'refresh-expired': 'manual', 'refresh-timeout': 'manual',
      callback: token => {
        if (disposed) return;
        stopWaiting(); view.retry.hidden = true; options.callback(token);
      },
      'before-interactive-callback': () => {
        if (disposed) return;
        stopWaiting(); view.say('ready');
      },
      'error-callback': () => fail('captcha_unavailable'),
      'unsupported-callback': () => fail('captcha_unavailable'),
      'expired-callback': () => fail('expired'),
      'timeout-callback': () => fail('expired')
    });
  } catch { fail('captcha_unavailable'); }
  return () => {
    disposed = true; stopWaiting();
    if (widget !== undefined) window.turnstile.remove(widget);
  };
}
