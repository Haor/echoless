# ARCH-2 Cleanup Plan

Audit source: `docs/audit/CODE_AUDIT.md`

## Scope

Resolve the dead realtime abstraction in `echoless-core` without changing the current realtime runtime or GUI/CLI coupling.

## Decision

Use CODE_AUDIT's option A for now: remove the unused `ControlApi` trait and `run_realtime` stub from `echoless-core`.

The current product runtime is the CLI/sidecar path. Keeping a public core realtime trait with no implementation makes the architecture look more unified than it is and can mislead frontend/runtime work.

## Edits

1. Remove `ControlApi` and `run_realtime` from `echoless-core`.
2. Remove the now-unused `crossbeam-channel` dependency from `echoless-core`.
3. Update README and crate metadata so `echoless-core` is described as config/offline/shared pipeline utilities, not a realtime control surface.
4. Leave research documents unchanged because they are historical architecture blueprints, not current implementation contracts.

## Verification

- `rg -n "ControlApi|run_realtime" crates README.md`
- `cargo fmt -p echoless-core --check`
- `cargo test -p echoless-core --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
- `git diff --check`
- `graphify update echoless`
