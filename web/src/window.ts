// The desktop app's window: exactly as large as the amp. The window opens hidden (see
// `tauri.conf.json`); once the amp is laid out, it takes the amp's size, centres and shows.

import { LogicalSize } from '@tauri-apps/api/dpi';
import { getCurrentWindow } from '@tauri-apps/api/window';

/** Sizes the window to `content` and shows it; follows `content` if its size ever changes. */
export async function fitWindow(content: HTMLElement): Promise<void> {
  // The fonts change the amp's height; measure once they are in.
  await document.fonts.ready;
  const win = getCurrentWindow();
  let shown = false;
  const fit = async () => {
    const { width, height } = content.getBoundingClientRect();
    await win.setSize(new LogicalSize(Math.ceil(width), Math.ceil(height)));
    if (!shown) {
      shown = true;
      await win.center();
      await win.show();
    }
  };
  await fit();
  new ResizeObserver(() => {
    void fit();
  }).observe(content);
}
