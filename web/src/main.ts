import '@fontsource/bebas-neue';
import '@fontsource/share-tech-mono';
import '@fontsource/barlow/300.css';
import '@fontsource/barlow/400.css';
import '@fontsource/barlow/600.css';
import './style.css';

import type { BufferChoice, HostInfo, NativeEngine } from './audio/native';
import type {
  AmpEngine,
  DeviceInfo,
  EngineState,
  LatencyEstimate,
  Measurement,
  StartOptions,
} from './audio/types';
import { readConfig, recallBuffer, recallHost, writeConfig, type DesktopConfig } from './config';
import { CABINET_CUSTOM, CABINET_NAMES, CHANNEL_NAMES, P, spec } from './params';
import { loadSettings, saveSettings } from './settings';
import * as fmt from './ui/format';
import { Knob, el, type KnobOptions } from './ui/knob';
import { Meter } from './ui/meter';
import { Segmented } from './ui/segmented';

/**
 * The desktop app's UI (`vite --mode desktop`), whose audio runs on the native host, or the
 * web app. A build-time constant: each build drops the other's engine. It has to be decided
 * in this module; Rollup does not follow a constant imported from another one.
 */
const DESKTOP = import.meta.env.MODE === 'desktop';

// ── Elements ───────────────────────────────────────────────
function $(id: string): HTMLElement {
  const e = document.getElementById(id);
  if (!e) throw new Error(`#${id} missing`);
  return e;
}
const app = $('app');
const tube = document.getElementById('tube') as SVGElement | null;
const power = $('power');
const statusText = $('status-text');
const hostSelect = $('host') as HTMLSelectElement;
const inputSelect = $('input-device') as HTMLSelectElement;
const channelSelect = $('input-channel') as HTMLSelectElement;
const outputSelect = $('output-device') as HTMLSelectElement;
const startBtn = $('start') as HTMLButtonElement;
const stopBtn = $('stop') as HTMLButtonElement;
const bypassBtn = $('bypass') as HTMLButtonElement;
const deviceNotice = $('device-notice');
const driverLine = $('driver-line');
const audioPanel = $('audio-panel');
const audioError = $('audio-error');
const footerStatus = $('footer-status');
const latencyButton = $('latency');
const latencyPanel = $('latency-info');
const latInput = $('lat-input');
const latOutput = $('lat-output');
const latEngine = $('lat-engine');
const latTotal = $('lat-total');
const latMeasured = $('lat-measured');
const latMeasuredDetail = $('lat-measured-detail');
const latMeasureBtn = $('lat-measure') as HTMLButtonElement;
const latDropouts = $('lat-dropouts');
const irButton = $('load-ir') as HTMLButtonElement;
const irFile = $('ir-file') as HTMLInputElement;
const irName = $('ir-name');
const dropOverlay = $('drop-overlay');
const bufferSelect = $('buffer') as HTMLSelectElement;
const bufferDetail = $('buffer-detail');
const rateSelect = $('sample-rate') as HTMLSelectElement;
const rateDetail = $('sample-rate-detail');
const setupButton = $('hardware-setup') as HTMLButtonElement;
const setupDetail = $('hardware-setup-detail');
const toneButton = $('test-tone') as HTMLButtonElement;
const audioLatInput = $('audio-lat-input');
const audioLatOutput = $('audio-lat-output');
const audioLatEngine = $('audio-lat-engine');
const audioLatTotal = $('audio-lat-total');
const audioLatMeasured = $('audio-lat-measured');
const audioLatDetail = $('audio-lat-detail');
const audioLatEngageRow = $('audio-lat-engage-row');
const audioLatEngage = $('audio-lat-engage') as HTMLButtonElement;
const configPath = $('config-path');

// ── Platform ───────────────────────────────────────────────
// Texts and controls for one platform only: `data-web` (the browser) or `data-desktop`
// (the desktop app, hidden until shown here). The stylesheet frames the amp per platform.
document.documentElement.dataset.platform = DESKTOP ? 'desktop' : 'web';
for (const e of document.querySelectorAll(DESKTOP ? '[data-web]' : '[data-desktop]')) e.remove();
for (const e of document.querySelectorAll<HTMLElement>('[data-desktop]')) e.hidden = false;

// ── Panels ─────────────────────────────────────────────────
// What opens a panel (a menu item, the status cluster) looks pressed while it is open.
for (const panel of document.querySelectorAll<HTMLElement>('.panel[popover]')) {
  const openers = document.querySelectorAll(
    `[popovertarget="${panel.id}"]:not([popovertargetaction="hide"])`,
  );
  panel.addEventListener('toggle', (e) => {
    const open = e.newState === 'open';
    for (const o of openers) o.classList.toggle('is-open', open);
  });
}

// The other platform's engine is not in the build at all.
let engine: AmpEngine;
/** The desktop app's engine, for its extras (audio hosts); null in the browser. */
let native: NativeEngine | null = null;
/** The desktop engine's class, for its static calls; null in the browser. */
let Native: typeof NativeEngine | null = null;
let hostList: Promise<HostInfo[]> = Promise.resolve([]);
if (DESKTOP) {
  const { NativeEngine } = await import('./audio/native');
  Native = NativeEngine;
  native = new NativeEngine();
  engine = native;
  hostList = NativeEngine.hosts();
} else {
  const { AudioEngine } = await import('./audio/engine');
  engine = new AudioEngine();
}

