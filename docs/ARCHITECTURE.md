# Echoless architecture

English | [简体中文](ARCHITECTURE.zh-CN.md)

```
                    ┌────────────────────────── echoless run (CLI process) ─────────────────────────┐
mic (cpal) ─────────►  near ring ─► near_delay ─┐                                                    │
                    │                           ├─► 10 ms frame loop ─► processor chain ─► output   │
system audio ───────►  far ring ────────────────┘        │                (AEC engine)      (cpal)  │
 (per-OS capture)   │                                    ├─► status JSONL (stdout)                  │
                    │   stdin JSONL control ─────────────┘                                          │
                    └───────────────────────────────────────────────────────────────────────────────┘
                                            ▲                                   ▲
                       Tauri app spawns & supervises              virtual device (VB-CABLE /
                       (start/stop, hot controls, events)          BlackHole / PipeWire null sink)
```

The desktop app never touches audio itself: it spawns the CLI as a sidecar,
writes a TOML config, reads the JSONL status stream, and sends hot-control
commands over stdin. Anything the GUI does can be done by hand with the CLI
(see [CLI.md](CLI.md)).

## Repository layout

| Path | What it is |
|---|---|
| `crates/echoless-cli` | The `echoless` binary: realtime pipeline, device I/O (cpal), delay probe, doctor, NVAFX installer |
| `crates/echoless-core` | Config model (TOML), frame/chain primitives, platform defaults |
| `crates/echoless-processors` | Engine implementations behind one trait: `aec3`, `localvqe`, `nvidia_afx_aec`, `passthrough` |
| `crates/echoless-audio-io` | WAV / sample-format helpers shared by offline and diagnostics |
| `crates/echoless-paths` | Brand data dir resolution (models, downloads) |
| `aec3/` | **WebRTC AEC3 in Rust** — its own cargo workspace, BSD-3-Clause, see below |
| `app/` | Tauri desktop app: React front-end (`app/src`), Rust shell (`app/src-tauri`) |
| `tools/macos-process-tap-poc/` | Swift helper for macOS system-audio capture |
| `configs/example.toml` | Annotated pipeline config example |

## Capture: per-OS reference sources

The far-end reference must be *exactly what the speakers play*.

- **Windows** — WASAPI loopback on the render endpoint. No driver, no helper.
- **macOS (14.4+)** — a Swift helper (`echoless-process-tap`) creates a Core
  Audio **Process Tap** over all processes (excluding Echoless itself) and
  streams interleaved f32 PCM to the CLI over stdout. The stream starts with a
  16-byte `ELTP` header (magic, version, sample rate, channels); if the tap
  rate differs from the pipeline rate the CLI resamples linearly. The helper
  also exposes a no-prompt TCC permission preflight used by `doctor`.
- **Linux (PipeWire/Pulse)** — the monitor source of the sink being played
  (`<sink>.monitor`). No driver and no helper; enumerated through cpal.

Near-end (mic) and output are plain cpal streams everywhere; device sample
rates are adapted to the pipeline rate transparently.

## Pipeline

Fixed-rate (default 48 kHz), fixed-frame (default 10 ms) loop:

