// Klirr's version, which lives in several files: the workspace's Cargo.toml (every crate takes
// it from there) and Cargo.lock, and package.json and package-lock.json in web/ (the version the
// app shows) and desktop/.
//
//   node scripts/version.mjs                 prints it, after checking that the files agree
//   node scripts/version.mjs check v1.2.3    also checks it against a release tag
//   node scripts/version.mjs set 1.2.3       writes it to every file
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const path = (file) => join(repoDir, file);
const read = (file) => readFileSync(path(file), 'utf8');

// Cargo.toml: `version` in [workspace.package]
const cargoToml = /(\[workspace\.package\][^[]*?\nversion = ")([^"]+)(")/;
// Cargo.lock: the entries of Klirr's own crates
const cargoLock = /(\[\[package\]\]\nname = "klirr-[a-z]+"\nversion = ")([^"]+)(")/g;
const npmDirs = ['web', 'desktop'];

function versions() {
    const found = {};
    found['Cargo.toml'] = cargoToml.exec(read('Cargo.toml'))?.[2];
    for (const m of read('Cargo.lock').matchAll(cargoLock)) found[`Cargo.lock (${m[0].split('"')[1]})`] = m[2];
    for (const dir of npmDirs) {
        found[`${dir}/package.json`] = JSON.parse(read(`${dir}/package.json`)).version;
        const lock = JSON.parse(read(`${dir}/package-lock.json`));
        found[`${dir}/package-lock.json`] = lock.version;
        found[`${dir}/package-lock.json (packages[""])`] = lock.packages[''].version;
    }
    return found;
}

function fail(message) {
    console.error(message);
    process.exit(1);
}

const [command, arg] = process.argv.slice(2);

if (command === 'set') {
    if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(arg ?? '')) fail('usage: version.mjs set 1.2.3[-alpha.1]');
    writeFileSync(path('Cargo.toml'), read('Cargo.toml').replace(cargoToml, `$1${arg}$3`));
    writeFileSync(path('Cargo.lock'), read('Cargo.lock').replace(cargoLock, `$1${arg}$3`));
    for (const dir of npmDirs) {
        for (const file of [`${dir}/package.json`, `${dir}/package-lock.json`]) {
            const json = JSON.parse(read(file));
            json.version = arg;
            if (json.packages?.['']) json.packages[''].version = arg;
            writeFileSync(path(file), `${JSON.stringify(json, null, 2)}\n`);
        }
    }
}

const found = versions();
const distinct = [...new Set(Object.values(found))];
if (distinct.length !== 1 || !distinct[0]) {
    fail(`The versions disagree:\n${Object.entries(found).map(([f, v]) => `  ${f}: ${v}`).join('\n')}`);
}
const version = distinct[0];
if (command === 'check' && arg !== undefined && arg !== `v${version}`) {
    fail(`The tag ${arg} does not match the version ${version}: tag v${version}, or set the version`);
}
if (command && !['set', 'check'].includes(command)) fail(`unknown command: ${command}`);
console.log(version);
