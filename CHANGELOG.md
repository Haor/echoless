# Changelog

All notable changes to Echoless are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org).

## [Unreleased]

### Added
- Windows **Auto Start** option: launch hidden after sign-in and start the saved
  Echoless audio pipeline automatically. Startup failures reveal the main window
  instead of remaining silent in the tray.

## [1.1.0] — 2026-07-11

A stability and polish release on top of 1.0.0: adaptive handling of audio
clock drift to kill periodic dropouts, a reliable delay probe, a real fix for
the random black-screen crash, persistent crash-forensics logging, and a wide
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
- **Clock skew in the Health panel.** The redundant "stale drops" counter
  (always the sum of mic stale + ref stale) gave its slot to a live clock-skew
  readout that turns red with the backend warning; every Health counter now
  has a hover tooltip explaining what it measures and when to worry.
- **Persistent crash-forensics log** at `<data dir>/logs/echoless-<stamp>.log`
  (next to the diagnostics folder). Captures app start, engine start/exit with
  crash attribution, full CLI stderr, and frontend errors with component
  stacks — a bug report can now just attach the file. One file per launch,
  pruned on startup (7 days / 20 files kept), 8 MiB per-file cap.
- **Live download progress.** LocalVQE model downloads show a percentage in the
  model box (was a static "···"), and the RTX/NVAFX runtime download shows a
  percentage during setup.

### Changed
- Platform-aware UI polish across the Engine, Diagnostics, and RTX pages.
- Advanced help texts rewritten as plain definitions — each option now states
  what it is, without jargon or tuning chatter.
- Pull requests to `dev` and `main` now run the existing release quality gates,
  and the Windows and macOS package jobs explicitly run the desktop backend
  tests before building installers.
