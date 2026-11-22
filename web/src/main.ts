import '@fontsource/bebas-neue';
import '@fontsource/share-tech-mono';
import '@fontsource/barlow/300.css';
import '@fontsource/barlow/400.css';
import '@fontsource/barlow/600.css';
import './style.css';

import { AudioEngine, describeStartError, outputSelectionSupported } from './audio/engine';
import { CABINET_CUSTOM, CABINET_NAMES, CHANNEL_NAMES, P, spec } from './params';
import { loadSettings, saveSettings } from './settings';
import * as fmt from './ui/format';
import { Knob, el, type KnobOptions } from './ui/knob';
import { Meter } from './ui/meter';
import { Segmented } from './ui/segmented';

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
const inputSelect = $('input-device') as HTMLSelectElement;
const channelSelect = $('input-channel') as HTMLSelectElement;
const outputSelect = $('output-device') as HTMLSelectElement;
const startBtn = $('start') as HTMLButtonElement;
const stopBtn = $('stop') as HTMLButtonElement;
const bypassBtn = $('bypass') as HTMLButtonElement;
const deviceNotice = $('device-notice');
const footerStatus = $('footer-status');
const latencyText = $('latency');
const irButton = $('load-ir') as HTMLButtonElement;
const irFile = $('ir-file') as HTMLInputElement;
const irName = $('ir-name');
const dropOverlay = $('drop-overlay');

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

const engine = new AudioEngine();

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
cabinetButtons.setEnabled(CABINET_CUSTOM, false, 'Load an impulse response first');
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
function fillSelect(
  select: HTMLSelectElement,
  devices: MediaDeviceInfo[],
  selected: string,
  fallback: string,
): void {
  select.replaceChildren();
  const def = el('option');
  def.value = '';
  def.textContent = fallback;
  select.append(def);
  devices
    .filter((d) => d.deviceId && d.deviceId !== 'default' && d.deviceId !== 'communications')
    .forEach((d, i) => {
      const o = el('option');
      o.value = d.deviceId;
      o.textContent = d.label || `${fallback.replace('System default', 'Device')} ${i + 1}`;
      select.append(o);
    });
  select.value = selected;
  if (select.value !== selected) select.value = '';
}

async function refreshDevices(): Promise<void> {
  try {
    const { inputs, outputs } = await AudioEngine.listDevices();
    fillSelect(inputSelect, inputs, settings.inputDeviceId, 'System default input');
    if (outputSelectionSupported) {
      fillSelect(outputSelect, outputs, settings.outputDeviceId, 'System default output');
    }
  } catch {
    say('Could not list audio devices', true);
  }
  updateNotice();
}

function updateNotice(): void {
  const notes: string[] = [];
  if (!outputSelectionSupported) {
    notes.push(
      'This browser always plays on the system default output. To choose the output device, use Chrome or Edge.',
    );
  }
  if (engine.running && settings.inputChannel === 1 && engine.inputChannelCount() < 2) {
    notes.push('This input delivers one channel only, so input 2 is silent. Choose input 1.');
  }
  if (!engine.running && inputSelect.options.length <= 1) {
    notes.push('Device names appear once you engage and allow audio input.');
  }
  deviceNotice.textContent = notes.join(' ');
  deviceNotice.hidden = notes.length === 0;
}

if (!outputSelectionSupported) {
  outputSelect.replaceChildren(new Option('System default output', ''));
  outputSelect.disabled = true;
}
channelSelect.value = String(settings.inputChannel);

inputSelect.addEventListener('change', () => {
  settings.inputDeviceId = inputSelect.value;
  persist();
  engine
    .setInputDevice(inputSelect.value)
    .then(updateNotice)
    .catch((err: unknown) => {
      say(describeStartError(err), true);
    });
});
channelSelect.addEventListener('change', () => {
  settings.inputChannel = Number(channelSelect.value) === 1 ? 1 : 0;
  persist();
  engine.setInputChannel(settings.inputChannel);
  updateNotice();
});
outputSelect.addEventListener('change', () => {
  settings.outputDeviceId = outputSelect.value;
  persist();
  engine.setOutputDevice(outputSelect.value).catch((err: unknown) => {
    say(`Could not switch output: ${describeStartError(err)}`, true);
  });
});
navigator.mediaDevices.addEventListener('devicechange', () => {
  void refreshDevices();
});

// ── Start / stop ───────────────────────────────────────────
engine.onStateChange = (state) => {
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
    latencyText.textContent = '';
  }
};
engine.onInputEnded = () => {
  say('The input device stopped (unplugged?). Choose another input or engage again.', true);
};

startBtn.addEventListener('click', () => {
  say('Starting…');
  engine
    .start({ ...settings, params: [...settings.params] })
    .then(async () => {
      if (!engine.running) return;
      say('Ready. Play!');
      const ms = engine.latencyMs();
      latencyText.textContent = ms === null ? '' : `LATENCY ≈ ${Math.round(ms)} MS`;
      latencyText.title =
        'Estimated by the browser: input + output buffers + the engine. The real figure also depends on your interface driver.';
      await refreshDevices();
    })
    .catch((err: unknown) => {
      say(describeStartError(err), true);
    });
});
stopBtn.addEventListener('click', () => {
  void engine.stop().then(() => {
    say('Stopped');
    updateNotice();
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
  cabinetButtons.setEnabled(CABINET_CUSTOM, true);
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

void refreshDevices();
