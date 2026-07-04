# echoless — 跨平台实时 reference-based AEC 工具

面向 **Windows 10/11 + macOS 14.4+** 的本地自用 reference-based AEC 工具。
目标场景是外放音箱 + USB 麦克风做 Discord 等语音连麦时,用系统播放声音
作为 far-end reference,消除麦克风里的扬声器回声。

当前状态:

- 真实 WebRTC AEC3 路径:vendored `aec3` fork + `aec3` 处理器。
- 实时主路径:`echoless run --config configs/example.toml` 走 `cpal` + ringbuf。
- 设备 I/O 边界已支持固定比率线性重采样:非 48k/16k 原生采样率的 mic/reference/output 可打开后适配到管线采样率。
- far reference 可用 `reference_channels = "mono" | "stereo"` 切换;默认 mono,stereo 用于外放 L/R 对比试听。
- 最终输出电平可用顶层 `output_level = 0..100` 调整:0 静音,50 原声,100 约 3x 增益;曲线为 `gain = (output_level / 50)^log2(3)`,后端在所有处理器之后统一应用并做软限幅保护。
- 离线评测:`echoless offline` 仍可用。
- LocalVQE 已通过动态 C ABI 接入 `localvqe` 处理器;CI 会构建上游 shared library、跑 regression,再跑 Echoless FFI smoke。
- NVIDIA AFX / RTX AEC 已作为 Windows-only 可选 backend 接入:`doctor` / 本地 runtime install / 离线 WAV / 实时 `nvidia_afx_aec`。
- Windows 本机 RTX AEC standalone 已完成 45s diagnostics smoke:RTX 5080 Blackwell runtime 可用,USB mic index `4`,reference `system`,output `3`(CABLE Input),`runtime_errors=0`。
- macOS artifact 可正常构建 AEC3/LocalVQE 路径;RTX AEC 在 macOS 上按设计不可用,GUI/安装器应通过 `echoless nvafx doctor --json` 禁用该 backend。
- 输出依赖外部虚拟音频设备:Windows 推荐 VB-CABLE,macOS 推荐 BlackHole 或 VB-CABLE MAC;也可使用 Virtual Desktop Mic 等用户已有设备。
- GUI/安装器应提供虚拟音频设备安装引导:检测设备是否已安装,未安装时引导用户安装,安装后重新枚举并验证 output/input 端可用。
- 产品默认策略:以 `aec3` 保真人声为主。LocalVQE 与 RTX AEC 是独立可选 backend。

## crate 结构

| crate | 职责 | 状态 |
|---|---|---|
| `echoless-audio-io` | 平台无关音频 I/O trait + 类型 + 文件/null 后端 | ✅ |
| `echoless-processors` | `EchoProcessor` trait + `ProcessorChain` + `aec3` / `localvqe` / `nvidia_afx_aec` 节点 | ✅ AEC3 可用;LocalVQE 可加载 DLL/dylib + GGUF 推理;RTX AEC Windows 可动态加载 AFX runtime |
| `echoless-core` | `PipelineConfig` + 离线编排 + 输出电平/声道策略等共享工具 | ✅ 离线可用;实时 cpal sidecar runtime 在 CLI |
| `echoless-cli` | CLI 前端:`processors` / `devices` / `offline` / `run` | ✅ |

依赖单向:`echoless-cli → echoless-core → echoless-processors`。**核心不依赖平台专用 crate;GUI/安装器只经 CLI sidecar 的 JSON/status/control 合约接入实时能力。**

## 核心设计:统一处理器

aec3 经典 AEC3、LocalVQE、RTX AEC 都是平级 `EchoProcessor` 节点。
当前产品主线是 AEC3 保真优先,LocalVQE 保留为独立可选处理器:
- AEC3:`--chain aec3`
- LocalVQE:`--chain localvqe`
- 加新方案 = 在 `echoless-processors` 写一个 `impl EchoProcessor` + 在 `registry` 登记一行,其余不动。

`ProcessorChain` 自动处理处理器边界的采样率/声道适配与 far ref 分发。
当前边界 SRC 仍是占位线性重采样;LocalVQE 已可真实推理,但最终音质版仍应把边界 SRC 换成有状态实现。
设备 I/O 边界也使用固定比率线性 SRC;这能解锁 24k/44.1k 等真实设备,但还不是 drift 自适应高质量 SRC。

LocalVQE 推理约束见 `docs/localvqe_inference.md`:上游 C API 是 16 kHz mono
mic + mono far reference,streaming hop 为 256 samples/16 ms。
配置参数放在 `[[chain]]` 节点里,例如 `model`、`library`、`threads`、`noise_gate`;
这让后续 Tauri GUI 可以编辑同一份 `PipelineConfig`,而不是依赖 CLI-only flag。

## 外部虚拟音频设备

Echoless 不创建系统级虚拟麦克风。实时管线会把处理后的人声写入用户选择的
`output` 设备;要让 Discord 等语音应用把它当作麦克风,系统里需要一个外部虚拟
音频设备把 output 端桥接成 input 端。

