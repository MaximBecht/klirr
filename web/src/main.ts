import '@fontsource/bebas-neue';
import '@fontsource/share-tech-mono';
import '@fontsource/barlow/300.css';
import '@fontsource/barlow/400.css';
import '@fontsource/barlow/600.css';
import './style.css';

import type { HostInfo, NativeEngine } from './audio/native';
import type {
  AmpEngine,
  DeviceInfo,
  EngineState,
  LatencyEstimate,
  Measurement,
  StartOptions,
} from './audio/types';
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
const audioRunning = $('audio-running');
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
let hostList: Promise<HostInfo[]> = Promise.resolve([]);
if (DESKTOP) {
  const { NativeEngine } = await import('./audio/native');
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
const settings = loadSettings(storage);
// The custom IR itself is not stored, so a stored "custom" cabinet cannot be restored.
if (settings.params[P.Cabinet] === CABINET_CUSTOM)
  settings.params[P.Cabinet] = spec(P.Cabinet).default;

let saveTimer: ReturnType<typeof setTimeout> | undefined;
function persist(): void {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveSettings(storage, settings);
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
  const setup = engine.running ? runningSetup() : [];
  audioRunning.textContent = setup.length > 0 ? `Running at ${setup.join(', ')}.` : '';
  updateDriverLine();
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
  return options;
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
  persist();
  forgetMeasurement();
  engine.setOutputDevice(outputSelect.value).catch((err: unknown) => {
    say(`Could not switch output: ${engine.describeError(err)}`, true);
  });
});
engine.watchDevices(() => {
  void refreshDevices();
});

// The audio host (driver API): desktop only.
async function fillHosts(): Promise<void> {
  try {
    const hosts = await hostList;
    hostSelect.replaceChildren(...hosts.map((h) => new Option(h.name, h.isDefault ? '' : h.id)));
  } catch (err) {
    say(`Could not list audio drivers: ${engine.describeError(err)}`, true);
  }
  hostSelect.value = settings.host;
  if (hostSelect.value !== settings.host) hostSelect.value = '';
}
hostSelect.addEventListener('change', () => {
  settings.host = hostSelect.value;
  persist();
  forgetMeasurement();
  void refreshDevices().then(() => {
    if (!native) return;
    const { host, inputDeviceId, outputDeviceId, inputChannel } = startOptions();
    native
      .reconfigure({ host, inputDeviceId, outputDeviceId, inputChannel })
      .then(updateNotice)
      .catch((err: unknown) => {
        say(engine.describeError(err), true);
      });
  });
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
  updateLatency();
};
engine.onDeviceLost = (message) => {
  say(message, true);
};

startBtn.addEventListener('click', () => {
  say('Starting…');
  engine
    .start(startOptions())
    .then(async () => {
      if (!engine.running) return;
      clearAudioError();
      say('Ready. Play!');
      await refreshDevices();
    })
    .catch((err: unknown) => {
      const message = engine.describeError(err);
      say(message, true);
      showAudioError(message);
    });
});
stopBtn.addEventListener('click', () => {
  void engine.stop().then(() => {
    say('Stopped');
    updateNotice();
  });
});

// ── Latency ────────────────────────────────────────────────
/** The last loopback measurement; void once the devices change or audio stops. */
let measurement: (Measurement & { ok: true }) | null = null;
let measuring = false;
/** Dropouts since Engage, sampled once a second; null when unknown. */
let dropouts: { count: number; addedMs: number } | null = null;
/** The dropout count when the last measurement was taken. */
let dropoutsAtMeasurement = 0;

const ms = (v: number) => `${v.toFixed(1)} ms`;

function showEstimate(e: LatencyEstimate | null): void {
  latInput.textContent = !e ? '—' : e.inputMs === null ? 'not reported' : ms(e.inputMs);
  latOutput.textContent = e ? ms(e.outputMs) : '—';
  latEngine.textContent = e ? ms(e.engineMs) : '—';
  latTotal.textContent = e ? `≈ ${ms(e.totalMs)}` : '—';
}

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
  // The panel's figures only matter while it is open.
  if (!latencyPanel.matches(':popover-open')) return;
  showEstimate(e);
  latDropouts.textContent = describeDropouts();
  if (measurement) {
    latMeasured.textContent = ms(measurement.totalMs);
    latMeasuredDetail.textContent = `Round trip ${ms(measurement.roundTripMs)} + engine, ${String(measurement.hits)} clicks agreed, ${plural(dropoutsAtMeasurement, 'dropout')} before it.`;
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
