import { test } from 'node:test';
import assert from 'node:assert/strict';
import { inputs, parseYaml, validate } from './check-lock.mjs';

test('pnpm lock policy checks both workspace dependencies and package-manager binaries', () => {
  const data = inputs();
  assert.ok(validate(data).tools > 0);
  for (const index of [0, 1]) {
    const altered = structuredClone(data);
    const entry = Object.values(altered.locks[index].packages)[0];
    entry.resolution.tarball = 'https://registry.npmjs.org.attacker.invalid/pkg.tgz';
    assert.throws(() => validate(altered), /Unexpected registry/);
    delete entry.resolution.tarball;
    entry.resolution.integrity = 'sha1-untrusted';
    assert.throws(() => validate(altered), /SHA-512/);
  }
});

test('pnpm lock policy rejects Git, file and unreviewed workspace dependencies', () => {
  for (const ref of ['file:../outside', 'link:../outside', 'git+https://example.invalid/repo']) {
    const data = inputs();
    Object.values(data.locks[1].snapshots)[0].dependencies = { unexpected: ref };
    assert.throws(() => validate(data), /Unreviewed dependency source/);
  }
  const data = inputs();
  data.manifests['apps/verification'].dependencies = { unreviewed: '1.0.0' };
  assert.throws(() => validate(data), /Runtime dependencies/);
});

test('pnpm policy rejects unsafe configuration and mismatched direct versions', () => {
  const data = inputs();
  data.workspace.ignoreScripts = false;
  assert.throws(() => validate(data), /Install scripts/);
  data.workspace.ignoreScripts = true;
  data.npmrc += '\n@scope:registry=https://example.invalid/';
  assert.throws(() => validate(data), /official npm registry/);
  const altered = inputs();
  altered.locks[1].importers['.'].devDependencies.wrangler.specifier = '^4.134.0';
  assert.throws(() => validate(altered), /Outdated lock entry/);
});

test('YAML input rejects duplicate keys and aliases', () => {
  assert.throws(() => parseYaml('key: 1\nkey: 2'), /duplicate YAML keys/);
  assert.throws(() => parseYaml('first: &anchor [1]\nsecond: *anchor'));
});
