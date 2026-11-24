import { execSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { defineConfig } from 'vitest/config';

const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')) as {
  version: string;
};

/** The short commit hash, `+dirty` with uncommitted changes; "unknown" outside git. */
function commit(): string {
  try {
    const hash = execSync('git rev-parse --short=8 HEAD', { encoding: 'utf8' }).trim();
    const dirty = execSync('git status --porcelain', { encoding: 'utf8' }).trim() !== '';
    return dirty ? `${hash}+dirty` : hash;
  } catch {
    return process.env.GITHUB_SHA?.slice(0, 8) ?? 'unknown';
  }
}

export default defineConfig(({ mode }) => ({
  define: {
    __KLIRR_VERSION__: JSON.stringify(pkg.version),
    __KLIRR_COMMIT__: JSON.stringify(commit()),
    __KLIRR_BUILD_DATE__: JSON.stringify(new Date().toISOString().slice(0, 10)),
  },
  build: {
    target: 'es2022',
    // The desktop app's UI (`--mode desktop`) is built next to the web app, not over it.
    outDir: mode === 'desktop' ? 'dist-desktop' : 'dist',
    // The wasm module is fetched at runtime; never inline it as a data URL.
    assetsInlineLimit: (file) => (file.endsWith('.wasm') ? false : undefined),
  },
  worker: { format: 'es' },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
}));
