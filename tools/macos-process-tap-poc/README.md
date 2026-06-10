# macOS Process Tap PoC / helper

This is a focused probe and development helper for Echoless macOS
`reference="system"`.

The Rust realtime pipeline can spawn this binary in `--stream-stdout` mode and
consume raw Float32 PCM as its far-end reference.

## Build

```bash
./tools/macos-process-tap-poc/build.sh
```

The build script embeds `NSAudioCaptureUsageDescription` into the command-line
binary and ad-hoc signs it. Apple requires the usage string for system audio
capture permission.

## Run

```bash
./tools/macos-process-tap-poc/.build/echoless-process-tap-poc --seconds 10 --out /tmp/process_tap_ref.wav
```

Play system audio while it runs. On first use, macOS should request System Audio
Recording permission for this binary or its parent host.

Expected signs of success:

- stderr shows callbacks and increasing frame counts;
- `peak` and `rms` rise above zero while system audio is playing;
- the output WAV plays the system audio continuously.

If it records silence:

- grant System Audio Recording permission in macOS System Settings;
- quit and rerun the binary;
- confirm system audio is actually playing through the selected output device.

## Realtime stream mode

```bash
./tools/macos-process-tap-poc/.build/echoless-process-tap-poc --stream-stdout --mono
./tools/macos-process-tap-poc/.build/echoless-process-tap-poc --stream-stdout
./tools/macos-process-tap-poc/.build/echoless-process-tap-poc --stream-stdout --exclude-pid 12345
```

`--stream-stdout` writes raw little-endian Float32 PCM to stdout and human logs
to stderr. Mono mode emits one channel; default mode emits interleaved stereo.

The Rust CLI discovers the helper in this order:

1. `ECHOLESS_PROCESS_TAP_HELPER`;
2. a helper binary next to the `echoless` executable;
3. this dev build path under `tools/macos-process-tap-poc/.build/`.

## Scope

The helper records a global stereo Process Tap by default. `--mono` records a
mono global tap. `--exclude-pid` converts the given PID to a Core Audio process
object and excludes it from the tap. The Rust realtime pipeline passes its own
PID so Echoless's processed output is not fed back into the far-end reference.
If Core Audio cannot translate the PID, the helper logs a warning and continues.
