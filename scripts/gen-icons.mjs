// Generates the Orbit Launcher icon set from src-tauri/icons/source/orbit-icon.png (1024x1024).
// The source is drawn by Orbit Client's tools/ModIconGenerator.java, so the launcher and the mod
// share one icon. Run with: npm run icons
import { copyFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const iconDir = join(root, 'src-tauri', 'icons');
const source = join(iconDir, 'source', 'orbit-icon.png');

// `tauri icon` writes every platform's sizes; only the ones tauri.conf.json lists are kept.
const out = mkdtempSync(join(tmpdir(), 'orbit-icons-'));
try {
  execFileSync(process.execPath, [join(root, 'scripts', 'run-tauri.mjs'), 'icon', source, '-o', out], { stdio: 'inherit' });
  for (const name of ['32x32.png', '128x128.png', '128x128@2x.png', 'icon.png', 'icon.ico']) {
    copyFileSync(join(out, name), join(iconDir, name));
  }
  copyFileSync(join(out, '128x128.png'), join(root, 'src', 'assets', 'logo.png'));
  console.log('Icons written to', iconDir);
} finally {
  rmSync(out, { recursive: true, force: true });
}
