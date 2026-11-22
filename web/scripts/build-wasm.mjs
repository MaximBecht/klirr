// Builds crates/klirr-wasm for wasm32-unknown-unknown and copies the module into src/wasm/,
// where Vite picks it up as a hashed asset.
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const webDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const repoDir = join(webDir, '..');

execFileSync(
    'cargo',
    ['build', '-p', 'klirr-wasm', '--release', '--target', 'wasm32-unknown-unknown', '--locked'],
    { cwd: repoDir, stdio: 'inherit' },
);

const built = join(repoDir, 'target', 'wasm32-unknown-unknown', 'release', 'klirr_wasm.wasm');
const outDir = join(webDir, 'src', 'wasm');
mkdirSync(outDir, { recursive: true });
copyFileSync(built, join(outDir, 'klirr.wasm'));
console.log(`klirr.wasm: ${(statSync(built).size / 1024).toFixed(0)} KiB`);
