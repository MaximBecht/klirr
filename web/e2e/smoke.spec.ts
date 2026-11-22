// Smoke test: the built app starts, sound flows from the (fake) microphone through the
// engine to the output, and stopping works. It does not judge how the amp sounds; the DSP
// tests in klirr-dsp do that.

import { test as base, expect, type Page } from '@playwright/test';
import { NOTE_PEAK } from './signal';

/** Level statistics of one probe (input or output) over a measuring period. */
interface Levels {
  maxPeak: number;
  maxRms: number;
  minRms: number;
  nonFinite: number;
}

declare global {
  interface Window {
    __probes?: { input?: AnalyserNode; output?: AnalyserNode; contexts: AudioContext[] };
  }
}

/**
 * Taps the app's audio graph without touching the app: any node connected to the
 * destination, and any microphone source, also feeds an AnalyserNode the test can read.
 */
function installProbes(): void {
  const probes: NonNullable<Window['__probes']> = { contexts: [] };
  window.__probes = probes;
  // Kept unbound on purpose: it is always called with an explicit `this` below.
  // eslint-disable-next-line @typescript-eslint/unbound-method
  const connect = AudioNode.prototype.connect as (
    this: AudioNode,
    ...args: unknown[]
  ) => AudioNode | undefined;
  const tap = (node: AudioNode): AnalyserNode => {
    const a = new AnalyserNode(node.context, { fftSize: 2048 });
    connect.call(node, a);
    return a;
  };
  AudioNode.prototype.connect = function (this: AudioNode, ...args: unknown[]) {
    if (args[0] instanceof AudioDestinationNode) {
      probes.output = tap(this);
      probes.contexts.push(this.context as AudioContext);
    }
    if (this instanceof MediaStreamAudioSourceNode) probes.input = tap(this);
    return connect.apply(this, args);
  } as typeof AudioNode.prototype.connect;
}

/** Reads both probes every 20 ms for `ms` and returns their level statistics. */
async function measure(page: Page, ms: number): Promise<{ input: Levels; output: Levels }> {
  return page.evaluate(async (duration) => {
    const probes = window.__probes;
    if (!probes?.input || !probes.output) throw new Error('audio graph not tapped');
    const read = (a: AnalyserNode, buf: Float32Array<ArrayBuffer>, s: Levels) => {
      a.getFloatTimeDomainData(buf);
      let peak = 0;
      let sum = 0;
      for (const v of buf) {
        if (!Number.isFinite(v)) s.nonFinite++;
        peak = Math.max(peak, Math.abs(v));
        sum += v * v;
      }
      const rms = Math.sqrt(sum / buf.length);
      s.maxPeak = Math.max(s.maxPeak, peak);
      s.maxRms = Math.max(s.maxRms, rms);
      s.minRms = Math.min(s.minRms, rms);
    };
    const fresh = (): Levels => ({ maxPeak: 0, maxRms: 0, minRms: Infinity, nonFinite: 0 });
    const input = fresh();
    const output = fresh();
    const inBuf = new Float32Array(probes.input.fftSize);
    const outBuf = new Float32Array(probes.output.fftSize);
    const end = performance.now() + duration;
    while (performance.now() < end) {
      read(probes.input, inBuf, input);
      read(probes.output, outBuf, output);
      await new Promise((r) => setTimeout(r, 20));
    }
    return { input, output };
  }, ms);
}

const dbfs = (linear: number) => 20 * Math.log10(Math.max(linear, 1e-12));
/** −1 dBFS, the safety limiter's ceiling (klirr_dsp::limiter::CEILING_DB), plus a hair. */
const CEILING = 10 ** (-1 / 20) + 1e-3;
/** One full loop of the test signal (2 s) plus margin. */
const LOOP_MS = 2500;

const test = base.extend<{ app: Page }>({
  // Opens the app with the probes installed, and fails the test on any console or page error.
  app: async ({ page }, use) => {
    const problems: string[] = [];
    page.on('console', (msg) => {
      if (msg.type() === 'error') problems.push(`console: ${msg.text()}`);
    });
    page.on('pageerror', (err) => {
      problems.push(`page error: ${err.message}`);
    });
    await page.addInitScript(installProbes);
    await page.goto('/');
    await expect(page.locator('#status-text')).toHaveText('OFFLINE');
    await use(page);
    expect(problems).toEqual([]);
  },
});

async function engage(page: Page): Promise<void> {
  await page.locator('#start').click();
  await expect(page.locator('#status-text')).toHaveText('ACTIVE', { timeout: 15_000 });
  await expect(page.locator('#footer-status')).not.toHaveClass(/is-error/);
}

test('engages, processes the microphone and stops', async ({ app: page }) => {
  await engage(page);
  await expect(page.locator('#tube')).toHaveClass(/is-on/);
  await expect(page.locator('#stop')).toBeEnabled();
  await expect(page.locator('#latency')).toHaveText(/LATENCY ≈ \d+ MS/);
  // With access granted, the fake device shows up by name.
  await expect(page.locator('#input-device option')).not.toHaveCount(1);

  const { input, output } = await measure(page, LOOP_MS);
  // The fake microphone delivers our note…
  expect(input.nonFinite).toBe(0);
  expect(dbfs(input.maxPeak)).toBeGreaterThan(dbfs(NOTE_PEAK) - 3);
  // …and the amp turns it into clearly audible, finite output under the limiter ceiling.
  expect(output.nonFinite).toBe(0);
  expect(dbfs(output.maxRms)).toBeGreaterThan(-30);
  expect(output.maxPeak).toBeLessThanOrEqual(CEILING);
  // During the silent half of the loop the output is silent too: the sound comes from the
  // input, not from the engine itself.
  expect(dbfs(output.minRms)).toBeLessThan(-60);

  await page.locator('#stop').click();
  await expect(page.locator('#status-text')).toHaveText('OFFLINE');
  await expect(page.locator('#start')).toBeEnabled();
  const states = await page.evaluate(() => window.__probes?.contexts.map((c) => c.state));
  expect(states).toEqual(['closed']);
});

test('bypass passes the dry signal through', async ({ app: page }) => {
  await engage(page);
  await page.locator('#bypass').click();
  await expect(page.locator('#bypass')).toHaveAttribute('aria-pressed', 'true');
  await page.waitForTimeout(200); // let the bypass crossfade finish

  const { input, output } = await measure(page, LOOP_MS);
  expect(output.nonFinite).toBe(0);
  // Dry means the output peaks where the input does (within 1 dB).
  expect(Math.abs(dbfs(output.maxPeak) - dbfs(input.maxPeak))).toBeLessThan(1);
});

test('every amp channel produces sound', async ({ app: page }) => {
  await engage(page);
  const buttons = page.locator('#channel button');
  const count = await buttons.count();
  expect(count).toBeGreaterThan(1);
  for (let i = 0; i < count; i++) {
    await buttons.nth(i).click();
    const { output } = await measure(page, LOOP_MS);
    expect(output.nonFinite, `channel ${i}`).toBe(0);
    expect(dbfs(output.maxRms), `channel ${i}`).toBeGreaterThan(-30);
    expect(output.maxPeak, `channel ${i}`).toBeLessThanOrEqual(CEILING);
  }
});
