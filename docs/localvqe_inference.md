# LocalVQE Inference Notes

LocalVQE is wired into Echoless through the upstream dynamic C ABI. The
`localvqe` processor loads a LocalVQE shared library plus a GGUF model at
configuration time, then runs the streaming frame API from the processing
thread. This note records the runtime contract and the remaining test limits.

## Inference Contract

- Model format: released F32 GGUF files from `LocalAI-io/LocalVQE`.
- Input: `16 kHz`, mono, `float32` or `int16`.
- Far reference: mono only. Stereo render must be downmixed before LocalVQE.
- Streaming hop: `256` samples at 16 kHz, i.e. `16 ms`.
- Analysis window: `512` samples.
- API for realtime integration: `localvqe_process_frame_f32(ctx, mic, ref, hop, out)`.
- API for offline smoke tests: `localvqe_process_f32(ctx, mic, ref, n_samples, out)`.
- Echoless can keep `frame_ms = 10`; the processor buffers 16 kHz blocks until
  a full 256-sample LocalVQE hop is available, and emits buffered output with a
  short startup latency.

## Platform Build

The lowest-risk first test is to build LocalVQE as its own C API library:

```bash
git clone --recursive https://github.com/localai-org/LocalVQE.git
cmake -S LocalVQE/ggml -B localvqe-shared-build -DCMAKE_BUILD_TYPE=Release -DLOCALVQE_BUILD_SHARED=ON
cmake --build localvqe-shared-build --config Release --target localvqe_shared

cmake -S LocalVQE/ggml -B localvqe-regression-build -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_FLAGS="-DLOCALVQE_BUILD"
cmake --build localvqe-regression-build --config Release --target test_regression regression-assets
ctest --test-dir localvqe-regression-build -C Release -R "^regression_" --output-on-failure
```

On Windows this produces `localvqe.dll`; on macOS this produces `liblocalvqe.dylib`.
Keep the GGML backend libraries next to the binary/library when packaging or
manual testing.

The Windows upstream regression target may need a temporary compatibility shim
for `S_ISREG` in `ggml/tests/test_helpers.h`; the Echoless GitHub workflow
applies this only to its throwaway LocalVQE clone.

## Echoless Integration Shape

The Rust processor stays a normal `EchoProcessor` node:

- `io_spec = 16 kHz, near mono, far mono`.
- Chain boundary resamples `48 kHz` mic/reference to `16 kHz`.
- If Echoless runs `reference_channels = "stereo"`, LocalVQE still receives a
  mono downmix because upstream LocalVQE has no stereo far-reference API.
- Use `localvqe_process_frame_f32` with a 256-sample stateful buffering layer.
- Do not call LocalVQE from the CPAL callback; keep it in the processing thread.
- Configure with `model`, optional `library`, `backend`, `device`, `threads`,
  `noise_gate`, and `noise_gate_threshold_dbfs` inside the `[[chain]]` node.
- If `library` is omitted, Echoless tries the current executable directory,
  `./localvqe/`, the current working directory, and `ECHOLESS_LOCALVQE_LIBRARY`.

Recommended first runtime chain:

```toml
reference_channels = "mono"

[[chain]]
kind = "sonora_aec3"
ns = true

[[chain]]
kind = "localvqe"
model = "models/localvqe-v1.2-1.3M-f32.gguf"
library = "localvqe.dll" # macOS: "liblocalvqe.dylib"
threads = 2
noise_gate = true
noise_gate_threshold_dbfs = -45.0
```

Use v1.2 first for Windows listening tests because it is the small/fast model.
Use v1.3 after the FFI path is stable.

## Current Limits

- The LocalVQE processor is real, but the boundary SRC in `ProcessorChain` is
  still the placeholder per-block linear resampler. This is acceptable for a
  first Windows functionality/listening test, not a final quality claim.
- LocalVQE is mono far-reference only. It can be tested after stereo AEC3, but
  the LocalVQE node itself receives a downmixed reference.
- The GitHub workflow runs an Echoless FFI smoke test with the built shared
  library and model. The Windows handoff still needs real device listening
  evidence for delay, CPU, artifacts, and speech preservation.
