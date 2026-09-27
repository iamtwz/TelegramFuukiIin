import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { parseAllDocuments } from 'yaml';

const root = new URL('../', import.meta.url);
const registry = 'https://registry.npmjs.org/';
const workspaces = ['apps/verification', 'packages/verification-protocol'];
const version = /^\d+\.\d+\.\d+$/;
const packageId = /^(?:@[a-z0-9._-]+\/)?[a-z0-9._-]+@\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const read = path => readFileSync(new URL(path, root), 'utf8');
export function parseYaml(text) {
  return parseAllDocuments(text).map(doc => {
    assert.equal(doc.errors.length, 0, 'Invalid or duplicate YAML keys');
    assert.equal(doc.warnings.length, 0, 'Unexpected YAML tags');
    return doc.toJS({ maxAliasCount: 0 });
  });
}
export function inputs() {
  return {
    manifests: Object.fromEntries(['.', ...workspaces].map(path => [path, JSON.parse(read(`${path}/package.json`))])),
    workspace: parseYaml(read('pnpm-workspace.yaml'))[0],
    locks: parseYaml(read('pnpm-lock.yaml')),
    npmrc: read('.npmrc'),
  };
}
function noRuntimeDependencies(manifest) {
  for (const field of ['dependencies', 'optionalDependencies', 'peerDependencies']) {
    assert.equal(Object.keys(manifest[field] || {}).length, 0, 'Runtime dependencies require explicit review');
  }
}
function registryPackages(lock) {
  assert.equal(lock.lockfileVersion, '9.0');
  assert.ok(Object.keys(lock.packages || {}).length > 0, 'Missing locked packages');
  for (const [id, entry] of Object.entries(lock.packages)) {
    assert.match(id, packageId, `Unexpected package source ${id}`);
    const resolution = entry.resolution;
    assert.ok(resolution && Object.keys(resolution).every(k => ['integrity', 'tarball'].includes(k)), `Unexpected resolution for ${id}`);
    assert.match(resolution.integrity || '', /^sha512-[A-Za-z0-9+/]{86}==$/, `Missing SHA-512 integrity for ${id}`);
    if (resolution.tarball) {
      const url = new URL(resolution.tarball);
      assert.equal(url.origin, registry.slice(0, -1), `Unexpected registry for ${id}`);
      assert.ok(!url.username && !url.password && !url.search && !url.hash, `Unexpected registry URL for ${id}`);
    }
  }
  for (const [id, snapshot] of Object.entries(lock.snapshots || {})) {
    assert.ok(lock.packages[id.split('(')[0]], `Unresolved snapshot ${id}`);
    for (const field of ['dependencies', 'optionalDependencies']) {
      for (const [name, ref] of Object.entries(snapshot[field] || {})) {
        assert.equal(typeof ref, 'string');
        assert.ok(lock.packages[`${name}@${ref.split('(')[0]}`], `Unreviewed dependency source ${name}@${ref}`);
      }
    }
  }
  return Object.keys(lock.packages).length;
}
export function validate({ manifests, workspace, locks, npmrc }) {
  const pkg = manifests['.'];
  assert.match(pkg.packageManager || '', /^pnpm@\d+\.\d+\.\d+$/, 'Pin pnpm exactly');
  assert.deepEqual(workspace.packages, workspaces, 'Unexpected workspace paths');
  assert.equal(workspace.ignoreScripts, true, 'Install scripts must stay disabled');
  assert.equal(workspace.ignorePnpmfile, true, 'pnpmfile hooks must stay disabled');
  assert.equal(workspace.saveExact, true);
  assert.equal(workspace.blockExoticSubdeps, true);
  assert.equal(workspace.verifyDepsBeforeRun, 'error');
  assert.equal(workspace.lockfileIncludeTarballUrl, true);
  assert.ok(workspace.minimumReleaseAge >= 1440, 'Keep the one-day release delay');
  assert.equal(workspace.registry ?? registry, registry);
  assert.deepEqual(workspace.registries || {}, {}, 'Scoped registries require review');
  const registryLines = npmrc.split('\n').filter(line => /registry\s*=/.test(line));
  assert.deepEqual(registryLines, [`registry=${registry}`], 'Use only the official npm registry');

  // pnpm 12 records its pinned package manager in a separate YAML document.
  assert.equal(locks.length, 2, 'Expected pnpm tool and workspace lock documents');
  const [tool, lock] = locks;
  assert.deepEqual(Object.keys(tool.importers), ['.']);
  assert.deepEqual(tool.importers['.'].configDependencies, {}, 'Config dependencies require review');
  const pin = pkg.packageManager.slice('pnpm@'.length);
  assert.deepEqual(tool.importers['.'].packageManagerDependencies, { pnpm: { specifier: pin, version: pin } });
  assert.deepEqual(Object.keys(lock.importers).sort(), ['.', ...workspaces].sort());
  for (const [path, manifest] of Object.entries(manifests)) {
    noRuntimeDependencies(manifest);
    const importer = lock.importers[path];
    noRuntimeDependencies(importer);
    assert.deepEqual(Object.keys(importer.devDependencies || {}).sort(), Object.keys(manifest.devDependencies || {}).sort());
    for (const [name, pin] of Object.entries(manifest.devDependencies || {})) {
      assert.match(pin, version, `Pin ${name} exactly`);
      assert.equal(importer.devDependencies[name].specifier, pin, `Outdated lock entry ${name}`);
      assert.equal(importer.devDependencies[name].version.split('(')[0], pin);
      assert.ok(lock.packages[`${name}@${pin}`], `Missing locked dependency ${name}`);
    }
  }
  return { packages: registryPackages(lock), tools: registryPackages(tool) };
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const count = validate(inputs());
  console.log(`Lockfile: ${count.packages} dependency packages + ${count.tools} pnpm packages, official registry, SHA-512 integrity; no runtime dependencies.`);
}
