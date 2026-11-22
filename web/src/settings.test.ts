import { describe, expect, it } from 'vitest';
import { P, defaults } from './params';
import { defaultSettings, loadSettings, saveSettings } from './settings';

function memoryStorage(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => {
      data.set(k, v);
    },
  };
}

describe('settings', () => {
  it('round-trips', () => {
    const storage = memoryStorage();
    const s = defaultSettings();
    s.params[P.Gain] = 0.9;
    s.params[P.Channel] = 2;
    s.host = 'wasapi-exclusive';
    s.inputDeviceId = 'abc';
    s.inputChannel = 5;
    saveSettings(storage, s);
    expect(loadSettings(storage)).toEqual(s);
  });

  it('falls back to defaults for missing, broken or out-of-range data', () => {
    expect(loadSettings(undefined)).toEqual(defaultSettings());
    expect(loadSettings(memoryStorage({ 'klirr.settings.v1': '{oops' }))).toEqual(
      defaultSettings(),
    );
    const s = loadSettings(
      memoryStorage({
        'klirr.settings.v1': JSON.stringify({
          params: { gain: 7, channel: 1.6, bass_db: 'loud', unknown: 1, bypass: 1 },
          inputChannel: 1.5,
          outputDeviceId: 42,
          host: null,
        }),
      }),
    );
    const expected = defaults();
    expected[P.Gain] = 1;
    expected[P.Channel] = 2;
    expect(s.params).toEqual(expected); // bypass is never restored
    expect(s.inputChannel).toBe(0);
    expect(s.outputDeviceId).toBe('');
    expect(s.host).toBe('');
    for (const inputChannel of [-1, 64, '1']) {
      const stored = JSON.stringify({ inputChannel });
      expect(loadSettings(memoryStorage({ 'klirr.settings.v1': stored })).inputChannel).toBe(0);
    }
  });

  it('survives a storage that throws', () => {
    const throwing = {
      setItem: () => {
        throw new Error('quota');
      },
    };
    expect(() => {
      saveSettings(throwing, defaultSettings());
    }).not.toThrow();
  });
});