// ── State ──────────────────────────────────────────────────
const storage = (() => {
  try {
    return window.localStorage;
  } catch {
    return undefined;
  }
})();
let settings = loadSettings(storage);

/**
 * The desktop app's settings file, once read: the desktop app keeps its settings there, not
 * in localStorage. Null in the browser, or if the file could not be read (then the desktop
 * app falls back to localStorage).
 */
let config: DesktopConfig | null = null;
/** Id of the platform's default audio host, which the settings file keys its entry by. */
let defaultHost = '';
/** Shown once the status line exists. */
let startupMessage: { text: string; isError: boolean } | null = null;
if (Native) {
  defaultHost = (await hostList.catch(() => [])).find((h) => h.isDefault)?.id ?? '';
  try {
    const loaded = await Native.loadConfig();
    configPath.textContent = loaded.path;
    if (loaded.status === 'new') {
      // First run: take over what the webview remembered, if anything.
      config = writeConfig(loaded.config, settings, defaultHost);
      void Native.saveConfig(config).catch(() => undefined);
    } else {
      config = loaded.config;
      settings = readConfig(config, defaultHost);
    }
    if (loaded.status === 'broken') {
      startupMessage = {
        text: `The settings file could not be read, so Klirr starts with its defaults. The old file is kept as ${loaded.brokenPath ?? 'a copy'}.`,
        isError: true,
      };
    } else if (loaded.status === 'newer') {
      startupMessage = {
        text: 'These settings come from a newer Klirr. Klirr keeps a copy of them before it saves its own.',
        isError: false,
      };
    }
  } catch (err) {
    configPath.textContent = 'not available';
    startupMessage = {
      text: `Could not read the settings file (${err instanceof Error ? err.message : String(err)}). Settings are kept in the window's storage instead.`,
      isError: true,
    };
  }
}

// The custom IR itself is not stored, so a stored "custom" cabinet cannot be restored.
if (settings.params[P.Cabinet] === CABINET_CUSTOM)
  settings.params[P.Cabinet] = spec(P.Cabinet).default;

let saveTimer: ReturnType<typeof setTimeout> | undefined;
let saveFailed = false;
/** Writes the current settings into the settings file's copy (desktop). */
function updateConfig(): void {
  if (config) config = writeConfig(config, settings, defaultHost);
}
function persist(): void {
  updateConfig();
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    if (!config || !Native) {
      saveSettings(storage, settings);
      return;
    }
    Native.saveConfig(config).then(
      () => {
        saveFailed = false;
      },
      (err: unknown) => {
        // Once, not on every knob turn.
        if (!saveFailed) say(err instanceof Error ? err.message : String(err), true);
        saveFailed = true;
      },
    );
  }, 300);
}

function setParam(index: P, value: number): void {
  settings.params[index] = value;
  engine.setParam(index, value);
  persist();
}

let statusTimer: ReturnType<typeof setTimeout> | undefined;
function say(message: string, isError = false): void {
  footerStatus.textContent = message;
  footerStatus.classList.toggle('is-error', isError);
  clearTimeout(statusTimer);
  statusTimer = setTimeout(
    () => {
      footerStatus.textContent = '';
    },
    isError ? 10000 : 4000,
  );
}
if (startupMessage) say(startupMessage.text, startupMessage.isError);

// ── Meters ─────────────────────────────────────────────────
const inMeter = new Meter('IN', 'Input clipping: turn the trim or your interface gain down');
const outMeter = new Meter('OUT', 'Output at full scale');
const gateLed = el('span', 'led led--green');
gateLed.title = 'Noise gate open';
const limitLed = el('span', 'led led--amber');
limitLed.title = 'Safety limiter working: turn the master down';
outMeter.element.append(limitLed);
$('meters').append(inMeter.element, outMeter.element);

let limitUntil = 0;
engine.onMeters = (m) => {
  const now = performance.now();
  inMeter.push(m.inputPeak, now);
  outMeter.push(m.outputPeak, now);
  gateLed.classList.toggle('is-on', m.gateOpen);
  if (m.limiterDb > 0.5) limitUntil = now + 500;
};
function frame(now: number): void {
  inMeter.draw(now);
  outMeter.draw(now);
  limitLed.classList.toggle('is-on', now < limitUntil);
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);

// ── Channel and cabinet ────────────────────────────────────
const channelButtons = new Segmented('Amp channel', CHANNEL_NAMES, (i) => {
  setParam(P.Channel, i);
  say(`Channel: ${CHANNEL_NAMES[i] ?? ''}`);
});
channelButtons.select(settings.params[P.Channel] ?? 0);
$('channel').append(channelButtons.element);

const cabinetButtons = new Segmented('Cabinet', CABINET_NAMES, (i) => {
  setParam(P.Cabinet, i);
});
cabinetButtons.select(settings.params[P.Cabinet] ?? 1);
cabinetButtons.setEnabled(CABINET_CUSTOM, false, 'Load an impulse response first (Cabinet menu)');
$('cabinet').append(cabinetButtons.element);

// ── Knobs ──────────────────────────────────────────────────
type KnobDef = Omit<KnobOptions, 'spec' | 'onInput'> & { param: P };
interface BlockDef {
  label: string;
  knobs: KnobDef[];
  extra?: HTMLElement;
}

