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

### Changed
- Platform-aware UI polish across the Engine, Diagnostics, and RTX pages.
- Advanced help texts rewritten as plain definitions — each option now states
  what it is, without jargon or tuning chatter.

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

## [1.0.0] — 2026-07-06

Initial public release: real-time acoustic echo cancellation for open-speaker
setups, with three interchangeable engines (AEC3, LocalVQE, NVAFX/RTX),
per-OS system-audio reference capture (WASAPI loopback / Core Audio Process Tap
/ PipeWire monitor), a virtual-microphone output path, the delay probe, output
volume control, diagnostics recording, and a standalone `echoless` CLI. Windows
and macOS supported; Linux experimental.
