# aec3-agc2

[![crate][crate-image]][crate-link]
[![docs][docs-image]][docs-link]
![BSD-3-Clause licensed][license-image]
![Rust Version][rustc-image]

Pure Rust implementation of [Automatic Gain Control 2 (AGC2)][AGC2] from WebRTC.

RNN-based voice activity detection (VAD) drives an adaptive digital gain
controller with limiter and clipping predictor. Includes fixed digital gain,
speech level estimation, and input volume control.

Part of the [AEC3] audio processing library.

## License

BSD-3-Clause. See [LICENSE] for details.

[//]: # (badges)

[crate-image]: https://img.shields.io/crates/v/aec3-agc2.svg
[crate-link]: https://crates.io/crates/aec3-agc2
[docs-image]: https://docs.rs/aec3-agc2/badge.svg
[docs-link]: https://docs.rs/aec3-agc2/
[license-image]: https://img.shields.io/badge/license-BSD--3--Clause-blue.svg
[rustc-image]: https://img.shields.io/badge/rustc-1.91+-blue.svg

[//]: # (general links)

[AGC2]: https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/agc2/
[AEC3]: https://github.com/dignifiedquire/aec3-apm#readme
[LICENSE]: https://github.com/dignifiedquire/aec3-apm/blob/main/LICENSE