const blocks: BlockDef[] = [
  {
    label: 'Input',
    extra: gateLed,
    knobs: [
      { param: P.InputGainDb, label: 'Trim', format: fmt.db, bipolar: true },
      { param: P.GateThresholdDb, label: 'Gate', format: fmt.gate, color: 'red' },
    ],
  },
  {
    label: 'Amp',
    knobs: [
      { param: P.Gain, label: 'Gain', format: (v) => fmt.dial(v) },
      { param: P.MasterDb, label: 'Master', format: fmt.level },
    ],
  },
  {
    label: 'Tone',
    knobs: [
      { param: P.BassDb, label: 'Bass', format: fmt.db, bipolar: true },
      { param: P.MidDb, label: 'Mid', format: fmt.db, bipolar: true },
      { param: P.TrebleDb, label: 'Treble', format: fmt.db, bipolar: true },
    ],
  },
  {
    label: 'Delay',
    knobs: [
      { param: P.DelayTimeMs, label: 'Time', format: fmt.time, color: 'blue', taper: 'log' },
      { param: P.DelayFeedback, label: 'Feedback', format: (v) => fmt.dial(v), color: 'blue' },
      { param: P.DelayMix, label: 'Mix', format: (v) => fmt.dial(v), color: 'blue' },
    ],
  },
  {
    label: 'Reverb',
    knobs: [
      { param: P.ReverbSize, label: 'Size', format: (v) => fmt.dial(v), color: 'green' },
      { param: P.ReverbDamping, label: 'Damp', format: (v) => fmt.dial(v), color: 'green' },
      { param: P.ReverbMix, label: 'Mix', format: (v) => fmt.dial(v), color: 'green' },
    ],
  },
];

const knobPanel = $('knobs');
for (const block of blocks) {
  const section = el('div', 'knob-block');
  const label = el('div', 'block-label');
  label.textContent = block.label;
  if (block.extra) label.append(block.extra);
  const row = el('div', 'knobs-row');
  for (const { param, ...def } of block.knobs) {
    const knob = new Knob({
      ...def,
      spec: spec(param),
      onInput: (v) => {
        setParam(param, v);
      },
    });
    knob.setValue(settings.params[param] ?? spec(param).default);
    row.append(knob.element);
  }
  section.append(label, row);
  knobPanel.append(section);
}

// ── Bypass ─────────────────────────────────────────────────
function renderBypass(): void {
  const on = settings.params[P.Bypass] === 1;
  bypassBtn.classList.toggle('active', on);
  bypassBtn.setAttribute('aria-pressed', String(on));
  app.classList.toggle('is-bypassed', on);
}
bypassBtn.addEventListener('click', () => {
  const on = settings.params[P.Bypass] !== 1;
  setParam(P.Bypass, on ? 1 : 0);
  renderBypass();
  say(on ? 'Bypass on: dry signal' : 'Bypass off');
});
renderBypass();

// ── Devices ────────────────────────────────────────────────
/** The devices of the current host, as last listed. */
let inputs: DeviceInfo[] = [];

function fillSelect(
  select: HTMLSelectElement,
  devices: DeviceInfo[],
  selected: string,
  fallback: string,
): void {
  select.replaceChildren(new Option(fallback, ''));
  for (const d of devices) select.append(new Option(d.label, d.id));
  select.value = selected;
  if (select.value !== selected) select.value = '';
}

/** Offers the channels the selected input has (two where that is unknown). */
function fillChannels(): void {
  const id = inputSelect.value;
  const device = inputs.find((d) => (id ? d.id === id : d.isDefault));
  const count = Math.max(1, device?.channels ?? 2);
  const label = (i: number) =>
    count === 2 ? (i === 0 ? 'Input 1 (left)' : 'Input 2 (right)') : `Input ${String(i + 1)}`;
  channelSelect.replaceChildren(
    ...Array.from({ length: count }, (_, i) => new Option(label(i), String(i))),
  );
  channelSelect.value = String(settings.inputChannel);
  if (channelSelect.value !== String(settings.inputChannel)) channelSelect.value = '0';
}

async function refreshDevices(): Promise<void> {
  try {
    const devices = await engine.listDevices(hostSelect.value);
    inputs = devices.inputs;
    fillSelect(inputSelect, devices.inputs, settings.inputDeviceId, 'System default input');
    if (engine.canChooseOutput) {
      fillSelect(outputSelect, devices.outputs, settings.outputDeviceId, 'System default output');
    }
  } catch (err) {
    inputs = [];
    say(`Could not list audio devices: ${engine.describeError(err)}`, true);
  }
  fillChannels();
  updateNotice();
  void refreshBuffers();
}

/** Whether the chosen input channel is one the running input does not deliver. */
let channelSilent = false;

function updateNotice(): void {
  const notes: string[] = [];
  if (!engine.canChooseOutput) {
    notes.push(
      'This browser always plays on the system default output. To choose the output device, use Chrome or Edge.',
    );
  }
  const channels = engine.inputChannelCount();
  channelSilent = engine.running && settings.inputChannel >= channels;
  if (channelSilent) {
    notes.push(
      `This input delivers ${channels === 1 ? 'one channel' : `${String(channels)} channels`} only, so input ${String(settings.inputChannel + 1)} is silent. Choose input 1.`,
    );
  }
  if (engine.labelsNeedPermission && !engine.running && inputSelect.options.length <= 1) {
    notes.push('Device names appear once you engage and allow audio input.');
  }
  if (!engine.labelsNeedPermission && inputSelect.options.length <= 1) {
    notes.push(
      'This driver lists no input devices. Connect your interface or choose another driver.',
    );
  }
  deviceNotice.textContent = notes.join(' ');
  deviceNotice.hidden = notes.length === 0;
  updateDriverLine();
  if (!native) describeWebRate();
}

