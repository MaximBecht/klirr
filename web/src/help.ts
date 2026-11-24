// The Help panel: about (version, build, license, links) and bug reports. What a report
// holds comes from `main.ts` (the audio setup) and, in the desktop app, the native side (the
// system and its log); `report.ts` turns it into text.

import {
  BUILD,
  describeVersion,
  mailtoUrl,
  reportFileName,
  reportText,
  type NativeDiagnostics,
  type ReportInput,
} from './report';

export interface HelpHooks {
  platform: 'web' | 'desktop';
  /** The native side's part of a report; null in the browser. */
  diagnostics: () => Promise<NativeDiagnostics | null>;
  /** The audio setup, as label → value lines. */
  audio: () => [string, string][];
  params: () => number[];
  /** What the UI told the user recently. */
  messages: () => string[];
  /** Opens a link (https or mailto) outside the app. */
  openLink: (url: string) => Promise<void>;
  /** Saves the report under `name`; resolves with where it went, to tell the user. */
  saveReport: (name: string, text: string) => Promise<string>;
}

function $(id: string): HTMLElement {
  const e = document.getElementById(id);
  if (!e) throw new Error(`#${id} missing`);
  return e;
}

export function setupHelp(hooks: HelpHooks): void {
  const panel = $('help-panel');
  const version = $('about-version');
  const build = $('about-build');
  const license = $('about-license');
  const consent = $('report-consent') as HTMLInputElement;
  const previewBox = $('report-preview-box') as HTMLDetailsElement;
  const preview = $('report-preview');
  const emailBtn = $('report-email') as HTMLButtonElement;
  const saveBtn = $('report-save') as HTMLButtonElement;
  const copyBtn = $('report-copy') as HTMLButtonElement;
  const status = $('report-status');

  /** The native side's part, as last asked (when the panel or the preview opened). */
  let native: NativeDiagnostics | null = null;
  const showAbout = () => {
    version.textContent = describeVersion({ build: BUILD, platform: hooks.platform, native });
    build.textContent = `${BUILD.commit} · ${BUILD.date}`;
    license.textContent = native?.asio
      ? 'GPLv3 (this build contains the Steinberg ASIO SDK)'
      : 'MIT';
  };
  showAbout();

  /**
   * The report as of now. Synchronous, so the buttons can open the mail program, download and
   * copy while the click still counts as the user's (browsers allow those only then).
   */
  function current(): ReportInput {
    return {
      build: BUILD,
      platform: hooks.platform,
      native,
      userAgent: navigator.userAgent,
      audio: hooks.audio(),
      params: hooks.params(),
      messages: hooks.messages(),
      created: new Date(),
    };
  }

  /** While the copy fallback selects the preview, it must not be rewritten. */
  let selecting = false;
  const showPreview = () => {
    if (previewBox.open && !selecting) preview.textContent = reportText(current());
  };
  async function refresh(): Promise<void> {
    native = await hooks.diagnostics().catch(() => null);
    showAbout();
    showPreview();
  }

  const tell = (text: string, isError = false) => {
    status.textContent = text;
    status.classList.toggle('is-error', isError);
  };
  const failed = (what: string) => (err: unknown) => {
    tell(`${what}: ${err instanceof Error ? err.message : String(err)}`, true);
  };

  const renderConsent = () => {
    saveBtn.disabled = copyBtn.disabled = !consent.checked;
  };
  consent.addEventListener('change', renderConsent);
  renderConsent();

  panel.addEventListener('toggle', () => {
    if (!panel.matches(':popover-open')) return;
    tell('');
    void refresh();
  });
  previewBox.addEventListener('toggle', () => {
    showPreview();
    if (previewBox.open && !selecting) void refresh();
  });

  /** Saves the report; resolves with where it went. */
  function save(input: ReportInput, name: string): Promise<string> {
    const text = reportText(input);
    if (previewBox.open) preview.textContent = text;
    return hooks.saveReport(name, text);
  }

  emailBtn.addEventListener('click', () => {
    const input = current();
    const name = consent.checked ? reportFileName(input.created) : null;
    const saving = name ? save(input, name) : Promise.resolve(null);
    const opening = hooks.openLink(mailtoUrl(input, name));
    const fallback = 'No mail program opened? Write to the address above from any mail service.';
    Promise.all([saving, opening])
      .then(([where]) => {
        tell(where ? `Saved the report: ${where}. Attach it to the email. ${fallback}` : fallback);
      })
      .catch(failed('Could not write the email'));
  });
  saveBtn.addEventListener('click', () => {
    const input = current();
    save(input, reportFileName(input.created))
      .then((where) => {
        tell(`Saved the report: ${where}.`);
      })
      .catch(failed('Could not save the report'));
  });
  copyBtn.addEventListener('click', () => {
    const text = reportText(current());
    navigator.clipboard.writeText(text).then(
      () => {
        tell('Copied the report. Paste it into your message.');
      },
      () => {
        // Not allowed here: select it for the user to copy. Opening the preview must not
        // rewrite it (its toggle event comes later, and is followed by this one-off reset).
        preview.textContent = text;
        if (!previewBox.open) {
          selecting = true;
          previewBox.addEventListener(
            'toggle',
            () => {
              selecting = false;
            },
            { once: true },
          );
          previewBox.open = true;
        }
        const range = document.createRange();
        range.selectNodeContents(preview);
        getSelection()?.removeAllRanges();
        getSelection()?.addRange(range);
        preview.scrollTop = 0;
        tell('Copying is not allowed here, so the report is selected below: press Ctrl+C (⌘C).');
      },
    );
  });

  // In the desktop app, links open in the system's browser and mail program.
  if (hooks.platform === 'desktop') {
    panel.addEventListener('click', (e) => {
      const link = (e.target as Element | null)?.closest<HTMLAnchorElement>('a[href]');
      if (!link) return;
      e.preventDefault();
      hooks.openLink(link.href).catch(failed('Could not open the link'));
    });
  }
}

/** The browser's way to open a link: a new tab, or the mail program for `mailto:`. */
export function openInBrowser(url: string): Promise<void> {
  const a = document.createElement('a');
  a.href = url;
  if (!url.startsWith('mailto:')) {
    a.target = '_blank';
    a.rel = 'noopener';
  }
  a.click();
  return Promise.resolve();
}

/** The browser's way to save a file: a download. */
export function downloadText(name: string, text: string): Promise<string> {
  const url = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => {
    URL.revokeObjectURL(url);
  }, 10_000);
  return Promise.resolve(`${name}, in your downloads`);
}
