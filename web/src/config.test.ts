import { describe, expect, it } from 'vitest';
import {
  emptyConfig,
  readConfig,
  recallBuffer,
  recallHost,
  writeConfig,
  type DesktopConfig,
} from './config';
import { P, defaults } from './params';
import { defaultSettings } from './settings';

const DEFAULT_HOST = 'WASAPI';

function stored(): DesktopConfig {
  return {
    version: 1,
    audio: {
      host: 'asio',
      hosts: {
        asio: {
          input: 'Focusrite USB ASIO',
          output: 'Focusrite USB ASIO',
          channel: 1,
          devices: { 'Focusrite USB ASIO': { buffer: 64 } },
        },
        WASAPI: { input: 'Mic', output: '', channel: 0, devices: {} },
      },
    },
    params: { gain: 0.8 },
  };
}

describe('desktop config', () => {
  it('reads the current host and its devices', () => {
    const s = readConfig(stored(), DEFAULT_HOST);
    expect(s.host).toBe('asio');
    expect(s.inputDeviceId).toBe('Focusrite USB ASIO');
    expect(s.outputDeviceId).toBe('Focusrite USB ASIO');
    expect(s.inputChannel).toBe(1);
    expect(s.buffer).toBe(64);
    expect(s.params[P.Gain]).toBe(0.8);
  });

  it('round-trips through the file', () => {
    const s = readConfig(stored(), DEFAULT_HOST);
    expect(readConfig(writeConfig(stored(), s, DEFAULT_HOST), DEFAULT_HOST)).toEqual(s);
  });

  it('keeps each host its own devices and buffer', () => {
    const config = stored();
    const s = readConfig(config, DEFAULT_HOST);
    s.host = '';
    recallHost(config, s, DEFAULT_HOST);
    expect(s.inputDeviceId).toBe('Mic');
    expect(s.buffer).toBeNull();
    s.buffer = 256;
    const written = writeConfig(config, s, DEFAULT_HOST);
    // The default host is kept under its own id, and ASIO's entry is untouched.
    expect(written.audio.host).toBe('');
    expect(written.audio.hosts.WASAPI?.devices['']?.buffer).toBe(256);
    expect(written.audio.hosts.asio).toEqual(config.audio.hosts.asio);
    s.host = 'asio';
    recallHost(written, s, DEFAULT_HOST);
    expect(s.buffer).toBe(64);
    expect(s.inputChannel).toBe(1);
  });

  it('keeps a buffer per output device', () => {
    const config = stored();
    const s = readConfig(config, DEFAULT_HOST);
    s.outputDeviceId = 'Other';
    recallBuffer(config, s, DEFAULT_HOST);
    expect(s.buffer).toBeNull();
    s.buffer = 128;
    const written = writeConfig(config, s, DEFAULT_HOST);
    const devices = written.audio.hosts.asio?.devices;
    expect(devices?.Other?.buffer).toBe(128);
    expect(devices?.['Focusrite USB ASIO']?.buffer).toBe(64);
  });

  it('keeps a sample rate per output device, with its buffer', () => {
    const config = stored();
    const s = readConfig(config, DEFAULT_HOST);
    expect(s.rate).toBeNull();
    s.host = 'wasapi-exclusive';
    recallHost(config, s, DEFAULT_HOST);
    s.rate = 96000;
    s.buffer = 288;
    const written = writeConfig(config, s, DEFAULT_HOST);
    expect(written.audio.hosts['wasapi-exclusive']?.devices['']).toMatchObject({
      buffer: 288,
      rate: 96000,
    });
    s.outputDeviceId = 'Other';
    recallBuffer(written, s, DEFAULT_HOST);
    expect(s.rate).toBeNull();
    s.outputDeviceId = '';
    recallBuffer(written, s, DEFAULT_HOST);
    expect(s.rate).toBe(96000);
  });

  it('hands back what it does not know', () => {
    const config: DesktopConfig = {
      ...stored(),
      theme: 'dark',
      params: { gain: 0.8, future_knob: 3 },
    };
    config.audio.exclusive = true;
    const asio = config.audio.hosts.asio;
    if (!asio) throw new Error('fixture');
    asio.sampleRate = 96000;
    const device = asio.devices['Focusrite USB ASIO'];
    if (!device) throw new Error('fixture');
    device.clock = 'spdif';
    const written = writeConfig(config, readConfig(config, DEFAULT_HOST), DEFAULT_HOST);
    expect(written.theme).toBe('dark');
    expect(written.params.future_knob).toBe(3);
    expect(written.audio.exclusive).toBe(true);
    expect(written.audio.hosts.asio?.sampleRate).toBe(96000);
    expect(written.audio.hosts.asio?.devices['Focusrite USB ASIO']?.clock).toBe('spdif');
  });

  it('falls back to defaults for invalid values', () => {
    const config = stored();
    const asio = config.audio.hosts.asio;
    if (!asio) throw new Error('fixture');
    asio.channel = -3;
    asio.devices['Focusrite USB ASIO'] = { buffer: 1.5 };
    config.params = { gain: 'loud', bypass: 1 };
    const s = readConfig(config, DEFAULT_HOST);
    expect(s.inputChannel).toBe(0);
    expect(s.buffer).toBeNull();
    expect(s.params).toEqual(defaults()); // bypass is never restored
  });

  it('takes over the webview settings on the first run', () => {
    const legacy = { ...defaultSettings(), host: 'wasapi-exclusive', inputDeviceId: 'Line' };
    const config = writeConfig(emptyConfig(), legacy, DEFAULT_HOST);
    expect(config.audio.host).toBe('wasapi-exclusive');
    expect(config.audio.hosts['wasapi-exclusive']?.input).toBe('Line');
    expect(readConfig(config, DEFAULT_HOST)).toEqual(legacy);
  });
});
