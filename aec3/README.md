# aec3 — Echoless's AEC3 engine

English | [简体中文](README.zh-CN.md)

A pure-Rust port of WebRTC AEC3 (Acoustic Echo Cancellation), maintained
independently by Echoless. BSD-3-Clause; see `LICENSE`.

## Layout

A standalone cargo workspace (edition 2024), consumed by the main workspace's
`echoless-processors` via a path dependency (feature `aec3`):

| crate | contents |
|---|---|
| `aec3-apm` | Audio Processing Module top level (echo cancel + NS + AGC2 pipeline) |
| `aec3-core` | AEC3 proper (delay estimator / erle / suppressor …) |
| `aec3-ns` / `aec3-agc2` | noise suppression / AGC2 |
| `aec3-common-audio` / `aec3-fft` / `aec3-simd` | shared audio primitives / FFT / SIMD |

## Changes relative to upstream

- **Delay-inertia tweaks (P4)**: an underrun no longer subtracts from the delay
  or triggers a soft reset; `estimate_delay` gains a render silence gate (the
  consistent counter is not incremented while gated). Each site is marked with
  an `Echoless:` comment; negative-direction search is implemented via a
  `near_delay` bias in the upper layer (zero changes in this workspace).
- **Stereo config derivation**: when a custom AEC3 config is injected, the
  multichannel variants are derived from that base (preserving stereo-specific
  tuning) rather than falling back to the mono default (`aec3-apm`).
- **Trimming**: upstream's C++ reference implementation, fuzzing, ffi/sys
  bindings, benchmarks, examples, CI/release configuration, and the 10M of test
  audio have all been removed; in-src unit tests and proptests are kept.

## Testing

```bash
cd aec3 && cargo test --workspace
```

All modifications are marked in place with `Echoless:` comments in the source;
for the integration contract, see how `crates/echoless-processors` calls into
`aec3-apm`.
