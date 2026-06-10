# QUAL-5 Cleanup Plan

Audit source: `docs/audit/CODE_AUDIT.md`

## Scope

Close the duplicated DSP utility part of `QUAL-5` without changing realtime audio behavior.

## Targets

1. Consolidate processors crate copy-or-zero helpers.
   - Current duplicates: `chain.rs`, `localvqe.rs`, `nvafx.rs`.
   - Target helper: one crate-local function that copies the overlapping samples and fills the remaining destination with silence.
   - Behavior lock: tests for shorter source, longer source, empty source, and exact length.

2. Consolidate CLI RMS dBFS helpers.
   - Current duplicates: realtime status/diagnostics RMS math and `probe-delay` RMS math.
   - Target helper: one CLI-local dBFS utility that supports both precomputed sum-of-squares and raw sample slices.
   - Behavior lock: tests for silence, full-scale, half-scale, empty input, and floor behavior.

3. Do not merge resamplers in this pass.
   - `realtime/resample.rs` implements stateful device-boundary linear resampling across callbacks.
   - `chain.rs` primarily uses persistent rubato FFT resamplers for processor-boundary SRC and only keeps a local linear fallback.
   - These are not the same abstraction today; forcing one shared helper would risk hiding state/latency differences.

## Verification

- `cargo test -p echoless-processors --locked`
- `cargo test -p echoless-cli realtime::stats --locked`
- `cargo test -p echoless-cli probe_delay --locked`
- `cargo fmt -p echoless-processors -p echoless-cli --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
- `git diff --check`
- `graphify update echoless`