推荐引导:

- Windows:检测并引导安装 VB-CABLE。Echoless 写入 `CABLE Input`,语音应用选择 `CABLE Output`。
- macOS:检测并引导安装 BlackHole 2ch 或 VB-CABLE MAC。Echoless 写入对应虚拟设备,语音应用选择同名输入设备。
- 用户已有 Virtual Desktop Mic、Loopback 或等价虚拟音频设备时,只要它同时提供可写 output 端和下游可选 input 端,也可以作为 output 候选。

产品集成建议:

- 首版使用“检测 + 显式安装引导 + 安装后验证”。
- 安装器集成第三方驱动时必须让用户清楚知道正在安装虚拟音频设备,并处理管理员权限、重启、许可说明和卸载入口。

## 构建与试跑

```bash
cd echoless
cargo build --release

# 列出处理器种类
cargo run -- processors

# 检查 NVIDIA AFX / RTX AEC runtime
cargo run -- nvafx doctor

# JSON 输出供 GUI/installer 消费
cargo run -- nvafx doctor --json
cargo run -- devices --json
cargo run -- processors --json
cargo run -- config validate --config configs/example.toml --json

# macOS/Windows:主动侦测 reference 与 mic 的对齐延迟
cargo run -- probe-delay --json
# 保留本次校准的 mic/ref/out WAV 与 stats.csv
cargo run -- probe-delay --json --keep-session

# 从本地 zip 安装 RTX AEC runtime 与当前 GPU 架构模型
cargo run -- nvafx install \
    --common-zip echoless-rtx-aec-common-runtime-win64-2.1.0.zip \
    --model-zip echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip

# 列出音频设备
cargo run -- devices

# 实时运行
cargo run --release -- run --config configs/example.toml

# 实时运行并输出前端可消费的 JSONL status(stdout 纯 JSONL,人类提示走 stderr)
cargo run --release -- run --config configs/example.toml --status-json

# 离线跑链
cargo run -p echoless-cli --bin echoless -- offline \
    --mic takes/doubletalk_01.mic.wav \
    --reference takes/doubletalk_01.ref.wav \
    --out out.wav \
    --chain "aec3"

# 或用配置文件
cargo run -p echoless-cli --bin echoless -- offline --mic m.wav --reference r.wav --out o.wav --config configs/example.toml

# RTX AEC 离线快捷命令(Windows RTX 机器 + 已安装 runtime)
cargo run -p echoless-cli --bin echoless -- nvafx offline --mic m.wav --reference r.wav --out rtx.wav

# RTX AEC 实时运行(Windows RTX 机器 + 已安装 runtime)
cargo run -p echoless-cli --bin echoless --release -- run --config configs/example.toml --processor nvidia_afx_aec --reference-channels mono --diagnostic-dir diagnostics/rtx-aec-realtime --diagnostic-seconds 45 --verbose
```

注意：`--diagnostic-seconds` 只限制诊断录音时长，不会自动停止实时进程；录完后按 Ctrl+C 停止。

## GitHub Actions 构建

推送到 `main` 后,`.github/workflows/build.yml` 会在 GitHub-hosted Windows/macOS runner 上:

1. 安装 Rust stable 与 clippy。
2. 运行 `cargo test --workspace --locked`。
3. 运行 `cargo clippy --workspace --all-targets --locked -- -D warnings`。
4. 临时 clone LocalVQE,构建 C API shared library,下载官方 GGUF 跑 regression。
5. 用上一步的 shared library + GGUF 跑 Echoless `localvqe_ffi_smoke`。
6. 生成 release artifact:`echoless-windows-*` / `echoless-macos-*`,并打包 LocalVQE runtime 与当前 v1.3 模型;macOS artifact 同时包含可拖拽安装的 `.dmg`。

已核对的 RTX AEC 集成构建基线:

- GitHub Actions run `27064782614`:Windows/macOS success。
- 代码 commit `b3e4b32f5abdc84c33e5a20ce16febad6f78ded2`;后续 `0bc71a6` 是诊断停止行为说明。
- Artifacts:`echoless-windows-X64`(约 20.8 MiB) / `echoless-macos-ARM64`(约 18.5 MiB)。

## 下一步

1. 确认 NVIDIA AFX runtime/model 再分发许可后,再开放远程下载/公开 release asset。
2. 增加 `eval` 子命令,用 output/input energy ratio 做离线效果量化。
3. `echoless-processors/chain.rs` 占位线性 SRC 换成 rubato 有状态 SRC。
4. 按 `docs/frontend/FRONTEND_ADAPTATION_PLAN.md` 把 CLI sidecar runtime 的 JSON/status/control 合约补齐,同时保留 CLI 一等入口。
5. 前端实现交接见 `docs/frontend/FRONTEND_AGENT_HANDOFF.md`。
6. 产品自更新只预留抽象,不在当前 CLI 后端实现;Velopack / Tauri updater 调研见 `docs/productization/update_strategy.md`。
