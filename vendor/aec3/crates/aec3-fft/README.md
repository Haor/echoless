# aec3-fft

[![crate][crate-image]][crate-link]
[![docs][docs-image]][docs-link]
![BSD-3-Clause licensed][license-image]
![Rust Version][rustc-image]

Pure Rust FFT implementations for the [AEC3] audio processing library.

Includes Ooura 128-point and general-purpose (fft4g) FFTs, plus a Rust port
of [PFFFT] (Pretty Fast FFT) for composite-size real and complex transforms.
Optimized for the specific sizes used in WebRTC audio processing (128, 256, 512).

Part of the [AEC3] audio processing library.

## License

BSD-3-Clause. See [LICENSE] for details.

[//]: # (badges)

[crate-image]: https://img.shields.io/crates/v/aec3-fft.svg
[crate-link]: https://crates.io/crates/aec3-fft
[docs-image]: https://docs.rs/aec3-fft/badge.svg
[docs-link]: https://docs.rs/aec3-fft/
[license-image]: https://img.shields.io/badge/license-BSD--3--Clause-blue.svg
[rustc-image]: https://img.shields.io/badge/rustc-1.91+-blue.svg

[//]: # (general links)

[PFFFT]: https://bitbucket.org/jpommier/pffft/
[AEC3]: https://github.com/dignifiedquire/aec3-apm#readme
[LICENSE]: https://github.com/dignifiedquire/aec3-apm/blob/main/LICENSE
