// A horizontal peak meter with a falling bar, peak hold and a clip LED.

import { peakDb } from './format';
import { el } from './knob';

const FLOOR_DB = -60;
const FALL_DB_PER_S = 30;
const HOLD_MS = 1200;
const CLIP_HOLD_MS = 1500;
/** Peaks at or above this count as clipping (−0.1 dBFS). */
const CLIP_LINEAR = 0.989;

export class Meter {
  readonly element: HTMLElement;
  private readonly bar: HTMLElement;
  private readonly hold: HTMLElement;
  private readonly clip: HTMLElement;
  private shownDb = -Infinity;
  private holdDb = -Infinity;
  private holdUntil = 0;
  private clipUntil = 0;
  private last = 0;

  constructor(label: string, clipTitle: string) {
    this.element = el('span', 'meter');
    const name = el('span', 'meter-label');
    name.textContent = label;
    const track = el('span', 'meter-track');
    this.bar = el('span', 'meter-bar');
    this.hold = el('span', 'meter-hold');
    track.append(this.bar, this.hold);
    this.clip = el('span', 'led led--red meter-clip');
    this.clip.title = clipTitle;
    this.element.append(name, track, this.clip);
    this.element.setAttribute('role', 'meter');
    this.element.setAttribute('aria-label', `${label} level`);
  }

  /** Feeds a new linear peak (called at the meter rate). */
  push(linearPeak: number, now = performance.now()): void {
    const db = peakDb(linearPeak);
    if (db >= this.shownDb) this.shownDb = db;
    if (db >= this.holdDb || now > this.holdUntil) {
      this.holdDb = db;
      this.holdUntil = now + HOLD_MS;
    }
    if (linearPeak >= CLIP_LINEAR) this.clipUntil = now + CLIP_HOLD_MS;
  }

  /** Animates the bar (called every frame). */
  draw(now = performance.now()): void {
    const dt = this.last ? (now - this.last) / 1000 : 0;
    this.last = now;
    this.shownDb -= FALL_DB_PER_S * dt;
    if (now > this.holdUntil) this.holdDb -= FALL_DB_PER_S * dt;
    this.bar.style.transform = `scaleX(${fraction(this.shownDb)})`;
    this.hold.style.left = `${fraction(this.holdDb) * 100}%`;
    this.hold.style.opacity = this.holdDb > FLOOR_DB ? '1' : '0';
    this.clip.classList.toggle('is-on', now < this.clipUntil);
  }

  reset(): void {
    this.shownDb = this.holdDb = -Infinity;
    this.clipUntil = 0;
  }
}

function fraction(db: number): number {
  if (!(db > FLOOR_DB)) return 0;
  return Math.min(1, (db - FLOOR_DB) / -FLOOR_DB);
}
