// An explicit static asset allowlist keeps secrets and server code out of Pages.
import { readFile, writeFile, mkdir, rm, copyFile } from 'node:fs/promises';
import { stripTypeScriptTypes } from 'node:module';
const output = new URL('../dist/verification/', import.meta.url);
const assets = new URL('../apps/verification/public/', import.meta.url);
await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
for (const file of ['index.html', 'style.css', 'app.js', 'view.js', 'i18n.js', '_headers', '_redirects']) {
  await copyFile(new URL(file, assets), new URL(file, output));
}
const protocol = await readFile(new URL('../packages/verification-protocol/src/index.ts', import.meta.url), 'utf8');
await writeFile(new URL('protocol.js', output), stripTypeScriptTypes(protocol, { mode: 'strip' }));
console.log('Pages static assets built in dist/verification (no Worker or Functions).');
