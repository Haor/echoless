# aec3 — Echoless 内化的 AEC3 引擎

WebRTC AEC3(声学回声消除)的纯 Rust 移植,源自
[dignifiedquire/aec3-apm](https://github.com/dignifiedquire/aec3-apm)
@ `aacadf0` 的 fork,现由 Echoless **完全内化维护**(不再跟随上游)。
BSD-3-Clause,见 `LICENSE`。

## 结构

独立 cargo workspace(edition 2024),经 path 依赖被主 workspace 的
`echoless-processors` 引用(feature `aec3`):

| crate | 内容 |
|---|---|
| `aec3-apm` | Audio Processing Module 顶层(echo cancel + NS + AGC2 管线) |
| `aec3-core` | AEC3 本体(delay estimator / erle / suppressor …) |
| `aec3-ns` / `aec3-agc2` | 噪声抑制 / AGC2 |
| `aec3-common-audio` / `aec3-fft` / `aec3-simd` | 公共音频原语 / FFT / SIMD |

## 相对上游的改动

- **延迟惯性魔改(P4)**:underrun 不扣 delay、不软重置;`estimate_delay`
  加 render 静音门(gate 期不增 consistent counter)。各处以 `Echoless:`
  注释标记;负方向搜索经上层 `near_delay` 偏置实现(本 workspace 零改动)。
- **裁剪**:上游的 cpp 参考实现、fuzz、ffi/sys 绑定、bench、examples、
  CI/发布配置、10M 测试音频已全部移除;in-src 单元测试与 proptest 保留。

## 测试

```bash
cd aec3 && cargo test
```

设计/结构分析见 `research/aec3_internal_map.md`(AEC/ 工作区),
集成结论见 `docs/architecture/AEC3_INTERNALIZATION_PLAN.md`。
