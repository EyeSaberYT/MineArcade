// Runs the Tauri CLI with rustup's toolchain first on PATH.
//
// A standalone Rust (e.g. Chocolatey's GNU build) earlier on PATH shadows
// rustup, and building with it fails with
// "error calling dlltool 'dlltool.exe': program not found".
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const cli = path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
if (!existsSync(cli)) {
  console.error('[run-tauri] Tauri CLI not found. Run "npm install" first.');
  process.exit(1);
}

const env = { ...process.env };
const cargoBin = path.join(env.CARGO_HOME ?? path.join(os.homedir(), '.cargo'), 'bin');
if (existsSync(cargoBin)) {
  // Windows env names are case-insensitive; keep a single PATH key.
  const key = Object.keys(env).find((k) => k.toLowerCase() === 'path') ?? 'PATH';
  env[key] = `${cargoBin}${path.delimiter}${env[key] ?? ''}`;
}

const { status, error } = spawnSync(process.execPath, [cli, ...process.argv.slice(2)], {
  stdio: 'inherit',
  env,
  cwd: root,
});
if (error) {
  console.error(`[run-tauri] failed to start the Tauri CLI: ${error.message}`);
  process.exit(1);
}
process.exit(status ?? 1);
