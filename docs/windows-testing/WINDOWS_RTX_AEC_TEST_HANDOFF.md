# Windows RTX AEC 测试 Handoff

## 目标

在 Windows 侧评估 NVIDIA RTX / Maxine Audio Effects SDK 的 AEC 能力，并用 Echoless 新增的诊断录制能力生成同源证据。当前产品主线仍是 `sonora_aec3` 保真优先；RTX AEC 只作为可选 backend / 对照路线评估，不要默认与 AEC3 级联。

## 当前代码状态

- Echoless 实时链路新增诊断录制：
  - CLI: `--diagnostic-dir <DIR>`
  - CLI: `--diagnostic-seconds <SECONDS>`
  - 配置: `[diagnostics] record_dir = "..."; max_seconds = 30`
- 诊断 session 输出：
  - `mic.wav`: 原始麦克风 mono float WAV
  - `ref.wav`: far reference float WAV；mono/stereo 取决于 `reference_channels`
  - `out.wav`: Echoless 处理后 mono float WAV
  - `stats.csv`: 每帧 dBFS、queue、drop、underrun、overrun
  - `metadata.txt`: sample rate、frame、reference channel 等
- LocalVQE 打包/示例已切到当前上游默认模型：
  - `models/localvqe-v1.3-4.8M-f32.gguf`
  - v1.2 仍只作为低 CPU 或 v1.3 过激时的 fallback。

## 当前 GitHub Actions artifact

- Repo: `Haor/echoless`
- Run: `27058593366`
- URL: <https://github.com/Haor/echoless/actions/runs/27058593366>
- Subtree commit: `81c3f810d23e4d655f31f2bbbc5046bb691b58cc`
- Status: success
- Artifact:
  - `echoless-windows-X64`
  - `echoless-macos-ARM64`

## Windows 侧应拿到的文件

除 GitHub Actions artifact 外，建议同时提供 `research/` 下全部 4 个文件，因为它们体积不大且互补：

1. `research/windows_aec_research.md`
   - 主调研文档，先看这里。包含 NVIDIA Maxine AEC、AEC3、LocalVQE、验证矩阵、风险清单。
2. `research/reference_repos_exploration_report.md`
   - 源码级核实报告。用于确认 LocalVQE C API、Maxine API 形态、虚拟麦路线和参考仓库证据。
3. `research/sonora_aec3_internal_map.md`
   - AEC3 / sonora 当前可调能力、delay、stereo far reference、tail、NS/AGC 风险。
4. `research/cross_platform_architecture.md`
   - HAL/Core/Processor/CLI/GUI 分层蓝本。Windows agent 如果要判断 RTX backend 应该挂在哪里，需要读这个。

最低集合是 1 + 2 + 3；如果 Windows agent 会改架构或 GUI 配置面，把 4 也必须带上。

## NVIDIA AFX SDK 基线事实

官方文档入口：

- Windows 环境要求: <https://docs.nvidia.com/maxine/afx/2.1.0/WindowsAFXSDK/GetStartedOnWindows.html>
- AEC effect 说明: <https://docs.nvidia.com/maxine/afx/latest/AboutTheEffects/AboutAcousticEchoCancellation.html>

截至 2026-06-06 官方文档显示：

- 需要带 Tensor Core 的 NVIDIA GPU。
- Windows 要求 64-bit Windows 10，驱动要求 572.61 或更新。
- SDK Installer 已包含 CUDA / TensorRT 依赖和所需库。
- AEC 支持 16 kHz 或 48 kHz、32-bit float 音频。
- AEC 是 reference-based：输入需要 near-end mic 信号和 far-end reference 信号。

注意：这里的 “RTX AEC” 指 NVIDIA Audio Effects / Maxine AFX SDK 的 AEC effect。如果测试 NVIDIA Broadcast 图形软件的 Echo Removal，也请单独记录为 closed-box downstream 测试，不要和 SDK AEC 混写。

## 测试顺序

### 1. 获取新 artifact

从 GitHub Actions 下载最新 `echoless-windows-*` artifact。解压后确认至少有：

- `echoless.exe`
- `example.toml`
- `localvqe.dll`
- GGML / LocalVQE 依赖 DLL
- `models/localvqe-v1.3-4.8M-f32.gguf`

### 2. 设备枚举

```powershell
.\echoless.exe devices
```

记录：

- USB 麦克风索引/名称
- 系统外放设备索引/名称
- VB-Cable / 虚拟麦输出设备索引/名称

### 3. AEC3 保真基线加诊断录制

用 artifact 里的 `example.toml` 作为起点。默认 AEC3 已是 `ns=false`、`agc=false`。

