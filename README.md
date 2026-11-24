<p align="center">
  <img src="docs/assets/logo.svg" alt="Klirr" width="330">
</p>

<p align="center"><strong>The free &amp; open-source guitar amp, in your browser.</strong></p>

<p align="center">
  <a href="https://github.com/MaximBecht/klirr/actions/workflows/ci.yml"><img src="https://github.com/MaximBecht/klirr/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT license"></a>
</p>

---

Plug your guitar into your sound card, open a web page, play. Klirr is an amp simulator with
clean, crunch and high-gain channels, a tone stack, speaker cabinets, delay and reverb. All
processing happens on your own computer; nothing is uploaded.

*The name comes from the German* Klirrfaktor *— total harmonic distortion, which is exactly
what an amp's gain stage produces.*

> **Status: early development.** The DSP core (`crates/klirr-dsp`) is done and tested. The first
> versions of the web app (`web/`) and of the desktop app (`desktop/`, lower latency) run
> locally; public hosting and installers come next. See the [project plan](docs/PLAN.md).

## Features

- **Three amp channels** — clean, crunch and two-stage high gain, 4× oversampled with
  anti-aliased waveshapers, so distortion stays smooth instead of fizzy.
- **Tone stack** — bass, mid and treble (±12 dB) that glide without clicks.
- **Speaker cabinets** — a 1×12 combo and a 4×12 stack generated in code, or load your own
  impulse response (`.wav`).
- **Delay and reverb** — tape-style gliding delay with darkening repeats, and a stable
  algorithmic reverb.
- **Noise gate** with hysteresis, and a **safety limiter** on the output.
- **Fast** — about 1 % of one CPU core for the full chain (measured natively; see `bench`).

## Signal chain

```
input trim → noise gate → high-pass → amp channel (4× oversampled) → tone stack
  → cabinet → master → delay → reverb → safety limiter → output
```

## Repository layout

```
crates/klirr-dsp/    DSP core in Rust (shared by the web and desktop versions)
crates/klirr-wasm/   WebAssembly bridge: a small C ABI around the core for the AudioWorklet
crates/klirr-native/ native audio host: the core on WASAPI, ASIO, Core Audio, ALSA, JACK
web/                 web app: Vite + TypeScript UI, AudioWorklet host (also the desktop UI)
desktop/             desktop app (Tauri): the web UI on the native audio host
docs/                project plan; later user and developer docs
audio/               your local test recordings (ignored by git)
```

## Development

