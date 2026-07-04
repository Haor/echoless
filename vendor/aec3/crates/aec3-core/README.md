# aec3-core

[![crate][crate-image]][crate-link]
[![docs][docs-image]][docs-link]
![BSD-3-Clause licensed][license-image]
![Rust Version][rustc-image]

Pure Rust implementation of [Echo Canceller 3 (AEC3)][AEC3] from WebRTC.

Adaptive filter-based acoustic echo canceller with automatic delay estimation,
render signal analysis, and echo path change detection. Operates in the
frequency domain using partitioned block processing.

Part of the [AEC3] audio processing library.

## License

BSD-3-Clause. See [LICENSE] for details.

[//]: # (badges)

[crate-image]: https://img.shields.io/crates/v/aec3-core.svg
[crate-link]: https://crates.io/crates/aec3-core
[docs-image]: https://docs.rs/aec3-core/badge.svg
[docs-link]: https://docs.rs/aec3-core/
[license-image]: https://img.shields.io/badge/license-BSD--3--Clause-blue.svg
[rustc-image]: https://img.shields.io/badge/rustc-1.91+-blue.svg

[//]: # (general links)

[AEC3]: https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/
[AEC3]: https://github.com/dignifiedquire/aec3-apm#readme
[LICENSE]: https://github.com/dignifiedquire/aec3-apm/blob/main/LICENSE
