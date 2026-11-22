# Klirr — project plan

> **Klirr** — the free & open-source guitar amp, in your browser (and later on your desktop).
>
> The name comes from the German *Klirrfaktor*: total harmonic distortion — exactly what an
> amp's gain stage produces.

This document records the decisions behind Klirr. It is the reference for what we build and
why. Update it when a decision changes.

---

## 1. Goals

- A **small, easy to use** guitar amp simulator: plug a guitar into a sound card, open a web
  page, play.
- **Publicly hosted** on Cloudflare at **zero cost** (the rest of the infrastructure is already
  there).
- **Clean sound**: no artefacts beyond the intended clean / crunch / high-gain character.
- **Performant** processing that also works on modest machines.
- A **public, open-source repository** following best practices: tests, CI/CD, documentation.
- Later: an optional **desktop app** with lower latency.

## 2. Decisions

| Topic | Decision |
|---|---|
| Name | **Klirr**, with a vacuum-tube logo next to the word mark |
| Tagline | "the free & open-source guitar amp" (+ "in your browser" / "and on your desktop") |
| License | **MIT** |
| DSP core | **Rust**, from the start, shared by the web and desktop versions |
| First target | **Web version** (Rust → WebAssembly in an `AudioWorklet`) |
| UI | TypeScript + Vite, ESLint + Prettier |
| Hosting | Cloudflare Workers **static assets** (no Worker code on the request path) |
| Desktop (later) | Tauri: UI in the webview, audio in the native Rust backend via `cpal` |
| Native audio backends | `cpal`: WASAPI shared + optional ASIO on Windows, Core Audio, ALSA/JACK. WASAPI exclusive later, if needed, as raw WASAPI via the `windows` crate (Windows only) |
| Distribution (desktop) | GitHub Releases built by CI; the site's download button links to the latest release |
| Cabinet sound | Built-in cabinets generated in code (no third-party files) + load your own IR |
| Technical names | repo `klirr`, crate `klirr-dsp`, web package `klirr-web` |

### Why Rust from the start

A desktop app only gets lower latency if the audio runs natively, outside the webview. That
needs the DSP in Rust anyway. Writing it once in Rust and compiling it to WebAssembly for the
web avoids a second implementation later. The trade-off is more up-front work, because the
web version cannot lean on the browser's built-in audio nodes.

### Why the audio runs on the user's machine

A server can only reach its own sound card, and sending audio to a server and back adds
50–200 ms of latency. So all processing happens locally: in the browser for the web version,
natively in the desktop app. The server only delivers files.

## 3. Artefacts the design avoids

Typical causes of sound artefacts in simple amp simulators, and how Klirr prevents each one.
Every row has a regression test in `crates/klirr-dsp/tests/regressions.rs`.

| Artefact | Typical cause | How Klirr avoids it |
|---|---|---|
| Buzz or clicks at the audio block rate | Filter state reset or rescaled at every block | Every stage runs per sample with continuous state; output is identical for any host block size |
| Lost dynamics, gain "stepping" | Normalising the distortion output per block | Static gain per setting; quiet playing stays quiet |
| Fizzy, metallic, off-pitch distortion | Aliasing from waveshaping at the base sample rate | 4× oversampling plus antiderivative anti-aliasing (ADAA) |
| Chopped note tails, clicks from the gate | Gate switching whole blocks, no attack/release | Per-sample envelope, hold time, 6 dB hysteresis, smooth fades |
| Reverb or delay blowing up | Feedback gain ≥ 1 | Feedback capped below 1 in every loop |
| Crackle while turning knobs | Coefficient jumps or state resets on parameter changes | Parameters glide; tone stack uses state-variable filters; channels and cabinets warm up before crossfading |
| DC offset, thumps | Asymmetric clipping without a DC blocker | DC blocker after every amp channel |
| Harsh digital clipping | EQ boosts or master pushing the output past 0 dBFS | Master after the EQ, safety limiter at −1 dBFS |
| Dropouts | Separate input/output streams drifting apart | Single processing callback (`AudioWorklet` / duplex stream) |

## 4. Architecture

### 4.1 Repository layout (target)

```
klirr/
├── crates/
│   ├── klirr-dsp/        # Pure Rust DSP core: no I/O, no allocation while processing
│   ├── klirr-wasm/       # C-ABI exports of the core for the AudioWorklet
│   └── klirr-native/     # Native audio host (cpal); the desktop app's audio side
├── web/                  # Vite + TypeScript UI, AudioWorklet host
├── desktop/              # (phase 4) Tauri app
├── docs/                 # This plan; later user and developer docs
└── .github/workflows/    # CI/CD
```

