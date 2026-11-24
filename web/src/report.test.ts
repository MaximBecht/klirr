import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { defaults } from './params';
import {
  BUILD,
  RecentMessages,
  SUPPORT_EMAIL,
  mailtoUrl,
  reportFileName,
  reportText,
  utcStamp,
  type ReportInput,
} from './report';

const created = new Date(Date.UTC(2026, 10, 24, 18, 43, 7));

function input(overrides: Partial<ReportInput> = {}): ReportInput {
  return {
    build: { version: '0.1.0', commit: 'abcdef12', date: '2026-11-24' },
    platform: 'desktop',
    native: {
      version: '0.1.0',
      asio: true,
      os: 'Windows 11 Pro 10.0.26300 (64-bit)',
      arch: 'x86_64',
      webview: '131.0.2903.112',
      log: ['2026-11-24 18:40:00Z INFO  Klirr 0.1.0'],
      previousLog: null,
      logPath: '~\\AppData\\Local\\com.bechtstudio.klirr\\logs\\klirr.log',
    },
    userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)',
    audio: [
      ['State', 'running'],
      ['Driver', 'WASAPI exclusive'],
    ],
    params: defaults(),
    messages: [],
    created,
    ...overrides,
  };
}

describe('bug report', () => {
  it('names the file after the time it was made', () => {
    expect(reportFileName(created)).toBe('klirr-report-20261124-184307.txt');
    expect(utcStamp(created)).toBe('2026-11-24 18:43:07Z');
  });

  it('holds the version, system, audio, settings and log', () => {
    const text = reportText(input());
    expect(text).toContain('0.1.0 (desktop app, with ASIO)');
    expect(text).toContain('abcdef12 · 2026-11-24');
    expect(text).toContain('Windows 11 Pro 10.0.26300 (64-bit), x86_64');
    expect(text).toMatch(/Driver: +WASAPI exclusive/);
    expect(text).toContain('gain=0.5');
    expect(text).toContain('INFO  Klirr 0.1.0');
    expect(text).toContain('Log file: ~\\AppData');
    expect(text).not.toContain('previous run');
  });

  it('includes the previous run only when it did not end cleanly', () => {
    const base = input();
    const native = base.native ? { ...base.native, previousLog: ['panic: oops'] } : null;
    const text = reportText({ ...base, native });
    expect(text).toContain('previous run, which did not end cleanly');
    expect(text).toContain('panic: oops');
  });

  it('describes the browser by its user agent', () => {
    const text = reportText(input({ platform: 'web', native: null }));
    expect(text).toContain('0.1.0 (web app)');
    expect(text).toContain('Mozilla/5.0 (Windows NT 10.0; Win64; x64)');
    expect(text).not.toContain('Log of this run');
  });

  it('writes a short email, with the setup only when the user agreed', () => {
    const without = decodeURIComponent(mailtoUrl(input(), null));
    expect(without.startsWith(`mailto:${SUPPORT_EMAIL}?subject=Klirr 0.1.0 bug report`)).toBe(true);
    expect(without).toContain('What happened:');
    expect(without).not.toContain('Windows 11');
    expect(without).not.toContain('attach');

    const url = mailtoUrl(input(), 'klirr-report-20261124-184307.txt');
    const withReport = decodeURIComponent(url);
    expect(withReport).toContain('Windows 11 Pro 10.0.26300 (64-bit) · WASAPI exclusive');
    expect(withReport).toContain('attach the report file: klirr-report-20261124-184307.txt');
    expect(withReport).toContain('\r\n');
    // Mail programs cut long links.
    expect(url.length).toBeLessThan(1500);
  });

  it('keeps the latest messages', () => {
    const m = new RecentMessages(2);
    m.push('one', false, created);
    m.push('two', true, created);
    m.push('three', false, created);
    expect(m.all()).toEqual(['18:43:07Z ERROR two', '18:43:07Z three']);
  });
});

describe('version', () => {
  const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

  it('is the same for the web app, the desktop app and the Rust crates', () => {
    const cargo = /^\[workspace\.package\][^[]*?^version = "([^"]+)"/m.exec(
      read('../../Cargo.toml'),
    );
    const desktop = JSON.parse(read('../../desktop/package.json')) as { version: string };
    expect(cargo?.[1]).toBe(BUILD.version);
    expect(desktop.version).toBe(BUILD.version);
  });
});