/** Shows why audio could not start, in the audio settings, and opens them. */
function showAudioError(message: string): void {
  audioError.textContent = message;
  audioError.hidden = false;
  if (!audioPanel.matches(':popover-open')) audioPanel.showPopover();
}
function clearAudioError(): void {
  audioError.hidden = true;
}

/**
 * What to start with. The desktop app opens what the menus show: a remembered device or
 * driver that is not there now would only fail to open. The browser opens the remembered
 * device, since it lists names only once audio input was allowed.
 */
function startOptions(): StartOptions {
  const options = { ...settings, params: [...settings.params] };
  options.inputChannel = Number(channelSelect.value);
  if (DESKTOP) {
    options.host = hostSelect.value;
    options.inputDeviceId = inputSelect.value;
    options.outputDeviceId = outputSelect.value;
  }
  options.rate = requestedRate();
  return options;
}

/**
 * The sample rate to ask for: the chosen one, where the driver lets Klirr choose it (asking
 * an ASIO driver would change the interface's rate for every program).
 */
function requestedRate(): number | null {
  if (!native || settings.rate === null) return null;
  return bufferChoice && bufferChoice.rates.length === 0 ? null : settings.rate;
}

if (!engine.canChooseOutput) {
  outputSelect.replaceChildren(new Option('System default output', ''));
  outputSelect.disabled = true;
}
fillChannels();

for (const select of [hostSelect, inputSelect, channelSelect, outputSelect]) {
  select.addEventListener('change', clearAudioError);
}
inputSelect.addEventListener('change', () => {
  settings.inputDeviceId = inputSelect.value;
  fillChannels();
  settings.inputChannel = Number(channelSelect.value);
  persist();
  forgetMeasurement();
  engine
    .setInputDevice(inputSelect.value, settings.inputChannel)
    .then(updateNotice)
    .catch((err: unknown) => {
      say(engine.describeError(err), true);
    });
});
channelSelect.addEventListener('change', () => {
  settings.inputChannel = Number(channelSelect.value);
  persist();
  forgetMeasurement();
  engine
    .setInputChannel(settings.inputChannel)
    .then(updateNotice)
    .catch((err: unknown) => {
      say(engine.describeError(err), true);
    });
  updateNotice();
});
outputSelect.addEventListener('change', () => {
  settings.outputDeviceId = outputSelect.value;
  // The buffer belongs to the output: bring back the one chosen for it.
  if (config) recallBuffer(config, settings, defaultHost);
  persist();
  forgetMeasurement();
  void refreshBuffers();
  const switched = native
    ? native.reconfigure({
        outputDeviceId: outputSelect.value,
        buffer: settings.buffer,
        rate: settings.rate,
      })
    : engine.setOutputDevice(outputSelect.value);
  switched.then(reportBuffer).catch((err: unknown) => {
    say(`Could not switch output: ${engine.describeError(err)}`, true);
  });
});
engine.watchDevices(() => {
  void refreshDevices();
});

// The audio host (driver API): desktop only.
/** The hosts as listed, for what their Hardware Setup opens. */
let hosts: HostInfo[] = [];
async function fillHosts(): Promise<void> {
  try {
    hosts = await hostList;
    hostSelect.replaceChildren(...hosts.map((h) => new Option(h.name, h.isDefault ? '' : h.id)));
  } catch (err) {
    say(`Could not list audio drivers: ${engine.describeError(err)}`, true);
  }
  hostSelect.value = settings.host;
  if (hostSelect.value !== settings.host) {
    // The remembered driver is not there (another build or machine): use the default, with
    // what was chosen on it.
    hostSelect.value = '';
    settings.host = '';
    if (config) recallHost(config, settings, defaultHost);
  }
  describeSetup();
}
hostSelect.addEventListener('change', () => {
  // Each driver keeps its own devices and buffer: the old one's are in the file already.
  updateConfig();
  settings.host = hostSelect.value;
  if (config) recallHost(config, settings, defaultHost);
  persist();
  forgetMeasurement();
  describeSetup();
  void refreshDevices().then(() => {
    if (!native) return;
    const { host, inputDeviceId, outputDeviceId, inputChannel, buffer, rate } = startOptions();
    native
      .reconfigure({ host, inputDeviceId, outputDeviceId, inputChannel, buffer, rate })
      .then(() => {
        updateNotice();
        reportBuffer();
      })
      .catch((err: unknown) => {
        say(engine.describeError(err), true);
      });
  });
});

// ── Hardware setup (desktop only) ──────────────────────────
// Opens where the settings for every program live (the ASIO driver's control panel, the
// system's sound settings). Klirr itself only asks for what applies to it while it runs.
function setupTarget(): string | null {
  const host = hosts.find((h) => (hostSelect.value ? h.id === hostSelect.value : h.isDefault));
  return host?.hardwareSetup ?? null;
}