### 4.2 Signal chain

```
Audio input (interface, channel 1 or 2)
  → Input trim                         + input meter
  → Noise gate                         smooth attack / hold / release, hysteresis
  → High-pass (70 Hz)                  removes rumble; keeps the low E (82 Hz) intact
  → Amp channel (clean / crunch / high gain), each with:
        pre-EQ (tighter high-pass for crunch / high gain) → drive
        → anti-aliased waveshaper(s) at 4× rate → DC blocker
      a new channel warms up silently for 10 ms, then crossfades in over ~10 ms
  → Tone stack                         bass (low shelf) / mid (bell) / treble (high shelf),
                                       state-variable filters that glide per sample
  → Cabinet                            built-in generated IR or your own .wav (convolution)
  → Master volume
  → Delay                              fractional delay line, darkening feedback loop
  → Reverb                             algorithmic (Freeverb-style), feedback capped < 1
  → Safety limiter                     ceiling just below 0 dBFS
  → Output                             + output meter
```

Design rules behind section 3:

- **State is continuous across blocks.** Output does not depend on the host's block size
  (tested: processing in blocks of 1, 37, 128 and 1024 samples gives identical output).
- **No per-block normalisation.** Gain is static per setting; dynamics are preserved.
- **4× oversampling plus ADAA** (antiderivative anti-aliasing) around every non-linearity.
- **Parameters glide** (smoothed over a few ms) instead of jumping. The tone stack uses
  state-variable filters, which stay clean when their gain changes every sample; filter state
  is never reset while playing.
- **Switching never starts a stage cold.** Cabinet convolvers keep running in the background;
  a newly selected amp channel warms up silently before it is crossfaded in.
- **Feedback paths are bounded** (reverb, delay) so nothing can blow up.
- **No allocation and no locks** inside `process`; everything is sized up front.

Reverb note: the earlier idea was a generated reverb IR played through a convolver. In the
Rust core an algorithmic reverb is much cheaper than a seconds-long convolution in WebAssembly,
so the core uses a stable Freeverb-style design. An IR reverb can be added later.

Latency added by the core: 23 samples (oversampling filters) + 32 samples (cabinet
convolution) ≈ 1.1 ms at 48 kHz. Measured cost of the full chain (high gain, cabinet, delay,
reverb): ~1.2 % of one CPU core natively, 81× faster than real time. WebAssembly will be
somewhat slower, still far within budget.

Measured aliasing (1760 Hz sine at maximum gain, energy off the harmonics relative to the
harmonics): clean −72 dB, crunch −68 dB, high gain −70 dB. Without oversampling, a comparable
high-gain stage measures around −22 dB.

### 4.3 Web version

- The Rust core is compiled to `wasm32-unknown-unknown` (`crates/klirr-wasm`) and loaded into
  an `AudioWorklet`. Worklets cannot `fetch`, so the main thread fetches the module bytes and
  sends them to the worklet, which instantiates them. (Sending bytes rather than a compiled
  `WebAssembly.Module` works in every browser.) Exports are a small C ABI, not wasm-bindgen
  glue; the module has no imports.
- Each worklet instance owns one engine. Audio passes through two fixed buffers in wasm
  memory; the worklet rebuilds its typed-array views whenever memory grows.
- UI → audio: knob changes are sent as messages and smoothed inside the core. Meters come back
  at ~30 fps.
- Optional later: `SharedArrayBuffer` for meters/params. It needs COOP/COEP headers, which are
  set in a static `_headers` file (still no Worker invocation). Those headers also block fonts
  from Google's servers, so **fonts are self-hosted**.

### 4.4 Input and output

- Any input device the OS exposes: PC line-in / mic, USB interface, with pedals or other
  hardware in front.
- **Device + channel selection** (many interfaces show up as one stereo device: input 1 = left,
  input 2 = right).
- `getUserMedia` with **echo cancellation, noise suppression and auto-gain turned off**.
- Input trim with a clip indicator.
- **Output device selection** with `AudioContext.setSinkId` (Chrome, Edge). Detected by
  feature (`'setSinkId' in AudioContext.prototype`), not by browser name. If missing
  (Firefox, Safari), show a **visible notice** near the device picker: audio plays on the
  system default; for full device selection use Chrome or Edge.
- Expected latency in the browser on Windows: roughly 20–40 ms string-to-ear (no ASIO in
  browsers). Fine for practice; documented in the README.
- README note: a guitar wants a high-impedance (Hi-Z / instrument) input. Line-in works but
  sounds weaker and duller; an interface or a pedal in front fixes it.

