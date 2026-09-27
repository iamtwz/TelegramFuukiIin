// Exercise the actual Pages static server, including routing and security headers.
// Uses only synthetic configuration and never loads the developer's .env files.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, writeFile, rm, readdir } from 'node:fs/promises';
import { createServer } from 'node:net';
import { once } from 'node:events';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const temp = await mkdtemp(join(tmpdir(), 'fuuki-iin-pages-'));
const probe = createServer(); probe.listen(0, '127.0.0.1'); await once(probe, 'listening');
const port = probe.address().port; await new Promise(resolve => probe.close(resolve));
const config = join(temp, 'wrangler.jsonc');
await writeFile(config, JSON.stringify({ name: 'fuuki-iin-pages-test', compatibility_date: '2026-09-24', pages_build_output_dir: join(root, 'dist/verification') }));
const child = spawn(process.execPath, [join(root, 'node_modules/wrangler/bin/wrangler.js'), 'pages', 'dev', '--ip', '127.0.0.1', '--port', String(port), '--inspector-port', '0', '--show-interactive-dev-session=false'], {
  cwd: temp, env: { ...process.env, WRANGLER_SEND_METRICS: 'false', CLOUDFLARE_LOAD_DEV_VARS_FROM_DOT_ENV: 'false', WRANGLER_LOG_PATH: join(temp, 'wrangler.log'), BROWSER: 'none' }, stdio: ['ignore', 'pipe', 'pipe']
});
let logs = ''; child.stdout.on('data', b => { logs += b; }); child.stderr.on('data', b => { logs += b; });
const exited = once(child, 'exit');
const base = `http://127.0.0.1:${port}`;
try {
  let ready = false;
  for (let n = 0; n < 120; n++) {
    if (child.exitCode !== null) throw new Error(logs);
    try { if ((await fetch(base, { signal: AbortSignal.timeout(300) })).ok) { ready = true; break; } } catch {}
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  assert.ok(ready, logs);
  const page = await fetch(`${base}/verify?session=${'a'.repeat(32)}&chat=-123&sitekey=synthetic-sitekey`);
  assert.equal(page.status, 200);
  assert.equal(new URL(page.url).searchParams.get('session'), 'a'.repeat(32));
  assert.equal(new URL(page.url).searchParams.get('chat'), '-123');
  assert.equal(new URL(page.url).searchParams.get('sitekey'), 'synthetic-sitekey');
  assert.match(await page.text(), /<title>TelegramFuukiIin Verify<\/title>/);
  assert.equal(page.headers.get('referrer-policy'), 'no-referrer');
  assert.equal(page.headers.get('x-content-type-options'), 'nosniff');
  assert.equal(page.headers.get('cache-control'), 'no-store');
  assert.match(page.headers.get('content-security-policy'), /frame-src https:\/\/challenges.cloudflare.com/);
  assert.match(await (await fetch(`${base}/protocol.js`)).text(), /encodeSubmission/);
  const app = await (await fetch(`${base}/app.js`)).text();
  assert.match(app, /telegram.sendData/); assert.ok(!app.includes('/api/verify'));
  const removedApi = await fetch(`${base}/api/verify`, {method: 'POST', body: '{}'});
  assert.ok([404,405].includes(removedApi.status));
  assert.deepEqual((await readdir(join(root, 'dist/verification'))).sort(), ['_headers','_redirects','app.js','i18n.js','index.html','protocol.js','style.css','view.js'].sort());
  console.log('PASS: real Pages static server, /verify route, CSP/security headers, browser modules; no Functions/Worker/API or secrets.');
} finally {
  if (child.exitCode === null) child.kill('SIGTERM');
  await exited;
  await rm(temp, { recursive: true, force: true });
}
