# Changelog

All notable changes to Echoless are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org).

## [1.1.0] — 2026-07-08

A stability and polish release on top of 1.0.0: adaptive handling of audio
clock drift to kill periodic dropouts, a reliable delay probe, and a wide
UI cleanup.

### Added
- **Adaptive clock-drift rate matching** (`output_rate_match`, on by default).
  The capture, output, and loopback clocks are independent and drift — most
  noticeably with virtual audio devices — which used to slowly starve or
  overflow the ring buffers and surface as periodic clicks/dropouts. The output
  and reference paths now resample to hold the buffers at a steady level instead
  of dropping or zero-filling frames. A half-frame deadband keeps well-behaved
  devices bit-exact, and the resampling ratio is trimmed at most ±3%. See
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
- **Output clock-skew detection** in diagnostics, so a drifting device is
  reported directly instead of showing up as unexplained WAV artifacts.
- **Live download progress.** LocalVQE model downloads show a percentage in the
  model box (was a static "···"), and the RTX/NVAFX runtime download shows a
  percentage during setup.

### Changed
- Platform-aware UI polish across the Engine, Diagnostics, and RTX pages.
- Advanced help texts rewritten as plain definitions — each option now states
  what it is, without jargon or tuning chatter.
- **Hardened LocalVQE model downloads:** forced HTTP/1.1 (dodges the
  Hugging Face CDN's occasional HTTP/2 stream cancels), more retries with
  resume. The models folder's `README.txt` now lists every supported filename
  with its pinned SHA-256.

### Fixed
- **Delay probe** no longer hangs or fails. Fixed a dev-only freeze where the
  probe sat at PROBING with no progress and never completed, the `run` helper
  exiting too early or not at all, and made the probe fill the correct delay
  parameter per platform (initial delay on Windows, near delay on macOS).
- Engine page no longer flashes in the NVAFX system-info panel — host info is
  cached and shown immediately on entry.
- The RUN PROBE help tooltip now opens upward so it no longer covers the
  progress dots and result beneath it.
- Fixed a black-screen / noise-floor issue on launch, and disabled the in-app
  right-click browser menu.
- macOS "Open Settings" for system-audio recording is no longer blocked by the
  deep-link allowlist.
- LocalVQE GET badge now matches the color of the OK / checkmark states.
- A LocalVQE download-failure message no longer overflows the Engine card — it
  renders small and clamps to three lines, with the full text on hover.
- Starting a second LocalVQE download while one is in flight no longer corrupts
  the first. Each model is now tracked and disabled independently while it
  downloads, and the backend rejects a duplicate concurrent download of the same
  file instead of letting the two clobber a shared partial file (which produced
  "size mismatch" errors until the page was reopened).
- The bundled `echoless` CLI now reports version 1.1.0 (the Rust workspace was
  left at 1.0.0 while the app shipped as 1.1.0).
- NVAFX runtime download is more robust: a longer timeout for the ~1 GB fetch,
  a byte-count readout when the server doesn't report a total size (so progress
  isn't blank), and stderr context included on timeout.

## [1.0.0] — 2026-07-06

Initial public release: real-time acoustic echo cancellation for open-speaker
setups, with three interchangeable engines (AEC3, LocalVQE, NVAFX/RTX),
per-OS system-audio reference capture (WASAPI loopback / Core Audio Process Tap
/ PipeWire monitor), a virtual-microphone output path, the delay probe, output
volume control, diagnostics recording, and a standalone `echoless` CLI. Windows
and macOS supported; Linux experimental.