- **Hardened LocalVQE model downloads:** forced HTTP/1.1 (dodges the
  Hugging Face CDN's occasional HTTP/2 stream cancels), more retries with
  resume. The models folder's `README.txt` now lists every supported filename
  with its pinned SHA-256.
- In-app tooltips unified on the themed popup (now portal-based so scrolling
  or clipping containers can't cut it off); native tooltips are kept only
  where long filesystem paths need unconstrained width.

### Fixed
- **The random black screen.** Since 1.0.0 the app could go fully black and
  unresponsive after running for a while — most often on machines whose audio
  clock drift hovered around the warning threshold. Root cause: backend
  `clock_skew_warning/resolved` events share the status event channel, the
  frontend dispatcher treated every unrecognized event as a status frame, and
  the resulting undefined telemetry values crashed a render
  (`undefined.toFixed`) with no error boundary to contain it — React unmounted
  the whole UI. Fixed in depth: the dispatcher now whitelists status frames,
  telemetry values are null-coalesced, number formatting guards against
  non-finite values, and new React error boundaries (app-wide plus around the
  telemetry panel) turn any future render crash into a contained fallback with
  a Retry button instead of a black window.
- **Clock-skew warnings no longer flap on scheduler hiccups.** A window switch
  or scheduling stall could spike one 5-second measurement window to 8%+ and
  bounce warning/resolved back and forth. The detector now smooths readouts
  with an EMA and only enters the warning state after two consecutive
  over-threshold windows — isolated spikes never alert, while real sustained
  mismatch still alerts within ~10 s.
- **Delay probe** no longer hangs or fails. Fixed a dev-only freeze where the
  probe sat at PROBING with no progress and never completed, the `run` helper
  exiting too early or not at all, and made the probe fill the correct delay
  parameter per platform (initial delay on Windows, near delay on macOS).
- Engine page no longer flashes in the NVAFX system-info panel — host info is
  cached and shown immediately on entry.
- The RUN PROBE help tooltip now opens upward so it no longer covers the
  progress dots and result beneath it.
- Fixed the frozen noise-floor grain on Windows, made the WebGL grain survive
  GPU context loss on both platforms, and disabled the in-app right-click
  browser menu.
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
  isn't blank), stderr context included on timeout, and no separate metadata
  request that can hold up an otherwise successful download.
- RTX setup keeps reporting live download progress without making a second
  metadata request. Default release assets use their pinned sizes for a
  percentage; custom tags continue to show the growing downloaded byte count.
- The Advanced page no longer shifts vertically when switching language. The
  section headers and page kicker had font-derived line heights, so CJK titles
  rendered a few pixels taller than Latin ones and nudged every row below them;
  the line boxes are now locked to a script-independent height.
- Selecting LocalVQE before a model is chosen, or NVAFX before its runtime is
  installed, no longer changes the active engine or tries to start an incomplete
  configuration. A running engine stops cleanly before the setup flow opens,
  and choosing the already active LocalVQE model is a no-op.
- After a successful NVAFX download-and-install, the ~1 GB download cache under
  the system temp directory is cleared automatically once the runtime and model
  are extracted and the doctor check passes. The cache is kept on failure so a
  retry doesn't re-download.
- macOS system-audio capture now has a strict readiness handshake. The engine
  cannot report a healthy run until Process Tap produces a valid stream header,
  the header wait has a 25-second deadline, and an unexpected helper exit stops
  the run instead of silently processing a zero reference.
- Restarting the engine can no longer let a stale sidecar overwrite the active
  run, tray state, or frontend status.
- Engine stop/restart and live controls stay responsive when sidecar status or
  control pipes stall; shutdown now has bounded fallbacks instead of freezing
  the desktop UI.
- Re-selecting the active engine no longer reloads it. When NVAFX is selected,
  its fixed `48 kHz / 10 ms / mono` pipeline is applied atomically and the three
  incompatible Advanced choices are locked.
- A rejected font-readiness promise no longer leaves the boot screen invisible,
  and delayed desktop event subscriptions are fully removed after a page remount.
- Launching Echoless again focuses the existing desktop window instead of
  starting a second audio pipeline against the same devices and data directory.
- LocalVQE native processing failures now clear stale queued audio, preserve
  failure telemetry, and pass through the current microphone frame while the
  backend recovers instead of accumulating or replaying old samples.
- LocalVQE runtime tuning is now transactional: a native noise-gate setter error
  leaves the last accepted gate and threshold in effect.
- Stereo reference audio stays frame-aligned when its ring buffer is full or
  stale data is trimmed, preventing left/right channel desynchronization under
  pressure.
- Clock-skew diagnostics now detect both faster and slower output/reference
  clocks and normalize loss counters to audio frames, including stereo paths.
- Device selectors containing quotes, backslashes, or control characters now
  round-trip through generated TOML without being changed or rejected.
- CoreAudio device-change callbacks remain valid when listener removal fails,
  avoiding a rare hot-plug crash on macOS.
- Timed-out Windows commands now terminate their complete child-process tree,
  so a descendant cannot keep the desktop backend waiting past its timeout.
- Simultaneous app launches now always reserve distinct crash-forensics logs
  with independent size accounting.
- NVAFX runtime installation and diagnostics recording now use their managed
  Echoless data directories exclusively. Removed path overrides can no longer
  point installation, recording, probe cleanup, or session deletion at an
  arbitrary directory; delay-probe cleanup is limited to the exact session it
  created.
- Fixed diagnostics recording so its enabled state is independent of the
  optional time cap. CLI and TOML users can again record until stopped while
  the output directory remains managed by Echoless.
- Undersized AEC3 filter-tail settings now fail validation instead of reaching
  an internal assertion, and all physical capture paths replace non-finite
  device samples before they can poison a stateful processor.
- Any CPAL stream failure now ends the current run and returns the GUI to OFF
  with the failure reason instead of leaving a nonfunctional pipeline displayed
  as active. RTX GPU discovery also has a bounded deadline, so a stuck
  `nvidia-smi` cannot freeze setup indefinitely.
- Cross-rate audio remains continuous at 44.1↔48 kHz: output rate matching now
  combines its nominal conversion ratio with device-clock drift correction,
  while input conversion preserves phase and buffers across variable callback
  sizes instead of rebuilding its FFT layout and inserting silence.
- Large-ratio output conversion now consumes every skipped source sample. A
  48→16 kHz endpoint previously advanced the interpolation phase by three while
  removing only two samples, producing the wrong speed/pitch and steadily
  filling the output ring.
- Entering an unavailable engine's setup flow now invalidates queued and
  in-flight restart work. A device refresh can no longer restart the old
  sidecar after the user has already moved into LocalVQE/NVAFX setup.
- Delay probing now waits for both the audio pipeline and diagnostics writer to
  become ready before its stabilization delay and beep train begin. Successful
  measurements are still returned when best-effort session cleanup is blocked
  by an external file lock.

### Security
- Audio-device names are rendered strictly as text during the scramble
  animation and never enter the HTML parser.
- External HTTPS links are checked using canonical URL components; credentials
  and non-default ports are rejected before the hostname allowlist is applied.

## [1.0.0] — 2026-07-06

Initial public release: real-time acoustic echo cancellation for open-speaker
setups, with three interchangeable engines (AEC3, LocalVQE, NVAFX/RTX),
per-OS system-audio reference capture (WASAPI loopback / Core Audio Process Tap
/ PipeWire monitor), a virtual-microphone output path, the delay probe, output
volume control, diagnostics recording, and a standalone `echoless` CLI. Windows
and macOS supported; Linux experimental.
