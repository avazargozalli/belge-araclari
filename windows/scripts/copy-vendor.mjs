// Copies pdf.js from node_modules into ui/vendor so the app ships it locally (no CDN, no network).
import { cpSync, mkdirSync, rmSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = join(root, 'node_modules', 'pdfjs-dist');
const target = join(root, 'ui', 'vendor', 'pdfjs');
if (!existsSync(source)) {
  console.error('pdfjs-dist bulunamadı. Önce "npm ci" çalıştırın.');
  process.exit(1);
}
rmSync(target, { recursive: true, force: true });
mkdirSync(target, { recursive: true });
for (const file of ['build/pdf.min.mjs', 'build/pdf.worker.min.mjs', 'LICENSE']) {
  cpSync(join(source, file), join(target, file.split('/').pop()));
}
// The QuickJS scripting sandbox is left out: PDF JavaScript is never executed.
const skip = (path) => /quickjs/i.test(path);
for (const folder of ['cmaps', 'standard_fonts', 'wasm', 'iccs']) {
  if (existsSync(join(source, folder))) cpSync(join(source, folder), join(target, folder), { recursive: true, filter: (p) => !skip(p) });
}
console.log('pdf.js →', target);
