// End-to-end smoke tests: the production build in real Chromium, with a fake microphone.
// Run with `npm run test:e2e` (needs the wasm module: `npm run build:wasm`).

import { defineConfig, devices } from '@playwright/test';
import { join } from 'node:path';
import { writeFakeMicWav } from './e2e/signal';

const PORT = 4174;
const fakeMic = join(import.meta.dirname, 'e2e', '.generated', 'fake-mic.wav');
writeFakeMicWav(fakeMic);

export default defineConfig({
  testDir: 'e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        permissions: ['microphone'],
        launchOptions: {
          args: [
            // Replace the real capture devices with a fake one that plays our WAV (looped).
            '--use-fake-device-for-media-stream',
            `--use-file-for-fake-audio-capture=${fakeMic}`,
            '--use-fake-ui-for-media-stream',
            // Audio is still rendered and measurable, but nothing reaches the speakers.
            '--mute-audio',
          ],
        },
      },
    },
  ],
  webServer: {
    // Always test a fresh production build, on a port of its own.
    command: `npx vite build && npx vite preview --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    timeout: 120_000,
    stdout: 'ignore',
    stderr: 'pipe',
  },
});