function describeSetup(): void {
  const target = setupTarget();
  setupButton.disabled = target === null;
  setupDetail.textContent = target
    ? `Opens ${target}, where the settings for every program live: Klirr leaves them as they are.`
    : 'This driver has no settings window to open.';
}

setupButton.addEventListener('click', () => {
  if (!Native) return;
  setupButton.disabled = true;
  Native.hardwareSetup(hostSelect.value, outputSelect.value)
    .then(() => refreshBuffers())
    .catch((err: unknown) => {
      say(engine.describeError(err), true);
    })
    .finally(() => {
      setupButton.disabled = setupTarget() === null;
    });
});

// ── Sample rate and buffer (desktop only) ──────────────────
/** What the current driver and devices allow, as last asked; null if unknown. */
let bufferChoice: BufferChoice | null = null;
/** Counts the questions, so only the latest answer shows. */
let bufferQuery = 0;

const frameMs = (frames: number, rate: number) => `${((frames / rate) * 1000).toFixed(1)} ms`;
const framesText = (frames: number, rate: number) =>
  `${String(frames)} frames · ${frameMs(frames, rate)}`;

/** Asks the driver what it allows on the devices the menus show, and fills the menu. */
async function refreshBuffers(): Promise<void> {
  if (!Native) return;
  const query = ++bufferQuery;
  let choice: BufferChoice | null;
  let error = '';
  try {
    choice = await Native.bufferChoice(
      hostSelect.value,
      inputSelect.value,
      outputSelect.value,
      settings.rate,
    );
  } catch (err) {
    choice = null;
    error = engine.describeError(err);
  }
  if (query !== bufferQuery) return;
  bufferChoice = choice;
  fillRates();
  fillBuffers(error);
}

const kHz = (rate: number) => `${String(rate / 1000)} kHz`;

/** The sample rate menu: choosable where the rate is Klirr's alone (WASAPI exclusive). */
function fillRates(): void {
  const c = bufferChoice;
  const running = native?.runningBuffer() ?? null;
  if (!c || c.rates.length === 0) {
    const rate = running?.sampleRate ?? c?.sampleRate;
    rateSelect.replaceChildren(new Option(rate ? kHz(rate) : 'Not available', ''));
    rateSelect.disabled = true;
    const who = c?.setBy === 'the Windows audio engine' ? 'Windows' : 'the driver';
    rateDetail.textContent = c
      ? `Set by ${who} for every program${setupTarget() ? ', in Hardware Setup' : ''}. Klirr runs at whatever it is set to.`
      : '';
    return;
  }
  const chosen = settings.rate !== null && c.rates.includes(settings.rate) ? settings.rate : null;
  rateSelect.replaceChildren(
    new Option('Device default', ''),
    ...c.rates.map((r) => new Option(kHz(r), String(r))),
  );
  rateSelect.disabled = false;
  rateSelect.value = chosen === null ? '' : String(chosen);
  const now = running ? `Running at ${kHz(running.sampleRate)}. ` : '';
  rateDetail.textContent = `${now}The device runs at this rate while Klirr holds it, and goes back to its own when Klirr lets go.`;
}

rateSelect.addEventListener('change', () => {
  settings.rate = rateSelect.value === '' ? null : Number(rateSelect.value);
  persist();
  forgetMeasurement();
  // The buffer sizes are counted at the rate.
  void refreshBuffers();
  if (!native) return;
  native
    .reconfigure({ rate: settings.rate })
    .then(() => {
      reportBuffer();
      fillRates();
    })
    .catch((err: unknown) => {
      say(engine.describeError(err), true);
    });
});

/** The browser's rate: the system's, shown only. */
function describeWebRate(): void {
  const rate = engine.sampleRate();
  rateSelect.replaceChildren(new Option(rate ? kHz(rate) : 'Set by the system', ''));
  rateDetail.textContent = rate
    ? 'The browser runs at the rate the system sets for the output device.'
    : 'Engage to see the rate the system runs the output at.';
}

function fillBuffers(error = ''): void {
  const c = bufferChoice;
  if (!c) {
    bufferSelect.replaceChildren(new Option('Not available', ''));
    bufferSelect.disabled = true;
    describeBuffer(error ? `The driver did not say which sizes it allows: ${error}` : '');
    return;
  }
  if (c.sizes.length === 0) {
    const running = native?.runningBuffer()?.frames ?? null;
    const frames = running ?? c.default;
    const label =
      frames === null ? `Set by ${c.setBy ?? 'the driver'}` : framesText(frames, c.sampleRate);
    bufferSelect.replaceChildren(new Option(label, ''));
    bufferSelect.disabled = true;
    describeBuffer();
    return;
  }
  // The default is the first entry already (and a stored size equal to it shows as it).
  const sizes = c.sizes.filter((n) => n !== c.default);
  const chosen = settings.buffer === c.default ? null : settings.buffer;
  // A size chosen before (on another rate, or a driver update since) stays selectable.
  if (chosen !== null && !sizes.includes(chosen)) {
    sizes.push(chosen);
    sizes.sort((a, b) => a - b);
  }
  const auto =
    c.default === null
      ? 'Driver default (its own setting)'
      : `Default · ${framesText(c.default, c.sampleRate)}`;
  bufferSelect.replaceChildren(
    new Option(auto, ''),
    ...sizes.map((n) => new Option(framesText(n, c.sampleRate), String(n))),
  );
  bufferSelect.disabled = false;
  bufferSelect.value = chosen === null ? '' : String(chosen);
  describeBuffer();
}

