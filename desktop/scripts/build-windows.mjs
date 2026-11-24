// Builds the Windows installer with both builds of Klirr: the ASIO one (GPLv3) first, set
// aside, then the MIT one, bundled by the NSIS template in src-tauri/windows/ with the ASIO
// build as its optional, pre-selected component. The installer lands in
// target/release/bundle/nsis/.
//
// The ASIO build needs LLVM (LIBCLANG_PATH) and the ASIO SDK (CPAL_ASIO_DIR, or the one asio-sys
// downloads), and the notices need cargo-about; see the README.
import { execSync } from 'node:child_process';
import {
    copyFileSync,
    mkdirSync,
    readFileSync,
    readdirSync,
    statSync,
    writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

if (process.platform !== 'win32') {
    console.error('build:windows builds the Windows installer, on Windows only');
    process.exit(1);
}

const desktopDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const releaseDir = join(desktopDir, '..', 'target', 'release');
const run = (command, env = {}) =>
    execSync(command, { cwd: desktopDir, stdio: 'inherit', env: { ...process.env, ...env } });

// Release builds carry the third-party notices (tauri.release.conf.json adds them)
run('node scripts/notices.mjs');
const release = '--config src-tauri/tauri.release.conf.json';

run(`npx tauri build ${release} --no-bundle --features asio`);
const asioDir = join(releaseDir, 'asio');
mkdirSync(asioDir, { recursive: true });
const asioExe = join(asioDir, 'klirr.exe');
copyFileSync(join(releaseDir, 'klirr.exe'), asioExe);

// Tauri marks the binary it bundles with the bundle type (read by its updater, for one), in
// place of "unknown"; the ASIO build skipped bundling, so mark it here the same way.
const marker = Buffer.from('__TAURI_BUNDLE_TYPE_VAR_UNK');
const exe = readFileSync(asioExe);
const at = exe.indexOf(marker);
if (at < 0 || exe.indexOf(marker, at + 1) >= 0) {
    console.error('The ASIO build has no single bundle type marker to set');
    process.exit(1);
}
exe.write('NSS', at + marker.length - 3, 'latin1');
writeFileSync(asioExe, exe);

run(`npx tauri build ${release} --bundles nsis`, { KLIRR_ASIO_EXE: asioExe });

const mib = (file) => `${(statSync(file).size / 2 ** 20).toFixed(2)} MiB`;
console.log(`klirr.exe: ${mib(join(releaseDir, 'klirr.exe'))} (MIT), ${mib(asioExe)} (ASIO)`);
const nsisDir = join(releaseDir, 'bundle', 'nsis');
for (const file of readdirSync(nsisDir).filter((f) => f.endsWith('-setup.exe'))) {
    console.log(`${file}: ${mib(join(nsisDir, file))}`);
}