### 4.5 Cabinets and impulse responses (IRs)

- An IR is a short recording of how a speaker cabinet + mic colour the sound. Without a cabinet
  stage, distortion sounds harsh and fizzy.
- **Built-in cabinets are generated in code** from a speaker model (low bump ~100 Hz, presence
  peaks ~2–3 kHz, steep roll-off above ~5 kHz), e.g. a 1×12 combo and a 4×12 stack. Our own
  work, so it is MIT-licensed with no third-party files.
- **Load your own IR**: drag a `.wav` onto the page. It is read locally and never uploaded.
- Third-party IR packs are **not** shipped: their licenses often forbid redistribution.
- Side project: record your own IRs with the Behringer ECM-8000. Play a sine sweep through the
  cab (clean power amp or the amp's FX return), close-mic it, deconvolve in **REW** (Room EQ
  Wizard, free) with the mic calibration file loaded, export as `.wav`, trim to 20–100 ms with
  a short fade-out, normalise. The result is neutral (measurement mic) and can be shipped with
  the project.

## 5. Hosting and cost (Cloudflare)

- All audio processing runs on the visitor's machine. Cloudflare only serves files (HTML, JS,
  CSS, `.wasm`), so the Workers CPU limit does not apply.
- Cloudflare docs: "Requests to static assets are free and unlimited", and "Only requests that
  hit a Worker will count against your limits and your bill." → the site uses **0** of the
  free plan's 100,000 requests/day. No bandwidth charges.
- **Do not enable `run_worker_first`**: it would run Worker code on every request (counted,
  and 429 errors on the free plan once the limit is reached).
- Free-plan asset limits: 20,000 files, 25 MiB per file — far more than needed.
- Unconfirmed: whether SPA fallback (`not_found_handling`) counts as a Worker request.
  Irrelevant here: a single page needs no fallback.
- Hosting on a subdomain of the existing domain (e.g. `klirr.<your-domain>`).
- Only a future API (e.g. saved/shared presets in KV or D1) would count requests, and only for
  those calls.

## 6. Desktop app (later)

- Tauri uses the OS webview (WebView2 on Windows). Web Audio inside the webview would have the
  **same** latency as the browser, so the audio must run in Tauri's **native Rust backend**.
- Audio I/O via `cpal` (`crates/klirr-native`): WASAPI, and ASIO with an extra build step
  (`--features asio`: LLVM for bindgen; the SDK is downloaded by the build). Target latency
  ~5–10 ms.
- cpal's WASAPI is **shared mode only**, with the period Windows picks (usually 10 ms), so it
  is roughly as fast as the browser. The low latency comes from ASIO on Windows and Core Audio
  on macOS. **WASAPI exclusive mode** (bypasses the Windows mixer, small buffers, no SDK, but
  takes over the device) can be added later as a small Windows-only backend calling
  `IAudioClient::Initialize` with `AUDCLNT_SHAREMODE_EXCLUSIVE` through the `windows` crate.
- cpal opens input and output as two streams. The input pushes into a lock-free ring, the
  output pulls from it and runs the engine (one processing callback, as in section 3). The
  ring is trimmed to a small safety margin so a random phase between the streams adds no
  latency; an underrun plays one block of silence and raises the margin. With ASIO both
  callbacks run in the driver's cycle, input first, so the ring adds nothing.
- `klirr-native measure` runs the same loopback probe as the web app, for a direct
  browser-vs-native comparison.
- The webview only shows the UI and sends parameter changes to the Rust side.
- Installers are small (≈3–10 MB) because no browser is bundled.
- **Distribution:** `tauri-action` builds Windows/macOS/Linux installers on a release tag and
  attaches them to a GitHub Release. Download button →
  `github.com/MaximBecht/klirr/releases/latest` (or `/releases/latest/download/<file>` with fixed
  asset names). Tauri's updater can read from the same releases. No Google Drive.
- **Licensing per target.** The ASIO SDK is dual-licensed since October 2025: proprietary or
  GPLv3. GPL obligations attach to the binaries that contain the SDK, not to the repository:
  - The source stays **MIT** everywhere. The SDK is not in the repository; the `asio` build
    feature downloads it at build time.
  - **Web, macOS and Linux** builds contain no ASIO and stay **MIT**.
  - The **Windows build with `--features asio`** is a combined work with the SDK and is
    distributed under **GPLv3** (MIT code may be combined into a GPLv3 work). The installer
    ships the GPLv3 text, a notice naming the SDK, and a link to the exact source tag with
    build steps; all dependencies of that build must be GPLv3-compatible (MIT and Apache-2.0
    are). The "ASIO" name and logo follow Steinberg's trademark rules.
  - Optional: a WASAPI-only Windows build that stays MIT, for anyone who needs that.
  - Alternative: sign Steinberg's proprietary ASIO license instead, which keeps the Windows
    binary MIT as well.