/** The line under the menu: who sets the size, and what runs. */
function describeBuffer(extra = ''): void {
  const lines: string[] = [];
  const c = bufferChoice;
  if (c && c.sizes.length === 0) {
    const where = setupTarget() ? ' Change it in Hardware Setup.' : '';
    lines.push(
      `On this driver ${c.setBy ?? 'the driver'} sets the buffer, for every program, so Klirr leaves it alone.${where}`,
    );
  } else if (c) {
    // ASIO drivers run one size for everyone; some keep the last size a program asked for.
    const asio = hostSelect.value.toLowerCase() === 'asio';
    lines.push(
      asio
        ? 'Klirr asks the driver for this size when it opens it. Most drivers go back to their own size when Klirr closes; some keep the last size a program asked for. If you hear dropouts, go one size up.'
        : 'Klirr asks for this size when it opens the devices; it applies to Klirr only. If you hear dropouts, go one size up.',
    );
  }

  const running = native?.runningBuffer() ?? null;
  if (running?.frames != null) {
    const now = `Running at ${framesText(running.frames, running.sampleRate)}.`;
    if (running.refused !== null && (running.requested !== null || running.requestedRate !== null))
      lines.push(
        `${now} The driver would not run as asked, so Klirr opened it with its own setup.`,
      );
    else if (running.requested !== null && running.requested !== running.frames)
      lines.push(`${now} The driver rounded the ${String(running.requested)} frames asked for.`);
    else lines.push(now);
  } else if (!engine.running) {
    lines.push('Takes effect when you engage.');
  }
  if (extra) lines.push(extra);
  bufferDetail.textContent = lines.join(' ');
}

/** After a (re)start: says so if the driver refused the size asked for. */
function reportBuffer(): void {
  describeBuffer();
  const running = native?.runningBuffer();
  if (running?.refused) {
    say(`The driver would not run as asked, so Klirr uses its own setup. ${running.refused}`, true);
  }
}

bufferSelect.addEventListener('change', () => {
  settings.buffer = bufferSelect.value === '' ? null : Number(bufferSelect.value);
  persist();
  forgetMeasurement();
  describeBuffer();
  if (!native) return;
  const wasRunning = engine.running;
  native
    .reconfigure({ buffer: settings.buffer })
    .then(() => {
      reportBuffer();
      const running = runningSetup().join(', ');
      if (wasRunning && running) say(`Restarted at ${running}`);
    })
    .catch((err: unknown) => {
      say(engine.describeError(err), true);
    });
});
audioPanel.addEventListener('toggle', () => {
  if (audioPanel.matches(':popover-open')) {
    if (native) {
      describeBuffer();
      fillRates();
    } else {
      describeWebRate();
    }
    updateLatency();
  } else {
    setTestTone(false);
  }
});

// ── Start / stop ───────────────────────────────────────────
let engineState: EngineState = 'stopped';
engine.onStateChange = (state) => {
  engineState = state;
  power.classList.toggle('is-online', state === 'running');
  power.classList.toggle('is-starting', state === 'starting');
  statusText.textContent =
    state === 'running' ? 'ACTIVE' : state === 'starting' ? 'WARMING UP' : 'OFFLINE';
  tube?.classList.toggle('is-on', state === 'running');
  startBtn.disabled = state !== 'stopped';
  stopBtn.disabled = state === 'stopped';
  if (state === 'stopped') {
    inMeter.reset();
    outMeter.reset();
    gateLed.classList.remove('is-on');
  }
  if (state !== 'running') measurement = null;
  dropouts = engine.dropouts();
  latMeasureBtn.disabled = state !== 'running' || measuring;
  toneOn = false;
  renderTone();
  updateLatency();
  if (native) {
    describeBuffer();
    fillRates();
  } else {
    describeWebRate();
  }
};
if (native) {
  native.onRestarted = () => {
    measurement = null;
    reportBuffer();
    fillRates();
    void refreshBuffers();
    updateNotice();
    updateLatency();
    const running = runningSetup().join(', ');
    say(
      running
        ? `The driver asked for a restart: now running at ${running}.`
        : 'The driver asked for a restart.',
    );
  };
}
engine.onDeviceLost = (message) => {
  say(message, true);
};

/**
 * Starts the audio. Call it from a click: the browser only lets audio start in one (the
 * context is created before the first await). Resolves once audio runs, or once it failed
 * (the error is shown in the audio settings).
 */
function engage(): Promise<void> {
  say('Starting…');
  return engine
    .start(startOptions())
    .then(async () => {
      if (!engine.running) return;
      clearAudioError();
      say('Ready. Play!');
      reportBuffer();
      await refreshDevices();
    })
    .catch((err: unknown) => {
      const message = engine.describeError(err);
      say(message, true);
      showAudioError(message);
    });
}
startBtn.addEventListener('click', () => {
  void engage();
});
stopBtn.addEventListener('click', () => {
  void engine.stop().then(() => {
    say('Stopped');
    updateNotice();
  });
});