1. Pull one frame from the near ring (after `near_delay_ms` alignment —
   macOS defaults to 25 ms to compensate the tap's head start, others 0).
2. Pull the matching frame from the far ring.
3. Feed both to the processor chain (usually a single AEC engine).
4. Push the processed frame to the output device.
5. At the status interval, emit a status JSON line (levels, latency, engine
   metrics). The interval is configurable; the app requests 80 ms, and the CLI
   defaults to 1000 ms under `--status-json`/`--verbose`.

Engines declare themselves in a manifest (`echoless processors --json`):
kind, platforms, parameters with types/defaults. The GUI renders its controls
from that manifest, so adding an engine parameter is a backend-only change.

**Hot controls vs restart.** Output level, near delay, AEC3 NS/AGC, LocalVQE
noise gate, bypass and diagnostics recording apply live over stdin. Device,
engine, sample-rate or model changes restart the sidecar (the GUI does this
automatically).

**Bypass keep-warm.** "Power off" sends `set_bypass true`: frames skip the
engine (15 ms crossfade) but capture/output keep running and the engine keeps
its adaptation state, so switching back on is instant and glitch-free.

**Clock-drift rate matching (`output_rate_match`, default on).** The pipeline
runs on the capture (mic) clock, the output device drains on its own clock, and
the far ring is filled on the render/loopback clock. These clocks are
independent and drift even at an identical nominal rate (all 48 kHz, no format
resampling) — virtual audio devices drift most. Uncompensated, the drift slowly
empties the output ring (underruns → zero-fill clicks) or overflows it. The
pre-T3 path dropped stale reference frames (`skip_stale`) and zero-filled
underruns; both are audible and knock the far/near frames out of alignment,
degrading AEC.

Instead of dropping, the engine resamples to absorb the drift. On the fast path
where the device rate equals the pipeline rate (otherwise a fixed-ratio
resampler already runs), a PI controller (`RateController`) reads the output
ring's water level, compares it to a 2-frame setpoint, and emits a ratio `trim`
clamped to ±3% (with anti-windup); the resampler then runs at
`base_ratio · (1 + trim)` to steer occupancy back to the setpoint. The far path
uses the analogous resampler in place of `skip_stale`. The key detail is a
**half-frame soft deadband**: while the water-level error stays within ±½ frame
the effective error is zero and `trim` decays to 0, so a device that only
jitters stays bit-exact and is never resampled — only sustained drift past
½ frame pulls the ratio. The ±3% clamp keeps the pitch shift inaudible.

## AEC3 (`aec3/`)

A Rust port of WebRTC's AEC3 (lineage: WebRTC C++ →
the [sonora](https://github.com/dignifiedquire/sonora) port, pinned at
`aacadf0` and maintained independently). Kept as a separate cargo workspace so
its 700+ upstream-derived tests run against the port unchanged. Echoless-specific modifications, all guarded
by config:

- **Delay hold** — once the delay estimator reaches confidence, the estimate
  is pinned and re-search during reference silence is suppressed. Upstream
  AEC3 assumes delay can wander (acoustic paths); a loopback reference has a
  stable delay, and re-searching during reference silence was measured to
  degrade long-session echo attenuation badly (with `delay_hold` off, a long
  run drops 9.6→5.6 dB; see
  `crates/echoless-processors/tests/echo_cancellation.rs`).
- **Render activity gate** and an explicit `aec3_delay_blocks` runtime metric.
- Externally applied near-delay bias replaces negative-delay handling.

## Engine distribution

| Engine | Runtime | Models |
|---|---|---|
| AEC3 | compiled in | — |
| LocalVQE | native library **bundled with the app** (`liblocalvqe`) | GGUF downloaded from [Hugging Face](https://huggingface.co/LocalAI-io/LocalVQE), SHA-256 pinned per file |
| NVAFX | downloaded from this repo's GitHub Releases (common runtime + per-GPU-arch model zip, SHA-256 manifest) | idem |

CI pins the LocalVQE source revision (`LOCALVQE_REF`) and builds the native
library per-OS; `app/scripts/prepare-tauri-assets.mjs` stages it into the
bundle.

## GUI ↔ CLI contract

- **One-shot commands** (`devices`, `doctor audio`, `processors`,
  `config validate`, `probe-delay`, `nvafx …`) are spawned with `--json` and
  parsed from stdout.
- **`run`** is long-lived: JSONL status on stdout (first line `started`,
  advertising `supported_controls`), human logs on stderr, control commands
  on stdin (see [CLI.md](CLI.md#runtime-controls)).
- **`probe-delay`** additionally emits progress JSONL on stderr
  (`beep_train_start` with the beep cadence) so the GUI can sync its progress
  indicator to real playback.
