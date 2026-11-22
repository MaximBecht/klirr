import { defineConfig } from 'vitest/config';

export default defineConfig(({ mode }) => ({
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
