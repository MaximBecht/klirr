// Writes target/notices/THIRD-PARTY-NOTICES.txt: the third-party software in the desktop app,
// with the license each is used under and the license texts with their copyright notices.
// Release builds install it next to Klirr's own licenses (src-tauri/tauri.release.conf.json).
//
// - Rust crates, from cargo-about (`cargo install cargo-about --locked --features cli`), for
//   every platform the app is released for and with the `asio` feature, so one file covers
//   every build: see notices/about.toml.
// - The npm packages the user interface bundles: web/'s dependencies, and Vite's runtime
//   helpers, which Vite writes into the bundle.
// - The prebuilt Microsoft and Steinberg code that is linked in on Windows.
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const desktopDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const repoDir = join(desktopDir, '..');
const webModules = join(repoDir, 'web', 'node_modules');
const outFile = join(repoDir, 'target', 'notices', 'THIRD-PARTY-NOTICES.txt');
const readJson = (file) => JSON.parse(readFileSync(file, 'utf8'));
const { version } = readJson(join(desktopDir, 'package.json'));

/** License texts, by text: { id, name, text, users: Set<string> } */
const texts = new Map();
function addText(id, name, text, user) {
    const key = text.replace(/\s+/g, ' ').trim();
    if (!texts.has(key)) texts.set(key, { id, name, text: text.trim(), users: new Set() });
    texts.get(key).users.add(user);
}

// --- Rust crates
let about;
try {
    about = JSON.parse(
        execFileSync(
            'cargo',
            [
                'about',
                'generate',
                '--format',
                'json',
                '--locked',
                '--features',
                'asio',
                '-m',
                join(desktopDir, 'src-tauri', 'Cargo.toml'),
                '-c',
                join(desktopDir, 'notices', 'about.toml'),
            ],
            {
                cwd: repoDir,
                encoding: 'utf8',
                maxBuffer: 256 * 2 ** 20,
                stdio: ['ignore', 'pipe', 'inherit'],
            },
        ),
    );
} catch (e) {
    console.error(
        'cargo about failed; install it with `cargo install cargo-about --locked --features cli`',
    );
    throw e;
}
/** crate label -> license ids it is used under */
const crates = new Map();
for (const license of about.licenses) {
    for (const { crate } of license.used_by) {
        if (crate.name.startsWith('klirr-')) continue; // Klirr's own
        const label = `${crate.name} ${crate.version}`;
        if (!crates.has(label)) crates.set(label, new Set());
        crates.get(label).add(license.id);
        addText(license.id, license.name, license.text, label);
    }
}

// --- npm packages
const preferred = ['MIT', 'Apache-2.0'];
/** package label -> license id */
const packages = new Map();
function addPackage(name, withDependencies = true) {
    const dir = join(webModules, ...name.split('/'));
    const pkg = readJson(join(dir, 'package.json'));
    const label = `${pkg.name} ${pkg.version}`;
    if (packages.has(label)) return;
    // "A OR B": take the first preferred choice, and its own license file if it has one
    const choices = String(pkg.license)
        .replace(/[()]/g, '')
        .split(/\s+OR\s+/);
    const id = preferred.find((p) => choices.includes(p)) ?? choices[0];
    packages.set(label, id);
    const files = readdirSync(dir).filter(
        (f) => /^(licen[cs]e|copying)/i.test(f) && !/\.spdx$/i.test(f),
    );
    const file = files.find((f) => f.toUpperCase().includes(id.toUpperCase())) ?? files[0];
    if (!file) throw new Error(`${label}: no license file`);
    let text = readFileSync(join(dir, file), 'utf8');
    // Vite's file goes on with the licenses of what Vite itself bundles; only its own applies
    if (pkg.name === 'vite')
        text = text.split(/^# Licenses of bundled dependencies/m)[0].replace(/^# .*$/m, '');
    addText(id, id, text, label);
    if (withDependencies) for (const dep of Object.keys(pkg.dependencies ?? {})) addPackage(dep);
}
for (const dep of Object.keys(readJson(join(repoDir, 'web', 'package.json')).dependencies))
    addPackage(dep);
// Vite's own runtime helpers only: its dependencies are build tools, none of them in the bundle
addPackage('vite', false);

// --- Prebuilt code linked in on Windows
const webview2 =
    'Microsoft WebView2 SDK: the WebView2 loader (WebView2LoaderStatic.lib from webview2-com-sys)';
addText(
    'BSD-3-Clause',
    'Microsoft WebView2 SDK License',
    readFileSync(join(desktopDir, 'notices', 'WebView2-SDK-LICENSE.txt'), 'utf8'),
    webview2,
);

// --- The file
const rule = '-'.repeat(78);
const list = (entries) =>
    [...entries]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([label, ids]) => `  ${label}  (${[...ids].join(', ')})`);
const out = [
    `Third-party notices for Klirr ${version}`,
    '='.repeat(78),
    '',
    'Klirr contains the third-party software listed here, each under the license named after',
    'it. The license texts, with their copyright notices, follow the lists. The list covers the',
    'desktop app on every platform it is released for, so it names some software that only',
    'one platform uses.',
    '',
    'The source code of each Rust crate is on crates.io (https://crates.io/crates/NAME/VERSION),',
    'and that of each npm package on npmjs.com (https://www.npmjs.com/package/NAME).',
    '',
    '',
    'Rust crates',
    rule,
    ...list(crates),
    '',
    '',
    'npm packages (the user interface)',
    rule,
    ...list([...packages].map(([label, id]) => [label, [id]])),
    '',
    '',
    'Other components (Windows)',
    rule,
    `  ${webview2}  (BSD-3-Clause)`,
    '  Steinberg ASIO SDK 2.3, (c) Steinberg Media Technologies GmbH, in the build with ASIO',
    '  support only  (GPL-3.0: see README.txt and GPL-3.0.txt next to this file)',
    '',
    '',
    'License texts',
    '='.repeat(78),
];
const sorted = [...texts.values()].sort(
    (a, b) => a.id.localeCompare(b.id) || b.users.size - a.users.size,
);
for (const t of sorted) {
    out.push(
        '',
        rule,
        `${t.name}`,
        `Used by: ${[...t.users].sort().join(', ')}`,
        rule,
        '',
        t.text,
        '',
    );
}

mkdirSync(dirname(outFile), { recursive: true });
writeFileSync(outFile, `${out.join('\n').replace(/[ \t]+$/gm, '')}\n`);
console.log(
    `THIRD-PARTY-NOTICES.txt: ${crates.size} crates, ${packages.size} npm packages, ${texts.size} license texts`,
);
