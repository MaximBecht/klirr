// A row of mutually exclusive buttons (amp channel, cabinet), like the legacy mode buttons.

import { el } from './knob';

export class Segmented {
  readonly element: HTMLElement;
  private readonly buttons: HTMLButtonElement[];
  private selected = 0;

  constructor(label: string, options: readonly string[], onSelect: (index: number) => void) {
    this.element = el('div', 'mode-buttons');
    this.element.setAttribute('role', 'radiogroup');
    this.element.setAttribute('aria-label', label);
    this.buttons = options.map((text, i) => {
      const b = el('button', 'mode-btn');
      b.type = 'button';
      b.textContent = text;
      b.setAttribute('role', 'radio');
      b.addEventListener('click', () => {
        if (b.disabled || i === this.selected) return;
        this.select(i);
        onSelect(i);
      });
      b.addEventListener('keydown', (e) => {
        const dir =
          e.key === 'ArrowRight' || e.key === 'ArrowDown'
            ? 1
            : e.key === 'ArrowLeft' || e.key === 'ArrowUp'
              ? -1
              : 0;
        if (!dir) return;
        e.preventDefault();
        const next = this.nextEnabled(i, dir);
        if (next !== i) {
          this.select(next);
          this.buttons[next]?.focus();
          onSelect(next);
        }
      });
      return b;
    });
    this.element.append(...this.buttons);
    this.select(0);
  }

  get value(): number {
    return this.selected;
  }

  select(index: number): void {
    this.selected = index;
    this.buttons.forEach((b, i) => {
      const on = i === index;
      b.classList.toggle('active', on);
      b.setAttribute('aria-checked', String(on));
      b.tabIndex = on ? 0 : -1;
    });
  }

  setEnabled(index: number, enabled: boolean, title = ''): void {
    const b = this.buttons[index];
    if (!b) return;
    b.disabled = !enabled;
    b.title = title;
  }

  private nextEnabled(from: number, dir: number): number {
    const n = this.buttons.length;
    for (let k = 1; k < n; k++) {
      const i = (from + dir * k + n) % n;
      if (!this.buttons[i]?.disabled) return i;
    }
    return from;
  }
}
