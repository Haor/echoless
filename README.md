# Echoless

[![Build](https://github.com/Haor/echoless/actions/workflows/build.yml/badge.svg)](https://github.com/Haor/echoless/actions/workflows/build.yml)

**Real-time acoustic echo cancellation for open-speaker setups.**
Echoless removes what your speakers are playing from what your microphone
hears, and hands the clean voice to Discord / VRChat / any voice app through a
virtual microphone — so the people on the other end never hear themselves (or
your game audio) echoed back.

```
far-end reference   system audio loopback (what your speakers play)
near-end capture    microphone (your voice + speaker echo + room)
output              echo-cancelled voice → virtual mic → voice app
```

No special hardware required: a USB or built-in mic, ordinary speakers, and a
virtual audio device.

## Features

- **Three interchangeable AEC engines** — switch live, per taste and hardware
  (see [Engines](#engines) below)
- **System-audio reference with no extra cabling** — WASAPI loopback
  (Windows), Core Audio Process Tap (macOS 14.4+), PipeWire monitor (Linux)
- **Delay probe** — plays a short beep train and measures your actual
  mic-to-reference delay, then applies it (cross-correlation, ~ms accuracy)
- **Power-off = bypass, not mute** — the mic path never dies; turning AEC off
  keeps your voice flowing untouched
- **Diagnostics recording** — capture mic / reference / output tracks to WAV
  for troubleshooting
- **Desktop app + standalone CLI** — the Tauri GUI drives the same `echoless`
  CLI you can script yourself ([CLI guide](docs/CLI.md))

## Engines

### AEC3 (default)

The echo canceller from the [WebRTC](https://webrtc.googlesource.com/src/)
audio processing module — the same algorithm family used by Chrome and Meet.
Adaptive linear filtering with delay estimation, non-linear residual
suppression, and optional noise suppression / AGC. CPU-light, 48 kHz native.

Echoless ships a Rust port of AEC3 (in [`aec3/`](aec3/), BSD-3-Clause) with
small modifications for the open-speaker use case: the estimated delay is
held once confidence is reached instead of being re-searched during silence,
which measurably improves long-session stability on loopback-style paths.

### LocalVQE (neural, experimental)

[LocalVQE](https://huggingface.co/LocalAI-io/LocalVQE) (Apache-2.0, by Richard
Palethorpe and Claude) is a family of compact neural models for echo
cancellation, noise suppression and dereverberation of 16 kHz speech, running
in real time on ordinary CPUs. It is a streaming, CPU-tuned derivative of
Microsoft's **DeepVQE** at roughly a tenth of the parameter count. Echoless
resamples its 48 kHz pipeline to 16 kHz and back around the model
automatically.

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
accelerated on RTX Tensor Cores. In our testing it produces the cleanest
voice of the three engines. Requires Windows and a Turing-or-newer RTX GPU;
the runtime (~1 GB) and per-architecture models are downloaded on first
setup. *AEC powered by NVIDIA Maxine.*

## Platforms

| OS | Reference capture | Virtual mic | Status |
|---|---|---|---|
| Windows 10 / 11 | WASAPI loopback | [VB-CABLE](https://vb-audio.com/Cable/) | supported |
| macOS 14.4+ | Core Audio Process Tap | [BlackHole 2ch](https://github.com/ExistentialAudio/BlackHole) | supported |
| Linux | monitor source | `pactl` null sink — no driver needed | experimental, not yet verified on hardware |

## Install

Grab the installer for your OS from
[Releases](https://github.com/Haor/echoless/releases), or
[build from source](#building-from-source).

You also need a virtual audio device (see the table above). The app's
**MIC SETUP** wizard checks for one and walks you through installing it.

## Quick start

1. Install a virtual audio device (VB-CABLE / BlackHole; on Linux one
   `pactl load-module module-null-sink …` command — the wizard shows it).
2. In Echoless: **INPUT** = your microphone, **OUTPUT** = the virtual device.
   The reference defaults to system audio.
3. Flip **POWER** on. On macOS, grant system-audio recording when prompted.
4. In your voice app, select the virtual device as the microphone
   (`CABLE Output` / `BlackHole 2ch` / `Monitor of Echoless-Output`).
5. If echo remains, run **RUN PROBE** on the Advanced page (~15 s of beeps)
   to measure and apply your exact device delay.

Power **OFF** is a bypass: AEC is skipped but your mic keeps flowing, so the
voice app never loses its input.

## CLI

Everything the GUI does goes through the `echoless` CLI, which works
standalone — `devices`, `run`, `probe-delay`, `offline` (WAV-in/WAV-out),
`doctor`, all with `--json` for scripting:

```bash
echoless devices --json
echoless run --mic default --reference system --output "CABLE Input"
echoless offline --mic mic.wav --reference ref.wav --out clean.wav --chain aec3
```

See **[docs/CLI.md](docs/CLI.md)** for the full command reference, the
runtime-control protocol, and configuration format. Architecture notes live
in **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)**.

## Building from source

Prereqs: Rust (stable), Node 22 + pnpm, and on macOS Xcode CLT + Swift
(for the Process Tap helper).

```bash
# CLI
cargo build --release                    # target/release/echoless

# macOS system-audio helper
tools/macos-process-tap-poc/build.sh

# Desktop app (dev)
cd app && pnpm install && pnpm tauri dev

# Desktop app (bundle)
cd app && pnpm tauri build
```

`cargo test --workspace` runs the test suite; the AEC3 fork has its own
workspace (`cd aec3 && cargo test`).

## Citing

If you use the LocalVQE engine in academic work, please cite LocalVQE via its
[`CITATION.cff`](https://github.com/localai-org/LocalVQE) and the upstream
DeepVQE paper:

```bibtex
@inproceedings{indenbom2023deepvqe,
  title     = {DeepVQE: Real Time Deep Voice Quality Enhancement for Joint
               Acoustic Echo Cancellation, Noise Suppression and Dereverberation},
  author    = {Indenbom, Evgenii and Beltr{\'a}n, Nicolae-C{\u{a}}t{\u{a}}lin
               and Chernov, Mykola and Aichner, Robert},
  booktitle = {Interspeech}, year = {2023},
  doi       = {10.21437/Interspeech.2023-2176}
}
```

## Acknowledgements & licenses

- **Echoless** is MIT licensed ([LICENSE](LICENSE)).
- **AEC3** (`aec3/`) derives from the
  [WebRTC project](https://webrtc.org)'s audio processing module —
  BSD-3-Clause ([aec3/LICENSE](aec3/LICENSE)).
- **LocalVQE** models and runtime are Apache-2.0, © Richard Palethorpe and
  Claude (Anthropic). Not for emergency or safety-critical use (see the
  [model card](https://huggingface.co/LocalAI-io/LocalVQE)).
- **NVAFX** uses the NVIDIA Maxine Audio Effects SDK under the
  [NVIDIA SDK License](https://developer.nvidia.com/downloads/maxine-sdk-license);
  the redistributed runtime/model packages are for installation by Echoless
  only. NVIDIA and Maxine are trademarks of NVIDIA Corporation.
- Virtual audio thanks: [VB-CABLE](https://vb-audio.com/Cable/) (donationware)
  and [BlackHole](https://github.com/ExistentialAudio/BlackHole) (GPL-3.0,
  used as an external device, not linked).
