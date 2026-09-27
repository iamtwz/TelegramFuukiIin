import { test } from 'node:test';
import assert from 'node:assert/strict';

function browserFixture(t) {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const original = new Map(['window', 'document', 'ResizeObserver'].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  t.after(() => {
    for (const [key, descriptor] of original) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  const scripts = [];
  const observers = [];
  const bounds = { width: 360, height: 0 };
  const container = { getBoundingClientRect: () => bounds };
  globalThis.window = {};
  globalThis.document = {
    createElement: () => ({ remove() { this.removed = true; } }),
    head: { append: script => scripts.push(script) },
    getElementById: id => { assert.equal(id, 'challenge'); return container; }
  };
  globalThis.ResizeObserver = class {
    constructor(callback) { this.callback = callback; observers.push(this); }
    observe(element) { assert.equal(element, container); }
    disconnect() { this.disconnected = true; }
  };
  const view = { state: '', spinner: false, retry: { hidden: true }, say(key) { this.state = key; }, loading(value) { this.spinner = value; } };
  return { scripts, observers, bounds, view };
}

test('SDK timeout is retryable, ignores late callbacks and waits for the SDK onload', async t => {
  const { scripts } = browserFixture(t);
  const { loadTurnstile } = await import('../public/view.js?test=loader');
  const first = loadTurnstile();
  assert.equal(loadTurnstile(), first);
  const firstName = new URL(scripts[0].src).searchParams.get('onload');
  const lateLoad = window[firstName];
  const failure = assert.rejects(first, /captcha_unavailable/);
  t.mock.timers.tick(12000);
  await failure;
  assert.equal(scripts[0].removed, true);
  lateLoad();
  const retry = loadTurnstile();
  assert.equal(scripts.length, 2);
  let resolved = false;
  retry.then(() => { resolved = true; });
  await Promise.resolve();
  assert.equal(resolved, false);
  window.turnstile = { render() {} };
  window[new URL(scripts[1].src).searchParams.get('onload')]();
  await retry;
  assert.equal(resolved, true);
});

test('a blank or zero-width widget times out with retry and disposed callbacks cannot submit', async t => {
  const { observers, bounds, view } = browserFixture(t);
  const { renderChallenge } = await import('../public/view.js');
  let callbacks;
  let removed;
  const tokens = [];
  window.turnstile = { render: (_, options) => { callbacks = options; return 'widget'; }, remove: id => { removed = id; } };
  const dispose = renderChallenge(view, { callback: token => tokens.push(token) });
  assert.equal(view.state, 'loading');
  assert.equal(view.spinner, true);
  bounds.width = 0; bounds.height = 65;
  observers[0].callback();
  assert.equal(view.state, 'loading');
  t.mock.timers.tick(15000);
  assert.equal(view.state, 'captcha_unavailable');
  assert.equal(view.retry.hidden, false);
  assert.equal(view.spinner, false);
  dispose();
  assert.equal(removed, 'widget');
  view.state = 'new_attempt';
  callbacks.callback('late-token');
  callbacks['error-callback']();
  assert.deepEqual(tokens, []);
  assert.equal(view.state, 'new_attempt');
});

test('visible challenges are not timed out while waiting for a person', async t => {
  const { observers, bounds, view } = browserFixture(t);
  const { renderChallenge } = await import('../public/view.js');
  let callbacks;
  const tokens = [];
  window.turnstile = { render: (_, options) => { callbacks = options; return 'visible'; }, remove() {} };
  const dispose = renderChallenge(view, { callback: token => tokens.push(token) });
  bounds.height = 65;
  observers[0].callback();
  assert.equal(view.state, 'ready');
  assert.equal(view.spinner, false);
  t.mock.timers.tick(120000);
  assert.equal(view.state, 'ready');
  assert.equal(view.retry.hidden, true);
  callbacks.callback('synthetic-token');
  assert.deepEqual(tokens, ['synthetic-token']);
  dispose();
});