```powershell
.\echoless.exe run --config .\example.toml `
  --mic "<USB mic name or index>" `
  --reference system `
  --output "<VB-Cable input or monitor output>" `
  --reference-channels mono `
  --diagnostic-dir .\diagnostics\aec3-mono `
  --diagnostic-seconds 45 `
  --verbose
```

再跑 stereo far reference 对照：

```powershell
.\echoless.exe run --config .\example.toml `
  --mic "<USB mic name or index>" `
  --reference system `
  --output "<VB-Cable input or monitor output>" `
  --reference-channels stereo `
  --diagnostic-dir .\diagnostics\aec3-stereo `
  --diagnostic-seconds 45 `
  --verbose
```

每轮场景：

- near-only：只说话，不播放声音。
- far-only：播放人声/视频，自己不说话。
- double-talk：播放人声，同时自己说话。
- movement：自己从麦克风左侧移动到右侧说话，观察立体参考是否影响回声消除。

### 4. LocalVQE v1.3 standalone 可选测试

复制 `example.toml` 为 `localvqe-v13.toml`，注释掉 `sonora_aec3` 块，启用：

```toml
[[chain]]
kind = "localvqe"
model = "models/localvqe-v1.3-4.8M-f32.gguf"
library = "localvqe.dll"
threads = 2
noise_gate = false
```

运行：

```powershell
.\echoless.exe run --config .\localvqe-v13.toml `
  --mic "<USB mic name or index>" `
  --reference system `
  --output "<VB-Cable input or monitor output>" `
  --diagnostic-dir .\diagnostics\localvqe-v13 `
  --diagnostic-seconds 45 `
  --verbose
```

不要把 AEC3 + LocalVQE 级联作为默认测试结论。之前听感已经显示级联更容易加重电音/锯齿。

### 5. NVIDIA RTX / AFX AEC 验证

目标分两层：

1. SDK 可用性验证：
   - 安装 AFX SDK。
   - 确认 GPU、驱动、SDK 版本。
   - 运行官方 AEC sample。官方示例命令形态：

```bat
run_effect_demo.bat turing aec 16k 16k
run_effect_demo.bat ampere aec 48k 48k
```

2. 与 Echoless 同源样本对照：
   - 优先使用 `diagnostics/aec3-*` 里的 `mic.wav` 和 `ref.wav` 作为 NVIDIA AEC 的 near/far 输入。
   - 如果官方 sample 不支持直接喂入这两路 WAV，则记录 sample 限制，并用 SDK API 或最小 harness 跑离线对照。
   - 如果短期只能测试 NVIDIA Broadcast GUI，则把它作为 downstream closed-box 测试：`Echoless AEC3 out -> Broadcast -> Discord/VRChat`，单独记录，不要说成 SDK AEC 结果。

RTX AEC 不要与 AEC3 串联后直接下结论。先做：

- AEC3 mono
- AEC3 stereo
- RTX AEC standalone
- AEC3 mono + NVIDIA Broadcast downstream

## 结果回传格式

写到 `WINDOWS_RTX_AEC_TEST_RESULTS.md`，建议包含：

- 机器信息：Windows 版本、GPU、驱动、SDK/Broadcast 版本、麦克风、外放设备、虚拟麦设备。
- Artifact 信息：Actions run id、commit SHA、压缩包名、解压文件列表。
- 每个场景的命令、配置文件、诊断 session 路径。
- 主观听感：人声保真、锯齿/电音、忽大忽小、回声残留、音乐/视频残留、双讲稳定性。
- 客观线索：`stats.csv` 中 queue 是否持续增长、drop/underrun/overrun 是否非零、CPU/GPU 占用。
- 结论分级：
  - `ship-default`: 可作为默认。
  - `optional`: 可作为用户可选项。
  - `compare-only`: 只适合对照。
  - `reject-for-now`: 当前音质或部署不合格。

## 已知风险

- LocalVQE v1.3 更强但更重，也可能比 v1.2 更 aggressive；它不是当前默认主线。
- AEC3 的 double-talk 忽大忽小优先从 `ns=false`、`agc=false`、tail、reference_channels、下游 Broadcast 串联顺序排查。
- NVIDIA AFX SDK 是外部二进制 SDK。先评估 license、redistribution、模型下载/安装、驱动门槛，再考虑正式集成。
- 如果 Windows 侧只测试 Broadcast GUI，结论只能用于用户工作流，不等价于我们可嵌入 SDK backend。

## 下一步建议

Windows agent 完成测试后，把 `WINDOWS_RTX_AEC_TEST_RESULTS.md` 与至少一组诊断 session 打包回传。Mac 侧再根据样本决定：

- 是否只保留 AEC3 + downstream Broadcast 的产品路线。
- 是否把 RTX AEC 做成独立 backend。
- 是否需要给 GUI 增加 `diagnostics`、`reference_channels`、`ns/agc/tail`、backend 切换的调参面板。
