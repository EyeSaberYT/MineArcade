// Syntax-checks every frontend module and verifies that relative imports
// resolve to real files and exported names. Run with: npm run check
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', 'src', 'js');

function walk(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : path.endsWith('.js') ? [path] : [];
  });
}

const files = walk(root);
const exportsOf = new Map();
for (const file of files) {
  const source = readFileSync(file, 'utf8');
  const names = new Set();
  for (const match of source.matchAll(/export\s+(?:async\s+)?(?:function\*?|const|let|class)\s+([A-Za-z_$][\w$]*)/g)) names.add(match[1]);
  for (const match of source.matchAll(/export\s*\{([^}]+)\}/g)) {
    for (const part of match[1].split(',')) {
      const name = part.trim().split(/\s+as\s+/).pop();
      if (name) names.add(name);
    }
  }
  exportsOf.set(file, names);
}

let failures = 0;
for (const file of files) {
  const label = relative(root, file);
  const result = spawnSync(process.execPath, ['--input-type=module', '--check'], {
    input: readFileSync(file, 'utf8'),
    encoding: 'utf8',
  });
  if (result.status !== 0) {
    failures++;
    console.error(`✗ ${label}\n${result.stderr}`);
    continue;
  }

  const source = readFileSync(file, 'utf8');
  const importRe = /import\s+(?:([\w$]+)\s*,?\s*)?(?:\{([^}]*)\}\s*)?(?:\*\s+as\s+[\w$]+\s*)?from\s+['"](\.[^'"]+)['"]|import\(\s*['"](\.[^'"]+)['"]\s*\)/g;
  for (const match of source.matchAll(importRe)) {
    const specifier = match[3] ?? match[4];
    const target = resolve(dirname(file), specifier);
    if (!exportsOf.has(target)) {
      failures++;
      console.error(`✗ ${label}: cannot resolve "${specifier}"`);
      continue;
    }
    if (match[2]) {
      for (const part of match[2].split(',')) {
        const name = part.trim().split(/\s+as\s+/)[0];
        if (name && !exportsOf.get(target).has(name)) {
          failures++;
          console.error(`✗ ${label}: "${specifier}" has no export "${name}"`);
        }
      }
    }
  }
}

console.log(failures ? `\n${failures} problem(s) in ${files.length} files` : `✓ ${files.length} modules OK`);
process.exit(failures ? 1 : 0);