You need a recent stable [Rust](https://rustup.rs) toolchain (1.85 or newer).

```sh
cargo test                      # unit and regression tests
cargo clippy --all-targets      # lints
cargo fmt --all                 # formatting
```

### Listen to the engine without a UI

Record your guitar dry (DI, straight from the interface) to a `.wav` file, put it in the
`audio/` folder, then render it through the amp:

```sh
cargo run --release --example render -- audio/di.wav audio/out.wav channel=2 gain=0.7 cabinet=2 reverb_mix=0.2
```

Everything in `audio/` (except its README) is ignored by git, so your recordings and renders
never end up in the repository.

Settings are `name=value` pairs, listed here in signal-chain order. Values outside the range
are clamped; selectors are rounded to whole numbers.

| Stage    | Parameter           | Range             | Default | Notes                                                |
|----------|---------------------|-------------------|---------|------------------------------------------------------|
| Input    | `input_gain_db`     | -24 – 24 dB       | 0       | Trim before the amp                                  |
| Gate     | `gate_threshold_db` | -96 – -20 dB      | -70     | -96 turns the gate off                               |
| Amp      | `channel`           | 0, 1, 2           | 0       | 0 clean, 1 crunch, 2 high gain                       |
| Amp      | `gain`              | 0 – 1             | 0.5     | Drive                                                |
| Tone     | `bass_db`           | -12 – 12 dB       | 0       |                                                      |
| Tone     | `mid_db`            | -12 – 12 dB       | 0       |                                                      |
| Tone     | `treble_db`         | -12 – 12 dB       | 0       |                                                      |
| Cabinet  | `cabinet`           | 0, 1, 2, 3        | 1       | 0 off, 1 1×12 combo, 2 4×12 stack, 3 custom IR       |
| Master   | `master_db`         | -60 – 12 dB       | -6      | Output volume                                        |
| Delay    | `delay_time_ms`     | 10 – 2000 ms      | 300     |                                                      |
| Delay    | `delay_feedback`    | 0 – 0.95          | 0.35    | Number of repeats                                    |
| Delay    | `delay_mix`         | 0 – 1             | 0       | Delay is off until this is raised                    |
| Reverb   | `reverb_size`       | 0 – 1             | 0.5     | Room size                                            |
| Reverb   | `reverb_damping`    | 0 – 1             | 0.5     | Higher is darker                                     |
| Reverb   | `reverb_mix`        | 0 – 1             | 0       | Reverb is off until this is raised                   |
| Global   | `bypass`            | 0, 1              | 0       | 1 passes the dry signal through                      |

The render example also takes two options of its own:

| Option          | Default | Notes                                                        |
|-----------------|---------|--------------------------------------------------------------|
| `input_channel` | 0       | Which channel of a multi-channel `.wav` to render            |
| `ir`            | none    | Path to a cabinet impulse response `.wav`; use with `cabinet=3` |

The input must be a `.wav` file. To convert another format, e.g. with ffmpeg:
`ffmpeg -i audio/di.mp3 audio/di.wav`.

### Measure performance

```sh
cargo run --release --example bench
```

### Web app

You also need [Node.js](https://nodejs.org) 20.19 or newer and the WebAssembly target:

```sh
rustup target add wasm32-unknown-unknown
cd web
npm install
npm run dev          # builds the engine to WebAssembly, then serves http://localhost:5173
```

Open the page in Chrome or Edge (they can also choose the output device), pick your input,
press **Engage** and allow audio input. Your settings are remembered in the browser.

- **Use a Hi-Z / instrument input.** A guitar on a plain line-in works, but sounds weaker and
  duller; an audio interface or a pedal in front fixes that.
- **Latency** in the browser is roughly 20–40 ms from string to ear on Windows in Chrome and Edge
  (browsers cannot use ASIO), more in Firefox. Fine for practice; the desktop app will do better.
  Click the latency figure in the footer for the browser's breakdown, or measure the real figure
  with a cable from your interface's output to its input.
- **Cabinet IRs:** drop a `.wav` impulse response anywhere on the page (or use *Load IR…*).
  It is read locally and never uploaded.
- Many interfaces show up as one stereo device: *Input 1* is the left channel, *Input 2* the
  right one.

**Help** shows the version and build, and helps report a bug: *Write Email* opens your mail
program with a message to support@bechtstudio.com. Only if you tick the box does it include your
system and audio setup and save a report file for you to attach (in the desktop app, with its
recent log); *What is included* shows the whole report first. Klirr sends nothing itself.

Other scripts in `web/`:

```sh
npm test               # Vitest (needs `npm run build:wasm` first; `dev` and `build` run it)
npm run test:e2e       # Playwright smoke test in Chromium with a fake microphone
                       # (first time: `npx playwright install chromium`)
npm run lint           # ESLint
npm run format         # Prettier
npm run typecheck      # TypeScript
npm run build          # production build into web/dist/
```

### Native audio host

`crates/klirr-native` runs the same engine natively, through [cpal](https://crates.io/crates/cpal):
WASAPI (shared mode) on Windows, optionally ASIO; Core Audio on macOS; ALSA or JACK on Linux. On
Windows it also has a WASAPI exclusive mode of its own, through the
[wasapi](https://crates.io/crates/wasapi) crate. It is
the audio side of the desktop app (below), and a command-line tool to compare native latency
with the browser's. Its loopback measurement is the same click probe as the web app's
latency panel, so the figures compare directly.

```sh
cargo run --release -p klirr-native -- devices                      # hosts and devices
cargo run --release -p klirr-native -- buffers --host wasapi-exclusive  # buffer sizes and rates allowed
cargo run --release -p klirr-native -- setup --host asio             # the driver's control panel
cargo run --release -p klirr-native -- measure --device Focusrite   # loopback latency
cargo run --release -p klirr-native -- play --device Focusrite channel=2 gain=0.7 cabinet=2
```

`measure` needs a cable from an output of the interface to the selected input (turn direct
monitoring off). It prints the stream setup, the backend's own latency figures and the measured
round trip, plus any dropouts. Device options: `--host`, `--device` (input and output),
`--input`, `--output`, `--channel N` (from 1), `--rate`, `--buffer FRAMES`, `--margin FRAMES`;
`play` takes the same `name=value` settings as the render example and `ir=path.wav`. On Linux,
building it needs the ALSA headers (`libasound2-dev`).

- **WASAPI** (default on Windows) runs in shared mode, like the browser: the period is fixed by
  Windows (usually 10 ms), so expect figures close to the browser's. A device another program
  holds in exclusive mode (Voicemeeter, a DAW) cannot be opened.
- **WASAPI exclusive** (`--host wasapi-exclusive`) bypasses the Windows mixer and uses the
  device's smallest period (often 3 ms), with no SDK and no extra build step. It takes the
  device away from every other program while it runs, so close them first (Voicemeeter, a
  DAW, browser tabs); `devices --host wasapi-exclusive` shows what each device accepts.
  `--buffer FRAMES` sets the period.
- **ASIO** talks to the interface's own driver with small buffers. Build with
  `--features asio` and run with `--host asio --buffer 64` (or 128). The build downloads the
  Steinberg ASIO SDK and needs LLVM for its bindings (`winget install LLVM.LLVM`, then set
  `LIBCLANG_PATH` to LLVM's `bin` folder if it is not found). The SDK is not part of this
  repository. It is dual-licensed (proprietary or GPLv3); only a Windows build with ASIO
  contains it, so only that binary is affected. The source, the web version and the macOS and
  Linux builds stay MIT (see the [project plan](docs/PLAN.md), section 6).

Some drivers use their ASIO buffer size for their Windows (WASAPI) path as well, and keep the
last size an ASIO program set. On a Focusrite Scarlett, WASAPI exclusive measured 11 ms string
to ear with the driver at 32 frames and 25 ms at 256; shared mode stayed near 89 ms either way.
Set the buffer small in the interface's control software before measuring or playing.

### Desktop app

`desktop/` is a [Tauri](https://tauri.app) app: the web app's UI in the system webview (WebView2
on Windows, WebKit on macOS and Linux) and the amp on the native audio host. The webview only
shows the UI; the audio never passes through it. The UI is `web/`, built with
`--mode desktop`, which swaps the browser's audio engine for one that drives the native host
over Tauri's IPC and adds a driver menu (WASAPI shared or exclusive, ASIO, Core Audio, JACK …).

```sh
cd desktop
npm install
npm run dev                        # the app, with the UI served by Vite (hot reload)
npm run build                      # a release build and installer, in target/release/bundle/
npm run build -- --features asio   # the same with ASIO (Windows; see above)
npm run build:windows              # the Windows release installer, with both builds
```

The Windows release installer holds both builds of `klirr.exe` and installs one: **ASIO
support (GPLv3)** is a component, selected by default, that installs the ASIO build in place of
the MIT one. It remembers the choice for updates; `/ASIO=0` or `/ASIO=1` sets it for silent
installs (`/S`). Its NSIS template is Tauri's, with the changes marked, in
`desktop/src-tauri/windows/`; the licenses it installs say which build is under which license.

The **Audio** panel holds the whole audio setup: driver, devices, sample rate, buffer size, the
latency figures and a test tone. Klirr only asks for a buffer size or sample rate when it opens
the devices and writes no driver or system settings itself, so nothing it chooses outlives it:

- The **buffer size** is a menu where the driver lets Klirr ask for one (WASAPI exclusive,
  ASIO drivers that report a range, Core Audio, ALSA); where it belongs to the system or the
  driver (WASAPI shared, JACK, ASIO drivers with one size), it is shown greyed out.
- The **sample rate** is a menu only on WASAPI exclusive, where the device runs at Klirr's rate
  while Klirr holds it and goes back to its own after. Elsewhere it is shown greyed out: an
  ASIO driver's rate applies to every program.
- **Hardware Setup** opens where the settings for every program live: the ASIO driver's own
  control panel, the Windows Sound settings, or Audio MIDI Setup on macOS. When an ASIO driver
  asks for a restart after a change there, Klirr opens the devices again by itself.
- The **test tone** (440 Hz at −20 dBFS, in place of the amp) checks the output; it stops when
  the panel closes.

If a driver refuses a size or rate, Klirr opens it with the driver's own and says so. Each driver
remembers its own devices and each output device its own buffer and rate. The **Latency** panel
is the measuring tool: the loopback measurement, and how much of it the driver does not report.

The app keeps its settings in `settings.json` in its config folder
(`%APPDATA%\com.bechtstudio.klirr` on Windows,
`~/Library/Application Support/com.bechtstudio.klirr` on macOS,
`~/.config/com.bechtstudio.klirr` on Linux), the same format on every platform. The file
is versioned (`"version"`); an older one is migrated when it is read, fields the app does not
know are kept, an unreadable one is kept as `settings.broken.json`, and a newer app's file is
copied to `settings.v<N>.json` before an older app writes over it. Builds before 0.1.0-alpha.1
used the folder `io.github.maximbecht.klirr`; its settings are copied over once, on the first
start, and the old folder is left as it was.

The app also keeps a log of what it opened and what failed (no audio, no knob turns) in
`klirr.log` in its log folder (`%LOCALAPPDATA%\com.bechtstudio.klirr\logs` on Windows,
`~/Library/Logs/com.bechtstudio.klirr` on macOS, `~/.local/share/com.bechtstudio.klirr/logs`
on Linux). Each start keeps the last run's log as `klirr.previous.log`. The home folder shows as
`~` in it.

You need Node.js 20.19 or newer and the web app's dependencies (`npm install` in `web/`). Rust
1.90 or newer builds the app. On Linux it also needs WebKitGTK and a few libraries (see
[Tauri's prerequisites](https://tauri.app/start/prerequisites/#linux)); on Debian or Ubuntu:
`sudo apt install libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev
librsvg2-dev libasound2-dev`. The app icons are rendered from `desktop/icon.svg` with
`npm run icons`.

## Contributing

Issues and pull requests are welcome. CI runs formatting, clippy, tests on Linux, Windows and
macOS, a WebAssembly build, a minimum-Rust-version check and the web app's lint, type check,
tests and build; please make sure `cargo fmt`, `cargo clippy --all-targets -- -D warnings`
and `cargo test` pass, and in `web/` `npm run lint`, `npm run typecheck` and `npm test`.

## License

[MIT](LICENSE)
