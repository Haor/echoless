# aec3-common-audio

[![crate][crate-image]][crate-link]
[![docs][docs-image]][docs-link]
![BSD-3-Clause licensed][license-image]
![Rust Version][rustc-image]

Audio DSP primitives for the [AEC3] audio processing library.

Includes sinc resampler, push resampler, channel buffer, biquad filter,
and audio format conversion utilities. These building blocks are shared
across the echo canceller, noise suppressor, and gain controller.

Part of the [AEC3] audio processing library.

## License

BSD-3-Clause. See [LICENSE] for details.

[//]: # (badges)

[crate-image]: https://img.shields.io/crates/v/aec3-common-audio.svg
[crate-link]: https://crates.io/crates/aec3-common-audio
[docs-image]: https://docs.rs/aec3-common-audio/badge.svg
[docs-link]: https://docs.rs/aec3-common-audio/
[license-image]: https://img.shields.io/badge/license-BSD--3--Clause-blue.svg
[rustc-image]: https://img.shields.io/badge/rustc-1.91+-blue.svg

[//]: # (general links)

[AEC3]: https://github.com/dignifiedquire/aec3-apm#readme
[LICENSE]: https://github.com/dignifiedquire/aec3-apm/blob/main/LICENSE