- **Code signing:** unsigned Windows installers show a SmartScreen warning (SignPath offers
  free signing for open source; Azure Trusted Signing ≈ $10/month). macOS needs an Apple
  Developer account ($99/year) for signing + notarisation. It is fine to launch unsigned first.

## 7. Quality and project hygiene

- **Tests:** Rust unit + regression tests for the DSP (block-size invariance, stability,
  aliasing limit, gate smoothness, no NaN/Inf, limiter ceiling). Vitest for the UI and a
  Playwright smoke test with Chrome's fake microphone (a generated pluck-and-silence WAV;
  checks that sound flows, stays finite and under the limiter ceiling, and stops).
- **CI (GitHub Actions):** `cargo fmt --check`, `clippy -D warnings`, tests, a
  `wasm32-unknown-unknown` build, and web lint/format/typecheck/test/build/smoke test on every
  push/PR. Later: preview deploy per PR, production deploy on `main` via `wrangler`, desktop release builds on
  tags.
- **Docs:** README to start. When the project grows: a `docs/` folder (user guide, DSP design,
  contributing).
- Dependabot for Cargo, npm and GitHub Actions.

## 8. Roadmap

### Phase 1 — DSP core (`crates/klirr-dsp`)
- [x] Workspace, MIT license, README, CI (fmt, clippy, docs, tests on 3 OSes, wasm build, MSRV)
- [x] Gate, high-pass, three amp channels with 4× oversampling + ADAA, DC blocker
- [x] Tone stack (SVF), generated cabinets + custom IR convolution
- [x] Delay, reverb, safety limiter, parameter smoothing, meters
- [x] Regression tests for every artefact in section 3 (45 tests in total)
- [x] Offline `render` example (process a DI `.wav`) and `bench` example
- [x] First version of the tube logo (`docs/assets/logo.svg`)
- [ ] Listening test with real DI recordings; tune voicings and cabinets by ear

### Phase 2 — Web version (`crates/klirr-wasm`, `web/`)
- [x] C-ABI WASM exports + AudioWorklet processor
- [x] UI in TypeScript (throttled knob messages, self-hosted fonts, settings remembered locally)
- [x] Device + channel selection, output selection with Firefox/Safari notice
- [x] Drag-and-drop IR loading
- [x] Klirr word mark + tube logo in the UI (first version; the tube glows while engaged)
- [x] Vitest unit tests (incl. the real wasm module), web CI job
- [x] Playwright smoke test with Chrome's fake microphone
- [x] Latency panel: live browser-reported breakdown, loopback measurement (on request only),
  dropout counter (audio clock vs wall clock)
- [ ] Test with a real guitar in Chrome, Edge and Firefox; check latency figures

### Phase 3 — Public launch
- [ ] Trademark / GitHub / crates.io / domain check for "Klirr"
- [ ] Cloudflare deployment (static assets only), preview deploys per PR
- [ ] Make the GitHub repository public

### Phase 4 — Desktop app
- [x] Native audio host (`crates/klirr-native`, `cpal`): WASAPI, optional ASIO, Core Audio,
  ALSA/JACK; `devices`, `measure` (loopback probe, same as the web) and `play` commands
- [ ] Measure native vs browser latency on real hardware (WASAPI, ASIO)
- [ ] Optional WASAPI exclusive backend (raw WASAPI, Windows only)
- [ ] Tauri app with the `cpal` audio backend
- [ ] Release workflow → GitHub Releases, download button on the site
- [ ] Code signing

### Later ideas
- Presets (local first; optionally shared via a small Worker API with KV/D1)
- Tuner, IR reverb, more amp voicings, your own recorded cabinet IRs
- TypeScript 7 once typescript-eslint supports it (planned for 7.1). 7.0 passes typecheck,
  tests, build and smoke test, but has no API for typescript-eslint; Dependabot skips 7.0.x.

## 9. Open questions

- Is "Klirr" free to use? (EUIPO/USPTO, GitHub, crates.io, npm, domains)
- Steinberg ASIO SDK license for the Windows release: GPLv3 for the ASIO build (see
  section 6) or Steinberg's proprietary agreement. Decide before the first Windows release.
- Code-signing route for Windows (SignPath vs. Azure Trusted Signing).
