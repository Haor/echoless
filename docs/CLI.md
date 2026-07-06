# `echoless` CLI reference

English | [简体中文](CLI.zh-CN.md)

The CLI is fully standalone — the desktop app is a front-end for it. Build
with `cargo build --release` (binary at `target/release/echoless`), or use
the copy shipped next to the app executable.

Every inspection command accepts `--json` for machine-readable output.

```
echoless <COMMAND>

  offline      process mic.wav + ref.wav through the chain → out.wav
  processors   list engines and their parameter manifests
  devices      list audio devices and reference sources
  doctor       environment diagnostics
  config       config file tools (validate)
  run          realtime pipeline
  probe-delay  measure mic↔reference alignment delay
  nvafx        NVIDIA RTX AEC runtime tools (doctor / install / offline)
```

## devices

```bash
echoless devices --json          # inputs, outputs, reference_sources
echoless devices --json --fast   # skip slow queries (GUI refresh path)
```

Device selectors used by other commands accept `default`, a list index, a
name fragment, or the `stable_id` from this output. Reference selectors:
`system` (OS loopback/tap), `none`, `output:<name>`, `input:<name>`.

## run

```bash
echoless run --mic default --reference system --output "CABLE Input"
echoless run --config my.toml --status-json
```

Key flags (all override the config file): `--mic`, `--reference`,
`--output`, `--sample-rate`, `--frame-ms`, `--reference-channels mono|stereo`,
`--near-delay-ms`, `--output-level 0..100` (50 = unity),
`--processor aec3|localvqe|nvidia_afx_aec|…`, `--ns/--no-ns`, `--ns-level`,
`--tail-ms`, `--verbose`, `--status-json`,
`--diagnostic-dir <DIR> [--diagnostic-seconds N]`.

With `--status-json`, stdout is JSONL: first a `started` event (negotiated
devices, `supported_controls`, resampling info), then periodic status frames
(dBFS levels, latency, engine metrics), plus acknowledgement events for
runtime controls. Human logs go to stderr.

### Runtime controls

While `run` is active, write one JSON object per line to stdin:

| Command | Payload | Effect |
|---|---|---|
| `set_output_level` | `{"cmd":"set_output_level","level":50}` | live output gain (0 mute · 50 unity · 100 ≈ 3×) |
| `set_near_delay_ms` | `{"cmd":"set_near_delay_ms","ms":25}` | live near/far alignment |
| `set_bypass` | `{"cmd":"set_bypass","bypass":true}` | skip the engine, keep it warm (15 ms crossfade) |
| `set_initial_delay_ms` | `{"cmd":"set_initial_delay_ms","ms":8}` | AEC3 initial delay hint |
| `set_aec3_ns` | `{"cmd":"set_aec3_ns","ns":true,"ns_level":"high"}` | AEC3 noise suppression |
| `set_aec3_agc` | `{"cmd":"set_aec3_agc","agc":false}` | AEC3 AGC |
| `set_localvqe_noise_gate` | `{"cmd":"set_localvqe_noise_gate","enabled":true,"threshold_dbfs":-45}` | LocalVQE output gate |
| `start_diagnostics` | `{"cmd":"start_diagnostics","dir":"...","max_seconds":30}` | record mic/ref/out WAVs |
| `stop_diagnostics` | `{"cmd":"stop_diagnostics"}` | finalize the recording session |

The `started` event's `supported_controls` array is authoritative for what a
given binary accepts.

## probe-delay

Measures the true near↔far delay by playing a beep train through the
speakers while recording both paths, then cross-correlating per beep
(0.5 ms envelope resolution):

```bash
echoless probe-delay --json --mic default --reference system --output "CABLE Input"
```

The command accepts `default` selectors, but its clap defaults are intentionally
macOS-oriented (`MacBook Pro...` / `BlackHole 2ch`) for the maintainer's local
calibration rig. Portable scripts should pass `--mic`, `--reference`, and
`--output` explicitly rather than relying on those defaults.

Stops nothing by itself — don't run it while another `run` holds the devices.
Flags: `--beeps N` (12), `--startup-delay S` (4), `--volume 0..1` (0.35),
`--out-dir/--keep-session/--keep-beep`, `--analyze-only <session>`.

JSON result includes `recommended_near_delay_ms` (measured lag + 8 ms
safety), per-beep lags, stddev/drift and warnings. In `--json` mode progress
markers are emitted on stderr as JSONL (`beep_train_start` with the exact
beep cadence). Supported on macOS, Windows and Linux (Linux maps the monitor
reference back to its sink for playback).

## offline

WAV-in/WAV-out processing with any engine — useful for A/B tests and CI:

```bash
echoless offline --mic mic.wav --reference ref.wav --out clean.wav --chain aec3
echoless offline --mic mic.wav --reference ref.wav --out clean.wav --config my.toml
```

`offline` validates processor topology and WAV-in/WAV-out behavior. It does not
simulate the realtime device boundary, `near_delay_ms`, bypass crossfade, queue
backpressure, or device sample-rate conversion path, so it should not be used as
an exact live-latency or live-routing benchmark.

## doctor / processors / config

```bash
echoless doctor audio --json    # virtual device present? reference OK? permissions?
echoless processors --json      # engine manifest (params, platforms, defaults)
echoless config validate my.toml
```

## nvafx (Windows + RTX)

```bash
echoless nvafx doctor --json               # GPU / driver / VC++ / runtime checks
echoless nvafx download-install --json     # fetch runtime + model for this GPU (~1 GB)
echoless nvafx install --common-zip <common.zip> --model-zip <model.zip>  # install from local zips
echoless nvafx offline --mic ... --reference ... --out ...
```

## Configuration file

`run`/`offline` take a TOML pipeline config — see
[`configs/example.toml`](../configs/example.toml) for a commented example
covering devices, pipeline (`sample_rate`, `frame_ms`, `near_delay_ms`,
`reference_channels`) and the `[[chain]]` engine blocks with per-engine
parameters.

## Environment variables

| Variable | Purpose |
|---|---|
| `ECHOLESS_PROCESS_TAP_HELPER` | path to the macOS system-audio helper (otherwise: next to the binary, then `tools/macos-process-tap-poc/.build/…` upward from CWD) |
| `ECHOLESS_LOCALVQE_LIBRARY` | path to `liblocalvqe` (otherwise: app bundle resources, then the brand data dir) |

Model/data directory: `~/Library/Application Support/Echoless` (macOS),
`%LOCALAPPDATA%\Echoless` (Windows), `~/.local/share/echoless` (Linux).
