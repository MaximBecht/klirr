import { defineConfig } from 'vitest/config';

export default defineConfig({
  build: {
    target: 'es2022',
    // The wasm module is fetched at runtime; never inline it as a data URL.
    assetsInlineLimit: (file) => (file.endsWith('.wasm') ? false : undefined),
  },
  worker: { format: 'es' },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
});
