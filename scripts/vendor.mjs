// Copies the browser builds of the frontend's third-party libraries into
// src/vendor, so the app needs no bundler. Run after updating them:
//   npm run vendor
import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const out = join(root, 'src', 'vendor');
mkdirSync(out, { recursive: true });

const files = [
  // 3D skin viewer (bundles three.js), exposes `window.skinview3d`.
  ['node_modules/skinview3d/bundles/skinview3d.bundle.js', 'skinview3d.bundle.js'],
  // Markdown for project descriptions, exposes `window.marked`.
  ['node_modules/marked/lib/marked.umd.js', 'marked.umd.js'],
  // HTML sanitiser for that markdown, exposes `window.DOMPurify`.
  ['node_modules/dompurify/dist/purify.min.js', 'purify.min.js'],
];

for (const [from, to] of files) {
  copyFileSync(join(root, from), join(out, to));
  console.log(`vendored ${to}`);
}