// ── Test tone ──────────────────────────────────────────────
let toneOn = false;
function renderTone(): void {
  const starting = engineState === 'starting';
  toneButton.disabled = starting;
  toneButton.textContent = starting ? '…' : toneOn ? 'ON' : 'OFF';
  toneButton.setAttribute('aria-pressed', String(toneOn));
  toneButton.title = engineState === 'stopped' ? 'Starts the audio, then the tone' : '';
}
function setTestTone(on: boolean): void {
  if (on === toneOn) return;
  toneOn = on && engine.running;
  engine.setTestTone(toneOn);
  renderTone();
}
toneButton.addEventListener('click', () => {
  if (engineState !== 'stopped') {
    setTestTone(!toneOn);
    return;
  }
  // Audio is off: start it, then the tone. Audio keeps running when the tone stops.
  void engage().then(() => {
    if (engine.running && audioPanel.matches(':popover-open')) setTestTone(true);
  });
});
renderTone();

// ── Latency ────────────────────────────────────────────────
/** The last loopback measurement; void once the devices change or audio stops. */
let measurement: (Measurement & { ok: true }) | null = null;
let measuring = false;
/** Dropouts since Engage, sampled once a second; null when unknown. */
let dropouts: { count: number; addedMs: number } | null = null;
/** The dropout count when the last measurement was taken. */
let dropoutsAtMeasurement = 0;
/** The estimate when the last measurement was taken, to tell what it misses. */
let estimateAtMeasurement: LatencyEstimate | null = null;

const ms = (v: number) => `${v.toFixed(1)} ms`;

function showEstimate(e: LatencyEstimate | null): void {
  latInput.textContent = !e ? '—' : e.inputMs === null ? 'not reported' : ms(e.inputMs);
  latOutput.textContent = e ? ms(e.outputMs) : '—';
  latEngine.textContent = e ? ms(e.engineMs) : '—';
  latTotal.textContent = e ? `≈ ${ms(e.totalMs)}` : '—';
}

/** The latency rows of the audio settings: the same figures, and the measurement. */
function showAudioLatency(e: LatencyEstimate | null): void {
  audioLatInput.textContent = !e ? '—' : e.inputMs === null ? 'not reported' : ms(e.inputMs);
  audioLatOutput.textContent = e ? ms(e.outputMs) : '—';
  audioLatEngine.textContent = e ? ms(e.engineMs) : '—';
  audioLatTotal.textContent = e ? `≈ ${ms(e.totalMs)}` : '—';
  audioLatMeasured.textContent = measurement ? ms(measurement.totalMs) : '—';
  audioLatDetail.textContent = engine.running
    ? `As ${engine.estimateSource} reports it; the measurement takes a loopback cable.`
    : 'The figures appear once audio runs.';
  // While audio is off, the reason the rows are empty comes with the way to fill them.
  audioLatEngageRow.hidden = engineState === 'running';
  audioLatEngage.disabled = engineState !== 'stopped';
  audioLatEngage.textContent = engineState === 'starting' ? 'WARMING UP…' : '▶ ENGAGE';
}
// The same as the Engage button in the header (errors show in this panel too).
audioLatEngage.addEventListener('click', () => {
  startBtn.click();
});

/**
 * What the estimate missed, by the measurement: the part of the latency the driver does not
 * report (converters, its own safety buffers). Empty when it cannot tell.
 */
function describeUnreported(): string {
  const e = estimateAtMeasurement;
  if (!measurement || !e || e.inputMs === null) return '';
  const missing = measurement.totalMs - e.totalMs;
  if (missing < 0.5)
    return ` ${sentenceCase(engine.estimateSource)} reports all of it (within 0.5 ms).`;
  return ` ${sentenceCase(engine.estimateSource)} reports ${ms(e.totalMs)} of it; ${ms(missing)} is not reported (converters and buffers it does not count).`;
}
const sentenceCase = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

const plural = (n: number, word: string) => `${String(n)} ${word}${n === 1 ? '' : 's'}`;

function describeDropouts(): string {
  if (!engine.running) return 'Dropouts are counted while audio runs.';
  if (!dropouts || dropouts.count === 0) return 'No dropouts since Engage.';
  return `${plural(dropouts.count, 'dropout')} since Engage, adding about ${ms(dropouts.addedMs)} of latency.`;
}

function updateLatency(): void {
  const e = engine.latencyEstimate();
  const dropoutCount = engine.running ? (dropouts?.count ?? 0) : 0;
  latencyButton.classList.toggle('has-dropouts', dropoutCount > 0);
  updateDriverLine();
  // The panels' figures only matter while they are open.
  if (audioPanel.matches(':popover-open')) showAudioLatency(e);
  if (!latencyPanel.matches(':popover-open')) return;
  showEstimate(e);
  latDropouts.textContent = describeDropouts();
  if (measurement) {
    latMeasured.textContent = ms(measurement.totalMs);
    latMeasuredDetail.textContent = `Round trip ${ms(measurement.roundTripMs)} + engine, ${String(measurement.hits)} clicks agreed, ${plural(dropoutsAtMeasurement, 'dropout')} before it.${describeUnreported()}`;
  } else if (!measuring) {
    latMeasured.textContent = '—';
    if (!engine.running) latMeasuredDetail.textContent = 'Engage, then measure.';
  }
}

// ── Driver line ────────────────────────────────────────────
// Under the power state and the meters: what audio runs on, at what sample rate and buffer
// size; while stopped, what Engage will open. Latency figures are in the latency panel only.
function driverName(): string {
  if (!DESKTOP) return 'Browser audio';
  return hostSelect.selectedOptions[0]?.text ?? 'Default driver';
}

