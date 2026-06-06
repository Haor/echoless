# echoless — 跨平台实时 reference-based AEC 工具

面向 **Windows 10/11 + macOS 14.4+** 的本地自用 reference-based AEC 工具。
目标场景是外放音箱 + USB 麦克风做 Discord / VRChat 语音连麦时,用系统播放声音
作为 far-end reference,消除麦克风里的扬声器回声。

当前状态:

- 真实 WebRTC AEC3 路径:vendored `sonora` fork + `sonora_aec3` 处理器。
- 实时 MVP:`echoless run --config configs/example.toml` 走 `cpal` + ringbuf。
- far reference 可用 `reference_channels = "mono" | "stereo"` 切换;默认 mono,stereo 用于外放 L/R 对比试听。
- 离线评测:`echoless offline` 仍可用。
- LocalVQE 已通过动态 C ABI 接入 `localvqe` 处理器;CI 会构建上游 shared library、跑 regression,再跑 Echoless FFI smoke。
- 原生平台 HAL、原生虚拟麦驱动仍是后续阶段;MVP 输出建议接 VB-Cable / BlackHole。

## crate 结构

| crate | 职责 | 状态 |
|---|---|---|
| `echoless-hal` | 平台无关 trait(`AudioSource`/`AudioSink`/`MonotonicClock`)+ 类型 + 文件/null 后端 | ✅ |
| `echoless-hal-win` | Windows HAL(WASAPI/WaveRT/QPC) | stub,实时 MVP 暂走 cpal |
| `echoless-hal-mac` | macOS HAL(CoreAudio/Process Tap/AudioServerPlugin/mach) | stub,实时 MVP 暂走 cpal |
| `echoless-processors` | `EchoProcessor` trait + `ProcessorChain` + `sonora_aec3` / `localvqe` 节点 | ✅ AEC3 可用;LocalVQE 可加载 DLL/dylib + GGUF 推理 |
| `echoless-core` | 管线编排 + `PipelineConfig` + `ControlApi` + `run_offline` | ✅ 离线可用;实时 cpal 路径在 CLI |
| `echoless-cli` | CLI 前端:`processors` / `devices` / `offline` / `run` | ✅ |

依赖单向:`echoless-cli/daemon → 平台HAL → echoless-hal`;`echoless-cli → echoless-core → echoless-processors`。**核心永不依赖平台 crate;前端只经 `ControlApi`。**

## 核心设计:统一可组合处理器

sonora 经典 AEC3 与 LocalVQE 都是平级 `EchoProcessor` 节点,**可单开 / 串联 / 自由组合 / 扩展**:
- 单开经典:`--chain sonora_aec3`
- 单开 LocalVQE:`--chain localvqe`
- 串联:`--chain sonora_aec3,localvqe`
- 加新方案 = 在 `echoless-processors` 写一个 `impl EchoProcessor` + 在 `registry` 登记一行,其余不动。

`ProcessorChain` 自动处理节点间采样率/声道适配与 far ref 分发(每级都拿真实 ref)。
当前边界 SRC 仍是占位线性重采样;LocalVQE 已可真实推理,但最终音质版仍应把边界 SRC 换成有状态实现。

LocalVQE 推理约束见 `docs/localvqe_inference.md`:上游 C API 是 16 kHz mono
mic + mono far reference,streaming hop 为 256 samples/16 ms。
配置参数放在 `[[chain]]` 节点里,例如 `model`、`library`、`threads`、`noise_gate`;
这让后续 Tauri GUI 可以编辑同一份 `PipelineConfig`,而不是依赖 CLI-only flag。

## 构建与试跑

```bash
cd echoless
cargo build --release

# 列出处理器种类
cargo run -- processors

# 列出音频设备
cargo run -- devices

# 实时运行
cargo run --release -- run --config configs/example.toml

# 离线跑链
cargo run -p echoless-cli --bin echoless -- offline \
    --mic takes/doubletalk_01.mic.wav \
    --reference takes/doubletalk_01.ref.wav \
    --out out.wav \
    --chain "sonora_aec3,localvqe"

# 或用配置文件
cargo run -p echoless-cli --bin echoless -- offline --mic m.wav --reference r.wav --out o.wav --config configs/example.toml
```

## GitHub Actions 构建

推送到 `main` 后,`.github/workflows/build.yml` 会在 GitHub-hosted Windows/macOS runner 上:

1. 安装 Rust stable 与 clippy。
2. 运行 `cargo test --workspace --locked`。
3. 运行 `cargo clippy --workspace --all-targets --locked -- -D warnings`。
4. 临时 clone LocalVQE,构建 C API shared library,下载官方 GGUF 跑 regression。
5. 用上一步的 shared library + GGUF 跑 Echoless `localvqe_ffi_smoke`。
6. 生成 release artifact:`echoless-windows-*` / `echoless-macos-*`,并打包 LocalVQE runtime 与小模型。

## 下一步

1. 用 Windows 外放 + USB mic + VB-Cable 做实机反馈,调 `tail_ms` / `ns_level`。
2. 增加 `eval` 子命令,用 output/input energy ratio 做离线效果量化。
3. `echoless-processors/chain.rs` 占位线性 SRC 换成 rubato 有状态 SRC。
4. 把实时 runtime 从 CLI 层进一步抽成 GUI/daemon 可复用控制面。
5. 原生 WASAPI/CoreAudio/虚拟麦驱动阶段再替换 cpal MVP。
