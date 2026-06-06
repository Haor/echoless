# LocalVQE Inference Notes

LocalVQE is not wired into the Echoless realtime Rust path yet. The current
`localvqe` processor remains a pass-through stub. This note records the
integration contract verified from the upstream LocalVQE C API.

## Inference Contract

- Model format: released F32 GGUF files from `LocalAI-io/LocalVQE`.
- Input: `16 kHz`, mono, `float32` or `int16`.
- Far reference: mono only. Stereo render must be downmixed before LocalVQE.
- Streaming hop: `256` samples at 16 kHz, i.e. `16 ms`.
- Analysis window: `512` samples.
- API for realtime integration: `localvqe_process_frame_f32(ctx, mic, ref, hop, out)`.
- API for offline smoke tests: `localvqe_process_f32(ctx, mic, ref, n_samples, out)`.

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

The Rust processor should stay a normal `EchoProcessor` node:

- `io_spec = 16 kHz, near mono, far mono`.
- Chain boundary resamples `48 kHz` mic/reference to `16 kHz`.
- If Echoless runs `reference_channels = "stereo"`, LocalVQE still receives a
  mono downmix because upstream LocalVQE has no stereo far-reference API.
- Use `localvqe_process_frame_f32` with a 256-sample stateful buffering layer.
- Do not call LocalVQE from the CPAL callback; keep it in the processing thread.

Recommended first runtime chain:

```toml
reference_channels = "mono"

[[chain]]
kind = "sonora_aec3"
ns = true

[[chain]]
kind = "localvqe"
model = "models/localvqe-v1.2-1.3M-f32.gguf"
```

Use v1.2 first for Windows listening tests because it is the small/fast model.
Use v1.3 after the FFI path is stable.
