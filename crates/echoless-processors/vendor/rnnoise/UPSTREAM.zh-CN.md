# RNNoise 上游基线

本目录内的源码来自 Xiph 官方 RNNoise 仓库，固定在提交：

```text
70f1d256acd4b34a572f999a05c87bf00b67730d
```

该快照晚于 `v0.2`，包含 32 频带网络、`bb18d2f` 的瞬态噪声增益衰减修复，以及后续官方模型更新。Echoless 只内置运行时所需的最小源码集合，不包含训练工具和示例程序。

模型来自该提交 `model_version` 指向的官方 Xiph 模型包：

```text
model version: 0a8755f8e2d834eff6a54714ecc7d75f9932e845df35f8b59bc52a7cfe6e8b37
weights_blob.bin sha256: 1b99898350e75656c77d068162fea402afe51eff15dc751989b1e9f53b98bf91
```

`weights_blob.bin` 由官方 `src/write_weights.c` 使用该模型包的 `src/rnnoise_data.c` 生成。源码和模型在构建时编入静态产物，运行时不下载模型，也不依赖外部 RNNoise 动态库。

上游地址：<https://github.com/xiph/rnnoise>

许可见同目录 `COPYING`（BSD-3-Clause）。
