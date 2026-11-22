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
> version of the web app (`web/`) runs locally; public hosting and a desktop app with lower
> latency come next. See the [project plan](docs/PLAN.md).

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
crates/klirr-dsp/   DSP core in Rust (shared by the web and desktop versions)
crates/klirr-wasm/  WebAssembly bridge: a small C ABI around the core for the AudioWorklet
web/                web app: Vite + TypeScript UI, AudioWorklet host
docs/               project plan; later user and developer docs
audio/              your local test recordings (ignored by git)
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
- **Latency** in the browser is roughly 20–40 ms from string to ear on Windows (browsers cannot
  use ASIO). Fine for practice; the desktop app will do better.
- **Cabinet IRs:** drop a `.wav` impulse response anywhere on the page (or use *Load IR…*).
  It is read locally and never uploaded.
- Many interfaces show up as one stereo device: *Input 1* is the left channel, *Input 2* the
  right one.

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

## Contributing

Issues and pull requests are welcome. CI runs formatting, clippy, tests on Linux, Windows and
macOS, a WebAssembly build, a minimum-Rust-version check and the web app's lint, type check,
tests and build; please make sure `cargo fmt`, `cargo clippy --all-targets -- -D warnings`
and `cargo test` pass, and in `web/` `npm run lint`, `npm run typecheck` and `npm test`.

## License

[MIT](LICENSE)
