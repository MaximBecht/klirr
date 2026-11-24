// Bug reports: the text of the report file and the email that goes with it. Klirr sends
// nothing itself: the Help panel opens the user's mail program with a short message and saves
// the full report for them to attach, and only with their consent.

import { PARAMS } from './params';

/** Where bug reports go. */
export const SUPPORT_EMAIL = 'support@bechtstudio.com';

/** The build, from `vite.config.ts`. */
export interface BuildInfo {
  version: string;
  /** Short commit hash, with `+dirty` when built with uncommitted changes. */
  commit: string;
  /** Build date, `YYYY-MM-DD`. */
  date: string;
}

export const BUILD: BuildInfo = {
  version: __KLIRR_VERSION__,
  commit: __KLIRR_COMMIT__,
  date: __KLIRR_BUILD_DATE__,
};

/** What the desktop app's native side reports (the `diagnostics` command). */
export interface NativeDiagnostics {
  version: string;
  asio: boolean;
  os: string;
  arch: string;
  webview: string;
  log: string[];
  previousLog: string[] | null;
  logPath: string | null;
}

/** Everything a report holds. */
export interface ReportInput {
  build: BuildInfo;
  platform: 'web' | 'desktop';
  /** The desktop app's native side; null in the browser. */
  native: NativeDiagnostics | null;
  /** The browser (web) or the webview (desktop). */
  userAgent: string;
  /** Audio, as label → value lines, in the order shown. */
  audio: [string, string][];
  /** Parameter values, by index. */
  params: number[];
  /** What the UI told the user, oldest first, with times. */
  messages: string[];
  /** When the report was made. */
  created: Date;
}

/** `2026-11-24 18:43:07Z`, as the native log writes it. */
export function utcStamp(date: Date): string {
  return `${date.toISOString().slice(0, 19).replace('T', ' ')}Z`;
}

/** `klirr-report-20261124-184307.txt`. */
export function reportFileName(date: Date): string {
  const digits = date.toISOString().slice(0, 19).replace(/\D/g, '');
  return `klirr-report-${digits.slice(0, 8)}-${digits.slice(8)}.txt`;
}

/** "0.1.0 (desktop, with ASIO)". */
export function describeVersion(input: Pick<ReportInput, 'build' | 'platform' | 'native'>): string {
  const kind = input.platform === 'desktop' ? 'desktop app' : 'web app';
  const asio = input.native?.asio ? ', with ASIO' : '';
  return `${input.build.version} (${kind}${asio})`;
}

/** The operating system, as well as it is known. */
function describeSystem(input: ReportInput): string {
  if (input.native) return `${input.native.os}, ${input.native.arch}`;
  return input.userAgent;
}

const section = (title: string, lines: string[]) =>
  [title, '-'.repeat(title.length), ...lines, ''].join('\n');
const rows = (pairs: [string, string][]) => {
  const width = Math.max(0, ...pairs.map(([k]) => k.length));
  return pairs.map(([k, v]) => `${`${k}:`.padEnd(width + 2)}${v}`);
};

/** The full report, as plain text. */
export function reportText(input: ReportInput): string {
  const app: [string, string][] = [
    ['Version', describeVersion(input)],
    ['Build', `${input.build.commit} · ${input.build.date}`],
  ];
  if (input.native && input.native.version !== input.build.version)
    app.push(['Native side', input.native.version]);
  const system: [string, string][] = [['System', describeSystem(input)]];
  if (input.native) {
    system.push(['Webview', input.native.webview]);
    system.push(['User agent', input.userAgent]);
  }
  const params = PARAMS.map((p) => `${p.name}=${formatValue(input.params[p.index])}`);

  const parts = [
    `Klirr bug report\n================\nCreated ${utcStamp(input.created)}\n`,
    section('App', rows([...app, ...system])),
    section('Audio', rows(input.audio)),
    section('Settings', wrap(params.join(' '), 90)),
    section('Messages shown', input.messages.length ? input.messages : ['(none)']),
  ];
  if (input.native) {
    parts.push(
      section('Log of this run', input.native.log.length ? input.native.log : ['(empty)']),
    );
    if (input.native.previousLog)
      parts.push(
        section('Log of the previous run, which did not end cleanly', input.native.previousLog),
      );
    if (input.native.logPath) parts.push(`Log file: ${input.native.logPath}\n`);
  }
  return parts.join('\n');
}

function formatValue(v: number | undefined): string {
  if (v === undefined) return '?';
  return Number.isInteger(v) ? String(v) : v.toFixed(3).replace(/0+$/, '');
}

/** Breaks a line of words at `width` characters. */
function wrap(text: string, width: number): string[] {
  const lines: string[] = [];
  let line = '';
  for (const word of text.split(' ')) {
    if (line && line.length + 1 + word.length > width) {
      lines.push(line);
      line = word;
    } else {
      line = line ? `${line} ${word}` : word;
    }
  }
  if (line) lines.push(line);
  return lines;
}

/**
 * The email: a template for the user to fill in, and below it one line about the setup and
 * the name of the report file to attach, if they agreed to include it. Short, because mail
 * programs cut long `mailto:` links (about 2,000 characters on Windows).
 */
export function mailtoUrl(
  input: Pick<ReportInput, 'build' | 'platform' | 'native' | 'audio'> & { userAgent: string },
  attachment: string | null,
): string {
  const subject = `Klirr ${input.build.version} bug report`;
  const body = [
    'What happened:',
    '',
    '',
    'What did you expect instead:',
    '',
    '',
    'Steps to get there:',
    '',
    '',
    '---',
    `Klirr ${describeVersion(input)}, build ${input.build.commit}`,
  ];
  if (attachment) {
    const system = input.native?.os ?? shortAgent(input.userAgent);
    const driver = input.audio.find(([k]) => k === 'Driver')?.[1];
    body.push(driver ? `${system} · ${driver}` : system);
    body.push(`Please attach the report file: ${attachment}`);
  }
  const encode = (s: string) => encodeURIComponent(s);
  return `mailto:${SUPPORT_EMAIL}?subject=${encode(subject)}&body=${encode(body.join('\r\n'))}`;
}

/** The browser and system part of a user agent, cut to fit an email line. */
function shortAgent(agent: string): string {
  return agent.length > 160 ? `${agent.slice(0, 157)}...` : agent;
}

/** The last messages the UI showed, with their times. */
export class RecentMessages {
  private readonly lines: string[] = [];
  constructor(private readonly keep = 50) {}

  push(message: string, isError: boolean, when = new Date()): void {
    const time = when.toISOString().slice(11, 19);
    this.lines.push(`${time}Z ${isError ? 'ERROR ' : ''}${message}`);
    if (this.lines.length > this.keep) this.lines.shift();
  }

  all(): string[] {
    return [...this.lines];
  }
}
