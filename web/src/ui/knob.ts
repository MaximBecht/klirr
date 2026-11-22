// A rotary knob: drag vertically, scroll, or use the keyboard. Double-click resets.

import type { ParamSpec } from '../params';
import { fromPosition, toPosition, type Taper } from './taper';

export type KnobColor = 'amber' | 'blue' | 'green' | 'red';

export interface KnobOptions {
  spec: ParamSpec;
  label: string;
  format: (value: number) => string;
  color?: KnobColor;
  taper?: Taper;
  /** Bipolar knobs draw their arc from the centre (tone controls, trim). */
  bipolar?: boolean;
  onInput: (value: number) => void;
}

/** Rotation range: 270° from −135° to +135°. */
const SWEEP_DEG = 270;
const DRAG_PX_FULL_RANGE = 200;
const FINE_FACTOR = 5;

export class Knob {
  readonly element: HTMLElement;
  private readonly opts: KnobOptions;
  private readonly control: HTMLElement;
  private readonly cap: HTMLElement;
  private readonly readout: HTMLElement;
  private position = 0;
  private value: number;

  constructor(opts: KnobOptions) {
    this.opts = opts;
    this.value = opts.spec.default;

    this.element = el('div', `knob-wrap knob-wrap--${opts.color ?? 'amber'}`);
    this.control = el('div', 'knob');
    this.control.tabIndex = 0;
    this.control.setAttribute('role', 'slider');
    this.control.setAttribute('aria-label', opts.label);
    this.control.title = 'Drag or scroll to adjust · Shift for fine · Double-click to reset';
    const ring = el('div', 'knob-ring');
    this.cap = el('div', 'knob-cap');
    this.cap.append(el('div', 'knob-mark'));
    this.control.append(ring, this.cap);
    const label = el('span', 'knob-label');
    label.textContent = opts.label;
    this.readout = el('span', 'knob-value');
    this.element.append(this.control, label, this.readout);

    this.bindPointer();
    this.bindWheel();
    this.bindKeys();
    this.control.addEventListener('dblclick', () => {
      this.input(opts.spec.default);
    });
    this.setValue(this.value);
  }

  get current(): number {
    return this.value;
  }

  /** Sets the value without notifying (e.g. when loading settings). */
  setValue(value: number): void {
    const { spec, taper } = this.opts;
    this.position = toPosition(spec, value, taper);
    this.value = fromPosition(spec, this.position, taper);
    if (!spec.stepped && Math.abs(this.value - value) < 1e-6) this.value = value;
    this.render();
  }

  private input(value: number): void {
    const before = this.value;
    this.setValue(value);
    if (this.value !== before) this.opts.onInput(this.value);
  }

  private moveTo(position: number): void {
    const p = Math.min(1, Math.max(0, position));
    this.input(fromPosition(this.opts.spec, p, this.opts.taper));
    this.position = p; // keep sub-step positions while dragging
  }

  private render(): void {
    const deg = -SWEEP_DEG / 2 + this.position * SWEEP_DEG;
    this.cap.style.transform = `rotate(${deg}deg)`;
    const from = this.opts.bipolar ? Math.min(this.position, 0.5) : 0;
    const to = this.opts.bipolar ? Math.max(this.position, 0.5) : this.position;
    this.control.style.setProperty('--arc-from', `${from * SWEEP_DEG}deg`);
    this.control.style.setProperty('--arc-to', `${to * SWEEP_DEG}deg`);
    const text = this.opts.format(this.value);
    this.readout.textContent = text;
    this.control.setAttribute('aria-valuemin', String(this.opts.spec.min));
    this.control.setAttribute('aria-valuemax', String(this.opts.spec.max));
    this.control.setAttribute('aria-valuenow', String(this.value));
    this.control.setAttribute('aria-valuetext', text);
  }

  private bindPointer(): void {
    let startY = 0;
    let startPos = 0;
    let fine = false;
    this.control.addEventListener('pointerdown', (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      this.control.focus();
      this.control.setPointerCapture(e.pointerId);
      this.control.classList.add('is-dragging');
      startY = e.clientY;
      startPos = this.position;
      fine = e.shiftKey;
    });
    this.control.addEventListener('pointermove', (e) => {
      if (!this.control.hasPointerCapture(e.pointerId)) return;
      if (e.shiftKey !== fine) {
        // Re-anchor when Shift changes mid-drag, so the knob does not jump.
        fine = e.shiftKey;
        startY = e.clientY;
        startPos = this.position;
      }
      const scale = DRAG_PX_FULL_RANGE * (fine ? FINE_FACTOR : 1);
      this.moveTo(startPos + (startY - e.clientY) / scale);
    });
    const end = (e: PointerEvent) => {
      if (this.control.hasPointerCapture(e.pointerId)) {
        this.control.releasePointerCapture(e.pointerId);
      }
      this.control.classList.remove('is-dragging');
    };
    this.control.addEventListener('pointerup', end);
    this.control.addEventListener('pointercancel', end);
  }

  private bindWheel(): void {
    this.control.addEventListener(
      'wheel',
      (e) => {
        e.preventDefault();
        if (e.deltaY === 0) return;
        this.step(-Math.sign(e.deltaY), e.shiftKey ? 0.005 : 0.02);
      },
      { passive: false },
    );
  }

  private bindKeys(): void {
    this.control.addEventListener('keydown', (e) => {
      const small = e.shiftKey ? 0.002 : 0.01;
      switch (e.key) {
        case 'ArrowUp':
        case 'ArrowRight':
          this.step(1, small);
          break;
        case 'ArrowDown':
        case 'ArrowLeft':
          this.step(-1, small);
          break;
        case 'PageUp':
          this.step(1, 0.1);
          break;
        case 'PageDown':
          this.step(-1, 0.1);
          break;
        case 'Home':
          this.moveTo(0);
          break;
        case 'End':
          this.moveTo(1);
          break;
        case 'Delete':
        case 'Backspace':
          this.input(this.opts.spec.default);
          break;
        default:
          return;
      }
      e.preventDefault();
    });
  }

  private step(direction: number, amount: number): void {
    const { spec } = this.opts;
    if (spec.stepped) {
      this.input(this.value + direction);
      return;
    }
    this.moveTo(this.position + direction * amount);
  }
}

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className = '',
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (className) e.className = className;
  return e;
}
