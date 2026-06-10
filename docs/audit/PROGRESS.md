# Echoless Audit Progress

Audit source: `docs/audit/CODE_AUDIT.md`

Baseline branch: `phase-0/green-baseline`

Last updated: 2026-06-10

## Status Ledger

| ID | Phase | Status | Closing commit | Verification |
| --- | --- | --- | --- | --- |
| P0.0 | green baseline | done | `7406a14` | `cargo clippy --workspace --all-targets --locked -- -D warnings`; `(cd app/src-tauri && cargo clippy --all-targets --locked -- -D warnings)`; `cargo test --workspace --locked`; `(cd app/src-tauri && cargo build --locked)` |
| TEST-1 | P0 safety net | done | `ee8999a` | CI now includes Tauri backend clippy/build, frontend `tsc --noEmit`, and `pnpm tauri build --debug --no-bundle --ci`; local Tauri build smoke passed |
| TEST-2 | P0 safety net | done | `ee8999a` | CI now includes first-party Rust fmt checks and root/app/vendor cargo-audit; local scoped fmt and cargo-audit runs passed |
| CFG-1 | P0 safety net | done | `ee8999a` | CI pins LocalVQE checkout to `de56a174d9662b65f404ec65ae8e4bc9712db0da` |
| RUNTIME-1 | P1 usable | done | `cdcddb8` | `started` event exposes `cli_version` and `supported_controls`; frontend guards diagnostics/output-level stdin commands; `cargo test -p echoless-cli runtime_control --locked`; `pnpm exec tsc --noEmit` |
| RUNTIME-2 | P1 usable | doing | `0d5e8cc` | Tauri `localvqe_assets` now reports native readiness/library path; `echoless_command` injects `ECHOLESS_LOCALVQE_LIBRARY` and native library search env; frontend gates LocalVQE READY on model + native runtime. Remaining: prove with real LocalVQE native assets inside macOS/Windows Tauri bundles |
| PKG-1 | P1 usable | doing | `0d5e8cc` | `bundle.externalBin`/`resources` configured; `pnpm prepare:tauri-assets` generates CLI sidecar, Process Tap helper resource, and LocalVQE resource copies when assets exist; `pnpm tauri build --debug --no-bundle --ci` passed locally. Remaining: full macOS/Windows installed-app bundle smoke |
| ARCH-1 | P2 split | todo | - | Mechanical split of `realtime.rs` and `main.rs` |
| PERF-1 | P3 realtime | todo | - | Zero-allocation processor-chain steady state |
| QUAL-1 | P3 realtime | todo | - | Stateful node-boundary resampling and stereo preservation |
| QUAL-3 | P3 realtime | todo | - | LocalVQE streaming buffer allocation/drain fix |
| QUAL-4 | P3 realtime | todo | - | Online waveform bucket aggregation |
| ROB-2 | P3 realtime | todo | - | Diagnostic writer finish/join hardening |
| SON-1 | P3 realtime | todo | - | Surface sonora process errors |
| SEC-1 | P4 hardening | todo | - | Validate `open_url` scheme or use opener plugin |
| SEC-2 | P4 hardening | todo | - | Prefer embedded nvafx pin hashes for default release |
| SEC-3 | P4 hardening | todo | - | Replace fixed temp TOML paths |
| SEC-4 | P4 hardening | todo | - | Remove implicit CWD LocalVQE library search |
| SEC-5 | P4 hardening | todo | - | Verify LocalVQE model downloads |
| SEC-6 | P4 hardening | todo | - | Resolve NVIDIA tools/system DLLs by safer paths |
| ROB-1 | P4 hardening | todo | - | Move long Tauri commands to blocking workers with timeouts |
| ROB-3 | P4 hardening | todo | - | Remove or recover poisoned `Mutex::lock().unwrap()` paths |
| ROB-4 | P4 hardening | todo | - | Replace silent `.lines().flatten()` handling with explicit error reporting |
| QUAL-2 | P4 hardening | todo | - | Harden `PipelineConfig::frame_size()` overflow behavior |
| SON-2 | P4 hardening | todo | - | Audit production-path `assert!` and profile sonora allocation behavior |
| SON-3 | P4 hardening | todo | - | Add vendor/sonora CI gate if fork remains vendored |
| DOC-1 | P4 hardening | todo | - | Centralize and document nvafx pin hash rotation |
| FE-1 | P5 frontend | todo | - | Pause idle waveform rAF loops and cache canvas sizing |
| FE-2 | P5 frontend | todo | - | Isolate live/health status rendering |
| LAT-1 | P5 frontend | todo | - | Fix or relabel user latency estimate |
| QUAL-5 | P5/P7 cleanup | todo | - | Consolidate duplicated DSP utilities |
| ARCH-2 | P6 architecture | todo | - | Decide and resolve dead `echoless-core` realtime abstraction |
| ARCH-3 | P6 architecture | todo | - | Decide subprocess hot-control vs in-process core path |
| FE-3 | P6 architecture | todo | - | Stop treating every config change as a full run restart |
| TEST-3 | P7 cleanup | todo | - | Add chain resampling/channel/output-level boundary tests |

## Notes

- `cargo fmt --all --check` currently reaches `vendor/sonora`, which is documented as a read-only third-party fork and has pre-existing rustfmt drift. P0.0 used scoped formatting checks for first-party root packages and `app/src-tauri`.
- Tauri clippy/build still reports the known `block v0.1.6` future-incompatibility note; CODE_AUDIT tracks that under later dependency governance rather than P0.0.
- P0 was verified locally but the GitHub Actions matrix was not triggered; no push was performed.
