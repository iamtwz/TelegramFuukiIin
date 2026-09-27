import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { readLaunch, encodeSubmission } from '../src/index.ts';
const fixture = JSON.parse(await readFile(new URL('../fixtures/v2.json', import.meta.url)));
test('Pages and Rust share the v2 submission fixture within Telegram size limits', () => {
  assert.deepEqual(readLaunch(new URLSearchParams(fixture.launch).toString()), fixture.launch);
  assert.deepEqual(JSON.parse(encodeSubmission(fixture.launch, fixture.submission.token)), fixture.submission);
  assert.ok(new TextEncoder().encode(encodeSubmission(fixture.launch, 'x'.repeat(2048))).length < 4096);
});
test('reject malformed routing hints, duplicate parameters, oversized or invalid tokens', () => {
  for (const patch of [{session:'a'}, {chat:'-01'}, {chat:'0'}, {chat:'-9007199254740992'}, {sitekey:'<script>'}]) {
    assert.throws(() => readLaunch(new URLSearchParams({...fixture.launch,...patch}).toString()));
  }
  assert.throws(() => readLaunch(new URLSearchParams(fixture.launch).toString() + '&session=' + fixture.launch.session));
  for (const token of ['', 'a'.repeat(2049), 'with space', '含中文', '\n', null]) assert.throws(() => encodeSubmission(fixture.launch, token));
});