/** Sample rate and buffer size while running, e.g. ["48 kHz", "256 samples"]. */
function runningSetup(): string[] {
  const parts: string[] = [];
  const rate = engine.sampleRate();
  if (rate) parts.push(`${String(rate / 1000)} kHz`);
  const b = engine.bufferFrames();
  const input = b?.input ?? b?.output;
  const output = b?.output ?? b?.input;
  if (input != null && output != null) {
    parts.push(
      input === output
        ? `${String(input)} samples`
        : `${String(input)}/${String(output)} samples in/out`,
    );
  }
  return parts;
}

function updateDriverLine(): void {
  const parts = [driverName()];
  let warning = false;
  if (engine.running) {
    parts.push(...runningSetup());
    const count = dropouts?.count ?? 0;
    if (count > 0) parts.push(plural(count, 'dropout'));
    if (channelSilent) parts.push(`input ${String(settings.inputChannel + 1)} silent`);
    warning = count > 0 || channelSilent;
  } else if (engineState === 'starting') {
    parts.push('opening…');
  } else {
    const input = inputSelect.selectedOptions[0]?.text;
    if (input) parts.push(input);
  }
  driverLine.textContent = parts.join(' · ');
  driverLine.classList.toggle('is-idle', engineState === 'stopped');
  driverLine.classList.toggle('is-warning', warning);
}

// Polling reads a few properties once a second: no cost to the audio.
setInterval(() => {
  if (!engine.running) return;
  dropouts = engine.dropouts();
  updateLatency();
}, 1000);
latencyPanel.addEventListener('toggle', updateLatency);

/** Invalidates the measurement when the path it measured changes. */
function forgetMeasurement(): void {
  if (!measurement) return;
  measurement = null;
  latMeasuredDetail.textContent = 'The devices changed: measure again.';
  updateLatency();
}

const MEASURE_ERRORS: Record<Exclude<Measurement, { ok: true }>['reason'], string> = {
  'no-signal':
    'No click came back. Is the cable going from the output to the selected input, and is the interface gain up?',
  noisy: 'The input is not quiet. Mute your strings and turn noisy pedals off, then try again.',
  inconsistent:
    'The clicks came back at different times. Something else may be playing into the input: try again.',
  stopped: 'Audio stopped during the measurement.',
};

latMeasureBtn.addEventListener('click', () => {
  // The clicks replace the amp; the tone would cover them.
  setTestTone(false);
  estimateAtMeasurement = engine.latencyEstimate();
  measuring = true;
  latMeasureBtn.disabled = true;
  latMeasured.textContent = '…';
  latMeasuredDetail.textContent = 'Measuring: you will hear a few clicks.';
  engine
    .measureLatency()
    .then((m) => {
      measurement = m.ok ? m : null;
      dropoutsAtMeasurement = dropouts?.count ?? 0;
      if (!m.ok) {
        latMeasured.textContent = '—';
        latMeasuredDetail.textContent = MEASURE_ERRORS[m.reason];
      }
    })
    .catch((err: unknown) => {
      latMeasuredDetail.textContent = err instanceof Error ? err.message : String(err);
    })
    .finally(() => {
      measuring = false;
      latMeasureBtn.disabled = !engine.running;
      updateLatency();
    });
});

// ── Impulse responses ──────────────────────────────────────
async function loadIr(file: File): Promise<void> {
  try {
    await engine.loadIr(file.name, await file.arrayBuffer());
  } catch (err) {
    say(`Could not load ${file.name}: ${err instanceof Error ? err.message : String(err)}`, true);
    return;
  }
  irName.textContent = file.name;
  irName.title = file.name;
  irName.classList.add('is-loaded');
  cabinetButtons.setEnabled(CABINET_CUSTOM, true, file.name);
  cabinetButtons.select(CABINET_CUSTOM);
  setParam(P.Cabinet, CABINET_CUSTOM);
  say(`Cabinet IR loaded: ${file.name}`);
}

irButton.addEventListener('click', () => {
  irFile.click();
});
irFile.addEventListener('change', () => {
  const file = irFile.files?.[0];
  irFile.value = '';
  if (file) void loadIr(file);
});

let dragDepth = 0;
const hasFiles = (e: DragEvent) => e.dataTransfer?.types.includes('Files') ?? false;
window.addEventListener('dragenter', (e) => {
  if (!hasFiles(e)) return;
  dragDepth++;
  dropOverlay.hidden = false;
});
window.addEventListener('dragleave', (e) => {
  if (!hasFiles(e)) return;
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) dropOverlay.hidden = true;
});
window.addEventListener('dragover', (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
});
window.addEventListener('drop', (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  dragDepth = 0;
  dropOverlay.hidden = true;
  const file = e.dataTransfer?.files[0];
  if (file) void loadIr(file);
});

if (DESKTOP) {
  // The amp is built: size the window to it and show it, before the drivers are listed
  // (which can take a moment with ASIO).
  const { fitWindow } = await import('./window');
  fitWindow(app).catch((err: unknown) => {
    say(`Could not size the window: ${err instanceof Error ? err.message : String(err)}`, true);
  });
  await fillHosts();
}
void refreshDevices();
