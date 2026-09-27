// Synthetic credentials, isolated Compose project/volume, no container network.
// Never reads the developer's .env or starts a live bot.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const temp = mkdtempSync(join(tmpdir(), 'fuuki-iin-docker-'));
const project = `fuuki-iin-smoke-${randomUUID().slice(0, 8)}`;
const envFile = join(temp, '.env');
const composeFile = join(temp, 'compose.yaml');
const override = join(temp, 'compose.test.yaml');
writeFileSync(envFile, [
  'TELEGRAM_BOT_TOKEN=123456:synthetic-token',
  'BOT_USERNAME=fuuki_iin_test_bot',
  'MANAGED_CHAT_IDS=-1001234567890',
  "OPENROUTER_API_KEY='synthetic$dollar#value'",
  'VERIFICATION_BASE_URL=https://verification.example.invalid',
  'TURNSTILE_SITE_KEY=synthetic-sitekey',
  'TURNSTILE_SECRET_KEY=synthetic-turnstile-secret',
  'DATABASE_PATH=/outside-volume/should-not-be-used.sqlite',
  '',
].join('\n'), { mode: 0o600 });
// Keep standard .env discovery inside the isolated temporary project.
writeFileSync(composeFile, readFileSync(join(root, 'compose.yaml')));
writeFileSync(override, `services:\n  bot:\n    image: ${project}-bot\n    pull_policy: never\n    build:\n      context: ${JSON.stringify(root)}\n    network_mode: none\n`);
const base = [
  'compose', '--project-name', project,
  '-f', composeFile, '-f', override,
];
const testEnv = { ...process.env };
for (const key of Object.keys(testEnv)) {
  if (key.startsWith('COMPOSE_')) delete testEnv[key];
}
function compose(args, { status = 0, stream = false, stderr = false } = {}) {
  const result = spawnSync('docker', [...base, ...args], {
    cwd: temp,
    env: testEnv,
    encoding: 'utf8',
    stdio: stream ? 'inherit' : 'pipe',
    timeout: stream ? 20 * 60_000 : 60_000,
  });
  if (result.error) throw result.error;
  assert.equal(result.status, status, `${args.join(' ')}\n${result.stdout ?? ''}${result.stderr ?? ''}`);
  return (stderr ? result.stderr : result.stdout) ?? '';
}
const run = ['run', '--rm', '--no-deps', '-T'];
const shell = (script) => compose([...run, '--entrypoint', 'sh', 'bot', '-ec', script]);

try {
  compose(['config', '--quiet']);
  compose(['build', 'bot'], { stream: true });
  assert.match(compose([...run, 'bot', 'check']), /Configuration valid: 1 managed groups/);
  assert.match(compose([...run, '-e', 'MANAGED_CHAT_IDS=', 'bot', 'check']), /Configuration valid: 0 managed groups/);
  assert.match(compose([...run, '-e', 'SUPER_ADMIN_IDS=42,43', 'bot', 'check']), /Super admins: 2/);
  compose([...run, '-e', 'SUPER_ADMIN_IDS=42,invalid', 'bot', 'check'], { status: 1 });
  assert.match(compose([...run, '-e', 'LOG_LEVEL=debug', 'bot', 'check']), /Log level: debug/);
  assert.match(compose([...run, '-e', 'LOG_LEVEL=debug', 'bot', 'check', '-v']), /Log level: verbose/);
  compose([...run, '-e', 'LOG_LEVEL=typo', 'bot', 'check'], { status: 1 });
  compose([...run, '-e', 'TELEGRAM_BOT_TOKEN=', 'bot', 'check'], { status: 1 });
  shell(`
    test "$(id -u)" = 10001
    test "$OPENROUTER_API_KEY" = 'synthetic$dollar#value'
    test "$DATABASE_PATH" = /var/lib/fuuki-iin/fuuki-iin.sqlite
    test -s /etc/ssl/certs/ca-certificates.crt
    test ! -e /app/.env
    test ! -e /build
    ! command -v cargo
    ! touch /app/should-fail 2>/dev/null
  `);

  // Startup creates SQLite before its first API request. The disabled network
  // makes that request fail without contacting Telegram.
  const logs = compose([...run, 'bot', 'run', '--debug'], { status: 1, stderr: true });
  const records = logs.split('\n').filter(line => line.startsWith('{')).map(line => JSON.parse(line));
  assert.ok(records.some(record => record.event === 'bot.starting' && record.fields.log_level === 'debug'));
  const request = records.find(record => record.event === 'api.request');
  const error = records.find(record => record.event === 'api.error');
  assert.equal(request.fields.method, 'getWebhookInfo');
  assert.equal(request.fields.api, 'telegram');
  assert.deepEqual(request.body, {});
  assert.equal(error.fields.request_id, request.fields.request_id);
  assert.equal(typeof error.fields.elapsed_ms, 'number');
  for (const secret of ['123456:synthetic-token', 'synthetic$dollar#value', 'synthetic-turnstile-secret', 'api.telegram.org/bot']) {
    assert.ok(!logs.includes(secret), 'startup error logs must not expose secrets or Bot API URLs');
  }
  const jevLogs = compose([...run, 'bot', 'jev-smoke', '-vv'], { status: 1, stderr: true });
  const jevRecords = jevLogs.split('\n').filter(line => line.startsWith('{')).map(line => JSON.parse(line));
  const jevRequest = jevRecords.find(record => record.event === 'api.request');
  assert.equal(jevRequest.fields.api, 'openrouter');
  assert.equal(jevRequest.fields.method, 'decisions');
  assert.ok(JSON.stringify(jevRequest.body).includes('lin_dev'));
  assert.ok(jevRecords.some(record => record.event === 'api.error'));
  assert.ok(!jevLogs.includes('synthetic$dollar#value'));
  const digest = shell(`
    test -s /var/lib/fuuki-iin/fuuki-iin.sqlite
    test "$(stat -c %u:%a /var/lib/fuuki-iin)" = 10001:700
    test "$(stat -c %u:%a /var/lib/fuuki-iin/fuuki-iin.sqlite)" = 10001:600
    test -f /var/lib/fuuki-iin/fuuki-iin.instance.lock
    sha256sum /var/lib/fuuki-iin/fuuki-iin.sqlite
  `).trim();
  compose(['down']);
  assert.equal(shell('sha256sum /var/lib/fuuki-iin/fuuki-iin.sqlite').trim(), digest);
  console.log('Compose smoke passed: configuration, log modes and redacted Telegram/Jev errors, literal secrets, non-root runtime, SQLite permissions and persistent data.');
} finally {
  try {
    // Cleanup is limited to the randomly named test project and its test data.
    compose(['down', '--volumes', '--rmi', 'all'], { stream: true });
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}
