// Deliberately restricted to Cargo's generated lock format; no TOML dependency.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const lock = readFileSync(new URL('../Cargo.lock', import.meta.url), 'utf8');
const manifest = readFileSync(new URL('../Cargo.toml', import.meta.url), 'utf8');
const dependencies = manifest.split('[workspace.dependencies]')[1].split('\n[')[0];
for (const line of dependencies.split('\n').filter(line => line.includes('='))) {
  assert.match(line, /(?:version\s*=\s*|^\w+\s*=\s*)"=\d+\.\d+\.\d+"/, `Unpinned direct dependency: ${line}`);
}
let count = 0;
const local = new Set(['fuuki-iin-bot', 'verification-protocol']);
for (const block of lock.split('[[package]]').slice(1)) {
  const name = block.match(/^name = "([^"]+)"/m)?.[1];
  const source = block.match(/^source = "([^"]+)"/m)?.[1];
  if (!source) { assert.ok(local.has(name), `Unexpected local dependency: ${name}`); continue; }
  assert.equal(source, 'registry+https://github.com/rust-lang/crates.io-index', `Unexpected source: ${name}`);
  assert.match(block, /^checksum = "[a-f0-9]{64}"$/m, `Missing registry checksum: ${name}`); count++;
}
console.log(`Cargo.lock: ${count} registry packages with checksums; exact direct versions; only approved workspace crates.`);
