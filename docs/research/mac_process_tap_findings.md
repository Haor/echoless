# macOS Process Tap findings

Updated: 2026-06-08

## Status

Core Audio Process Tap is viable as Echoless's macOS `reference="system"` backend.
The first realtime integration is now implemented through a Swift helper that
streams raw Float32 PCM to the Rust pipeline.

Verified locally on:

- macOS 26.5.1
- Apple Swift 6.3.2
- SDK: `/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk`

PoC/helper path:

- `tools/macos-process-tap-poc/`

## What Was Verified

The PoC follows Apple's documented tap path:

1. create `CATapDescription`;
2. call `AudioHardwareCreateProcessTap`;
3. create a private aggregate device with `kAudioAggregateDeviceTapListKey`;
4. read the aggregate device with `AudioDeviceCreateIOProcIDWithBlock`;
5. write captured Float32 PCM to WAV.

The PoC embeds `NSAudioCaptureUsageDescription` into the command-line binary and
ad-hoc signs it.

Successful run with system audio playback:

```bash
./tools/macos-process-tap-poc/.build/echoless-process-tap-poc \
  --seconds 4 \
  --out /private/tmp/echoless-process-tap-ref-with-afplay.wav &
rec_pid=$!
sleep 0.7
afplay /System/Library/Sounds/Glass.aiff
wait $rec_pid
```

Observed result:

```text
started Process Tap: 48000 Hz, 2 ch, 32-bit float, interleaved
frames=196608 callbacks=384 peak=0.19832 rms=0.01682
wrote /private/tmp/echoless-process-tap-ref-with-afplay.wav
```

That proves the tap can capture real system-output audio, not just silent
callbacks.

Realtime integration smoke test:

```bash
./target/debug/echoless run \
  --mic coreaudio:BuiltInMicrophoneDevice \
  --reference system \
  --output coreaudio:BlackHole2ch_UID \
  --processor sonora_aec3 \
  --reference-channels stereo \
  --status-json \
  --stats-interval-ms 1000
```

Observed `started` event:

```json
{"type":"started","reference_source":"macos_process_tap","sample_rate":48000,"reference_channels":"stereo"}
```

Observed status after startup:

```text
ref_dbfs=-36.09, ref_underruns=0, ref_input_drops=0, runtime_errors=0
```

`devices --json` on macOS now exposes `System Audio (Process Tap)` as the
system reference and no longer lists ordinary output devices as reference
sources. Input devices, including BlackHole, remain available as explicit
fallback references.

## Important Failure Mode

Running the same binary inside the Codex command sandbox failed:

```text
AudioHardwareCreateProcessTap returned unknown object failed: -3
```

Running it outside the sandbox succeeded. Product implication:

- permission and CoreAudio ownership must belong to the packaged app or a fixed
  helper binary;
- development runs from Cursor/Codex/Terminal can attribute system audio
  permission to the wrong host;
- the frontend should not infer product permission behavior from dev-host
  permission prompts.

## API Notes

For the aggregate device tap list, using `CATapDescription.uuid.uuidString` as
`kAudioSubTapUIDKey` worked. Reading `kAudioTapPropertyUID` before aggregate
creation failed in the sandboxed run and is not needed for this PoC.

Default PoC mode is global stereo tap:

```swift
CATapDescription(stereoGlobalTapButExcludeProcesses: excludedProcessIDs)
```

The realtime integration passes `--exclude-pid <echoless pid>` to the helper.
The helper translates that PID through
`kAudioHardwarePropertyTranslatePIDToProcessObject` and excludes the resulting
Core Audio process object. This keeps Echoless's processed virtual-mic output
out of the far-end reference. If the PID cannot be translated, the helper logs
a warning and continues with the remaining exclusions.

## Product Integration Plan

Backend:

- macOS 14.2+ / preferably 14.4+: route `reference="system"` to Process Tap,
  not CPAL output-device loopback. **Implemented for 48 kHz through Swift
  helper stdout streaming.**
- Keep BlackHole/VB-CABLE MAC as fallback for older macOS or denied system
  audio permission.
- Add a macOS HAL source that emits timestamped Float32 frames from Process Tap.
  First implementation emits raw Float32 PCM through helper stdout; timestamp
  plumbing is still future work.
- Reuse the existing `ReferenceChannels` setting:
  - `mono` uses mono global tap or downmix;
  - `stereo` uses stereo global tap.
- Stop exposing ordinary physical output devices as recommended macOS reference
  sources unless a backend can actually read them. **Implemented for macOS
  `devices --json`.**

Frontend/doctor contract:

- `reference_sources.system` should become available on supported macOS when
  the Process Tap backend is present.
- Add `system_audio_capture` status:
  `supported | permission_needed | denied | unavailable | unknown`.
- A request/check command can start and stop a tiny tap to trigger permission;
  there does not appear to be a stable public TCC permission-query API for this.

## References

- Apple: Capturing system audio with Core Audio taps:
  https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps
- Apple: `AudioHardwareCreateProcessTap`:
  https://developer.apple.com/documentation/coreaudio/audiohardwarecreateprocesstap%28_%3A_%3A%29
- Apple: `NSAudioCaptureUsageDescription`:
  https://developer.apple.com/documentation/bundleresources/information-property-list/nsaudiocaptureusagedescription
