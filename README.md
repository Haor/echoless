# Echoless

English | [简体中文](README.zh-CN.md)

**Real-time acoustic echo cancellation for open-speaker setups.**
Echoless removes what your speakers are playing from what your microphone
hears, and hands the clean voice to Discord / any voice app through a virtual
microphone — so the people on the other end never hear themselves (or your
game audio) echoed back.

```
far-end reference   system audio loopback (what your speakers play)
near-end capture    microphone (your voice + speaker echo + room)
output              echo-cancelled voice → virtual mic → voice app
```

No special hardware required: a USB or built-in mic, ordinary speakers, and a
virtual audio device.

## Interface Preview

<p align="center">
  <img width="800" alt="Echoless main interface demo" src="https://github.com/user-attachments/assets/c4d846f9-9a7b-4b2d-91ab-945ab9e0ed26" />
</p>

## Features

- **Three interchangeable AEC engines** — switch live, per taste and hardware
  (see [Engines](#engines) below)
- **System-audio reference, no extra cabling** — WASAPI loopback (Windows),
  Core Audio Process Tap (macOS 14.4+), PipeWire monitor (Linux)
- **Delay probe** — plays a short beep train and measures your actual
  mic-to-reference delay, then applies it (cross-correlation, ~ms accuracy)
- **Power-off = bypass, not mute** — the mic path never dies; turning AEC off
  passes your voice through untouched
- **Diagnostics recording** — capture mic / reference / output tracks to WAV
  for troubleshooting
- **Desktop app + standalone CLI** — the Tauri GUI drives the same `echoless`
  CLI you can script yourself ([CLI guide](docs/CLI.md))

## Engines

### AEC3 (default)

The echo canceller from the [WebRTC](https://webrtc.googlesource.com/src/)
audio processing module — the same algorithm family used by Chrome and Google
Meet. Adaptive linear filtering with delay estimation, non-linear residual
suppression, and optional noise suppression / AGC. CPU-light, 48 kHz native.

Echoless bundles a Rust port of AEC3 (in [`aec3/`](aec3/), BSD-3-Clause) with
a small tweak for the open-speaker use case: the estimated delay is held once
confidence is reached instead of being re-searched during silence, which
measurably improves long-session stability on loopback-style paths.

### LocalVQE (neural, experimental)

[LocalVQE](https://github.com/localai-org/LocalVQE), by
[Local AI](https://huggingface.co/LocalAI-io), is a family of compact neural
models for echo cancellation, noise suppression and dereverberation of 16 kHz
speech, running in real time on ordinary CPUs. It is a streaming, CPU-tuned
derivative of Microsoft's **DeepVQE** at roughly a tenth of the parameter
count. The runtime code is Apache-2.0; the official models are published
separately on [Hugging Face](https://huggingface.co/LocalAI-io/LocalVQE). The
models run at 16 kHz — Echoless resamples 48↔16 kHz on both sides of its
pipeline, splicing the model transparently into the signal chain.

| Model | Does | Params |
|---|---|---:|
| v1.3 *(default)* | AEC + noise suppression + dereverb | 4.8 M |
| v1.2 | AEC + NS + dereverb, ~¼ the CPU cost | 1.3 M |
| v1.4-AEC | echo removal only — keeps voice, noise and room | 203 K |

The inference runtime ships inside the app; model weights are downloaded from
Hugging Face on demand (SHA-256 verified). The overview page's NOISE switch
maps to the model choice: **on = v1.3, off = v1.4** (pure AEC).

### NVAFX / RTX AEC (Windows + RTX GPU)

Acoustic echo cancellation from the
[NVIDIA Maxine](https://developer.nvidia.com/maxine) Audio Effects SDK,
accelerated on RTX Tensor Cores. In our testing it preserves the voice best
while suppressing loud echo, but leaves some residual echo — we recommend
chaining a denoiser after it (e.g. NVIDIA Broadcast). Requires Windows and a
Turing-or-newer RTX GPU; the runtime (~1 GB) and per-architecture models are
downloaded on first setup. *AEC powered by NVIDIA Maxine.*

## Platforms

| OS | Reference capture | Virtual mic | Status |
|---|---|---|---|
| Windows 10 / 11 | WASAPI loopback | [VB-CABLE](https://vb-audio.com/Cable/) | supported |
| macOS 14.4+ | Core Audio Process Tap | [BlackHole 2ch](https://github.com/ExistentialAudio/BlackHole) | supported |
| Linux | monitor source | `pactl` null sink — no driver needed | experimental, not yet verified on hardware |

## Tech stack

- **Core / CLI** — Rust (cargo workspace: `echoless-core` / `echoless-audio-io`
  / `echoless-processors` / `echoless-cli`).
- **Audio I/O** — [cpal](https://github.com/RustAudio/cpal) for device capture
  and playback, `ringbuf` lock-free ring buffers,
  [rubato](https://github.com/HEnquist/rubato) for resampling; reference
  capture uses native platform APIs (WASAPI loopback / Core Audio Process Tap /
  PipeWire monitor).
- **AEC engines** — AEC3 (standalone Rust workspace, `aec3/`), LocalVQE
  (C + ggml, loaded over `libloading` FFI), NVAFX (NVIDIA Maxine Audio Effects
  SDK).
- **Desktop app** — [Tauri v2](https://tauri.app) (Rust backend) + React 18 +
  TypeScript, built with Vite / tested with Vitest.
- **macOS system audio** — a Swift Process Tap helper
  (`tools/macos-process-tap-poc/`).

## Install

Download the installer for your platform from
[Releases](https://github.com/Haor/echoless/releases) (`.dmg` / `.exe` /
`.deb` / `.AppImage`), or [build from source](#building-from-source).

> **macOS**: if the first launch says the app is "damaged" or won't open, run
> this in a terminal and try again:
>
> ```bash
> sudo xattr -rd com.apple.quarantine /Applications/Echoless.app
> ```

You also need a virtual audio device (see the table above). The app's
**MIC SETUP** wizard checks for one and walks you through installing it.

## Quick start

1. Install a virtual audio device ([VB-CABLE](https://vb-audio.com/Cable/) /
   [BlackHole](https://github.com/ExistentialAudio/BlackHole); on Linux one
   `pactl load-module module-null-sink …` command — the wizard shows it).
2. In Echoless: **INPUT** = your microphone, **OUTPUT** = the virtual device.
   The reference defaults to system audio.
3. Flip **POWER** on. On macOS, grant system-audio recording when prompted.
4. In your voice app, select the virtual device as the microphone
   (`CABLE Output` / `BlackHole 2ch` / `Monitor of Echoless-Output`).
5. If echo remains, run **RUN PROBE** on the Advanced page (~15 s of beeps)
   to measure and apply your exact device delay.

Power **OFF** is a bypass: AEC is skipped but your mic passes through
untouched, so the voice app never loses its input.

The bottom **VOL** is output gain: hover and scroll to adjust, in dB — 50 is
unity gain (0 dB, passed through as-is), ranging from 0 (mute) to about
+9.5 dB (≈3×); click to mute / unmute. Use it to compensate the voice level.

## CLI

Echoless ships a standalone `echoless` CLI (the GUI is built on it) —
`devices`, `run`, `probe-delay`, `offline` (WAV-in/WAV-out), `doctor`, all
with `--json` for scripting:

```bash
echoless devices --json
echoless run --mic default --reference system --output "CABLE Input"
echoless offline --mic mic.wav --reference ref.wav --out clean.wav --chain aec3
```

See **[docs/CLI.md](docs/CLI.md)** for the full command reference, the
runtime-control protocol, and configuration format. Architecture notes live
in **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)**.

## Building from source

Prereqs: Rust (stable), Node 22 with Corepack, and on macOS Xcode CLT + Swift
(for the Process Tap helper). The app workspace pins pnpm via
`app/package.json`; run `corepack enable` so Node uses that pnpm.
`app/pnpm-workspace.yaml` sets `minimumReleaseAge: 10080` (7 days): an npm
dependency must have been published for at least 7 days before it can be
installed, shrinking the exposure window to newly-published-package
supply-chain attacks.

```bash
# CLI
cargo build --release                    # target/release/echoless

# macOS system-audio helper
tools/macos-process-tap-poc/build.sh

# Desktop app (dev)
cd app && corepack enable && pnpm install && pnpm tauri dev

# Desktop app (bundle)
cd app && pnpm tauri build
```

`cargo test --workspace` runs the test suite; the AEC3 engine is a separate
workspace (`cd aec3 && cargo test`).

## Acknowledgements & licenses

- **Echoless** is MIT licensed ([LICENSE](LICENSE)).
- **AEC3** (`aec3/`) implementation references the
  [WebRTC project](https://webrtc.org)'s audio processing module, based on
  [sonora](https://github.com/dignifiedquire/sonora)'s Rust port —
  BSD-3-Clause ([aec3/LICENSE](aec3/LICENSE)).
- **LocalVQE** runtime code and models are Apache-2.0, © Local AI, based on
  the DeepVQE and GTCRN research (academic citation info in its
  [repository](https://github.com/localai-org/LocalVQE)'s `CITATION.cff`). Not
  for emergency or safety-critical use (see the
  [model card](https://huggingface.co/LocalAI-io/LocalVQE)).
- **NVAFX** uses the NVIDIA Maxine Audio Effects SDK under the
  [NVIDIA SDK License](https://developer.nvidia.com/downloads/maxine-sdk-license);
  the redistributed runtime / model packages are for installation by Echoless
  only. NVIDIA and Maxine are trademarks of NVIDIA Corporation.
- Virtual audio thanks: [VB-CABLE](https://vb-audio.com/Cable/) and
  [BlackHole](https://github.com/ExistentialAudio/BlackHole).

Full third-party license texts (WebRTC BSD-3, LocalVQE Apache-2.0 + NOTICE,
NVIDIA Maxine SDK) are collected in
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).
