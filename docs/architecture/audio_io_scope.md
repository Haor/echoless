# Audio I/O Scope

本文固定 Echoless 当前音频 I/O 边界,避免把通用 trait 误读成原生平台 I/O 或自研虚拟麦路线。

## 当前产品决策

- 原生虚拟麦克风驱动不做。Windows/macOS 输出长期依赖外部虚拟音频设备,例如 VB-Cable、BlackHole、Virtual Desktop Mic 或用户指定的等价设备。
- 当前实时路径使用 `cpal` 枚举、采集与输出设备。没有证据表明 `cpal` 是音质、断音或延迟问题的瓶颈。
- GUI 不依赖原生平台 I/O。前端继续通过 CLI sidecar 调用 `devices`、`config validate`、`run --status-json` 和 diagnostics。
- 平台专用 I/O stub crate 已删除。当前源码只保留 `echoless-audio-io` 这个平台无关音频 I/O 抽象 crate。

## `echoless-audio-io` 是什么

`echoless-audio-io` 提供 pull 式音频 I/O trait 和基础类型:

- `AudioSource`: 麦克风输入和 far-end reference 输入。
- `AudioSink`: 处理后音频输出到用户选择的外部虚拟设备或试听输出。
- `MonotonicClock`: 单调时钟接口。
- `WavFileSource` / `WavFileSink`: 离线评测与测试。
- `NullSource` / `NullSink`: fallback 与测试辅助。

它不是当前 runtime 的平台后端,也不包含 WASAPI/CoreAudio 实现。当前实时音频路径在 `echoless-cli/src/realtime.rs`:

```text
cpal mic stream + cpal reference stream + cpal output stream
        -> ringbuf
        -> ProcessorChain
        -> selected output device
```

## 当前不做的内容

- Windows WaveRT/SysVAD/simpleaudiosample 自研虚拟麦驱动。
- macOS AudioServerPlugin 自研虚拟麦。
- 自动静默安装第三方虚拟设备。
- WASAPI/CoreAudio 原生平台 I/O 重写。
- 为 GUI 单独暴露 native driver 或原生平台 I/O 控制面。

## 重新评估条件

默认保持 `cpal` 实时路径。只有出现可测量、可复现的 I/O 瓶颈时才重新评估平台原生 I/O,触发条件应来自 diagnostics 或人工测试:

- Windows loopback/reference 在目标设备上无法可靠采集。
- macOS system audio 需要 Process Tap 才能获得可接受的 reference,而 BlackHole/外部路由不可接受。
- 输入/参考 timestamp 不足以稳定估算回声延迟,导致 AEC3 双讲或音量稳定性明显变差。
- device change、sleep/wake、默认设备切换恢复不稳定。
- 端到端延迟/抖动主要来自 I/O 层,且无法通过 `frame_ms`、queue、buffer 参数改善。
- 需要更完整的 WASAPI/CoreAudio 错误码、stream format、buffer size 诊断。

如果未来证据充分,只做窄范围平台 I/O 修补,优先从被证明有问题的 reference capture、timestamp 或 device recovery 开始;仍不实现自研虚拟麦驱动。
