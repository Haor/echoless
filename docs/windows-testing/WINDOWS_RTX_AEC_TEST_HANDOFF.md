# Windows RTX AEC 测试 Handoff

## 目标

在 Windows 侧评估 NVIDIA RTX / Maxine Audio Effects SDK 的 AEC 能力，并用 Echoless 新增的诊断录制能力生成同源证据。当前产品主线仍是 `sonora_aec3` 保真优先；RTX AEC 只作为可选 backend / 对照路线评估，不要默认与 AEC3 级联。

## 2026-06-06 Windows 实测更新

本轮已经在 Windows 侧跑通 NVIDIA AFX SDK AEC 离线对照，并完成 Echoless RTX AEC standalone 实时 smoke：

- SDK：NVIDIA AFX SDK Windows 2.1.0 / `2026-03-11_NVIDIA_AFX_SDK_Win_v2.1.0.9`
- GPU：RTX 5080 / Blackwell，driver `596.49`
- NGC CLI：4.19.0，Windows core SDK resource 访问通过
- Toolchain：CMake 4.3.3，Visual Studio 2022 Build Tools / MSVC 19.44
- AEC feature/model：
  - `features\nvafxaec\bin\nvafxaec.dll`
  - `features\nvafxaec\models\blackwell\aec_16k.trtpkg`
  - `features\nvafxaec\models\blackwell\aec_48k.trtpkg`
- 官方 sample 已从 SDK 2.1 自带 samples 编译成功：
  - `samples\build\Release\effects_demo.exe`
- Echoless 实时 RTX AEC:
  - mic: USB mic index `4`
  - reference: `system`
  - output: index `3` / CABLE Input
  - `--diagnostic-seconds 45` 录制成功
  - `stats.csv` / runtime stats: `runtime_errors=0`

重要踩坑：

- 不要用旧的 `NVIDIA-Maxine/Maxine-AFX-SDK` v1.3 sample 对 SDK 2.1 runtime/model；会出现 DLL/入口点不匹配。
- SDK 2.1 的 feature 下载脚本实际参数名是 `--ngc_org` / `--ngc_team`，不是连字符版本。
- `download_features.ps1` 读取 `NGC_CLI_API_KEY` 或 `NGC_API_KEY`，本轮没有自动读取 `ngc config`。
- 该脚本会把 NGC API key 打到控制台输出；下载后建议轮换 key，不要提交下载日志。
- 官方 `effects_demo` 不能读取 Echoless 当前诊断 WAV 的 `WAVE_FORMAT_EXTENSIBLE` 头；需要转成普通 IEEE_FLOAT WAV，样本数据可以不变。

本轮同源离线对照输入：

```text
diagnostics/aec3-mono-round1/session-1780739447/mic.wav
diagnostics/aec3-mono-round1/session-1780739447/ref.wav
```

输出：

```text
diagnostics/rtx-aec-offline-inputs/echoless-aec3-mono-round1/nvafx_aec_out.wav
diagnostics/rtx-aec-offline-inputs/echoless-aec3-mono-round1/nvafx_aec_segment_summary.csv
diagnostics/logs/nvafx-aec-offline-round1.stdout.log
```

NVIDIA sample 关键日志：

```text
Input Sample rate: 48000
Output Sample rate: 48000
Input Channels: 2
Output Channels: 1
Input Samples per frame: 480
Output Samples per frame: 480
Processing time: about 1.2-1.3s for 44.9985s audio
```

分段能量摘要：

| Case | Segment | mic dBFS | ref dBFS | out dBFS | out - mic |
|---|---:|---:|---:|---:|---:|
| RTX AEC 48k offline | 0-10s far-only | -47.67 | -33.79 | -62.08 | -14.41 |
| RTX AEC 48k offline | 10-25s double-talk | -33.37 | -23.89 | -35.84 | -2.48 |
| RTX AEC 48k offline | 25-35s movement | -33.97 | -21.09 | -37.76 | -3.79 |
| RTX AEC 48k offline | 35-45s near-only | -40.75 | -30.34 | -42.81 | -2.06 |

当前结论：

- RTX AEC SDK 离线链路已跑通，可进入主观 AB。
- Echoless `nvidia_afx_aec` 离线 / 实时路径已跑通；当前把它保留为 Windows RTX 用户的独立可选 backend。
- 仍不能把它列为默认方案；还缺更长时间实时稳定性、delay/drift 对齐、GPU 满载稳定性、主观 AB、分发许可确认。

## 2026-06-06 Runtime 分发准备

已经准备好 “common runtime zip + per-arch model zip” 的本地 staging：

```text
C:\Users\haor2\workspace\aec\runtime-packages\dist-rtx-aec-2.1.0-aec48-split
```

最终 release asset 形态：

- `echoless-rtx-aec-common-runtime-win64-2.1.0.zip`
- `echoless-rtx-aec-model-win64-2.1.0-turing-aec48.zip`
- `echoless-rtx-aec-model-win64-2.1.0-ampere-aec48.zip`
- `echoless-rtx-aec-model-win64-2.1.0-ada-aec48.zip`
- `echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip`
- `manifest.json`

每个模型 zip 可直接解压到 runtime 根目录，内部路径为：

```text
features/nvafxaec/models/<arch>/aec_48k.trtpkg
```

安装器 / doctor 检测失败时只提示用户安装缺失组件，不要求普通用户安装 SDK：

- 缺 `nvidia-smi` 或 `nvcuda.dll`：安装 NVIDIA graphics driver。
- driver 低于 `572.61`：更新 NVIDIA graphics driver。
- 缺 `VCRUNTIME140.dll` / `VCRUNTIME140_1.dll` / `MSVCP140.dll`：安装 Microsoft Visual C++ 2015-2022 Redistributable x64。
- GPU compute capability 不在 `75`、`80`、`86`、`89`、`100`、`120`：RTX AEC backend 不可用。
- 缺模型：按 GPU 架构下载对应 model zip 并解压。

详细分发设计见 `docs/research/rtx_aec_runtime_distribution.md`。在确认 NVIDIA AFX SDK / model 再分发许可前，不要公开上传二进制。

## 当前代码状态

- NVIDIA AFX / RTX AEC 已作为 Windows-only 可选 backend 接入：
  - CLI: `echoless nvafx doctor [--runtime-dir <DIR>] [--json]`
  - CLI: `echoless nvafx install --common-zip <ZIP> --model-zip <ZIP>`
  - CLI: `echoless nvafx offline --mic <mic.wav> --reference <ref.wav> --out <out.wav>`
  - 实时: `echoless run --processor nvidia_afx_aec --reference-channels mono`
  - v1 约束: 48 kHz / 10 ms / mono mic / mono reference / mono output
- Echoless 实时链路新增诊断录制：
  - CLI: `--diagnostic-dir <DIR>`
  - CLI: `--diagnostic-seconds <SECONDS>`
  - 配置: `[diagnostics] record_dir = "..."; max_seconds = 30`
- 诊断 session 输出：
  - `mic.wav`: 原始麦克风 mono float WAV
  - `ref.wav`: far reference float WAV；mono/stereo 取决于 `reference_channels`
  - `out.wav`: Echoless 处理后 mono float WAV
  - `stats.csv`: 每帧 dBFS、queue、drop、underrun、overrun、node process time、runtime error
  - `metadata.txt`: sample rate、frame、reference channel、RTX 选中的 GPU 架构与模型等
- LocalVQE 打包/示例已切到当前上游默认模型：
  - `models/localvqe-v1.3-4.8M-f32.gguf`
  - v1.2 仍只作为低 CPU 或 v1.3 过激时的 fallback。

## 已验证 GitHub Actions artifact 基线

- Repo: `Haor/echoless`
- Run: `27064782614`
- URL: <https://github.com/Haor/echoless/actions/runs/27064782614>
- Code commit: `b3e4b32f5abdc84c33e5a20ce16febad6f78ded2`
- Branch follow-up: `0bc71a6ecd8889617f96e8190dde4c5858c1c265` 只补充实时诊断停止行为说明。
- Status: success
- Artifact:
  - `echoless-windows-X64` / 21,811,400 bytes
  - `echoless-macos-ARM64` / 19,402,204 bytes

注意：`.github/workflows/build.yml` 只在 `main` push 自动跑；`nvafx-full-integration` 分支需要手动 `workflow_dispatch` 才会生成分支 artifact。

## Windows 侧应拿到的文件

除 GitHub Actions artifact 外，建议同时提供 `docs/research/` 下全部 4 个文件，因为它们体积不大且互补：

1. `docs/research/windows_aec_research.md`
   - 主调研文档，先看这里。包含 NVIDIA Maxine AEC、AEC3、LocalVQE、验证矩阵、风险清单。
2. `docs/research/reference_repos_exploration_report.md`
   - 源码级核实报告。用于确认 LocalVQE C API、Maxine API 形态、虚拟麦路线和参考仓库证据。
3. `docs/research/sonora_aec3_internal_map.md`
   - AEC3 / sonora 当前可调能力、delay、stereo far reference、tail、NS/AGC 风险。
4. `docs/research/cross_platform_architecture.md`
   - Audio I/O/Core/Processor/CLI/GUI 分层蓝本。Windows agent 如果要判断 RTX backend 应该挂在哪里，需要读这个。

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

注意：这里的 “RTX AEC” 指 NVIDIA Audio Effects / Maxine AFX SDK 的 AEC effect，本轮测试不纳入其他闭源 GUI 后处理链。

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

目标分三层：

1. Runtime 可用性验证：
   - 不要求普通测试者安装完整 AFX SDK。
   - 如果已有 prepared runtime zip，先安装：

```powershell
.\echoless.exe nvafx install `
  --common-zip .\echoless-rtx-aec-common-runtime-win64-2.1.0.zip `
  --model-zip .\echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip
```

   - 或指向已解压的 runtime：

```powershell
.\echoless.exe nvafx doctor --runtime-dir "C:\Users\haor2\workspace\aec\runtime-packages\echoless-rtx-aec-runtime-win64-blackwell-2.1.0-aec48"
```

2. 与 Echoless 同源样本离线对照：
   - 优先使用 `diagnostics/aec3-*` 里的 `mic.wav` 和 `ref.wav` 作为 near/far 输入。

```powershell
.\echoless.exe nvafx offline `
  --mic .\diagnostics\aec3-mono-round1\session-1780739447\mic.wav `
  --reference .\diagnostics\aec3-mono-round1\session-1780739447\ref.wav `
  --out .\diagnostics\rtx-aec-standalone\nvafx_aec_out.wav
```

3. 实时 RTX AEC standalone：

本机从 repo 根目录直接复制这一行：

```powershell
$env:ECHOLESS_NVAFX_RUNTIME_DIR='C:\Users\haor2\workspace\aec\runtime-packages\echoless-rtx-aec-runtime-win64-blackwell-2.1.0-aec48'; .\target\release\echoless.exe run --config .\configs\example.toml --mic 4 --reference system --output 3 --processor nvidia_afx_aec --reference-channels mono --diagnostic-dir .\diagnostics\rtx-aec-realtime --diagnostic-seconds 45 --verbose
```

`--diagnostic-seconds 45` 只会让诊断录制在 45 秒后 finalize；实时程序会继续运行，看到录制文件写完后按 Ctrl+C 停止。

如果从 GitHub artifact 解压目录运行，把 `.\target\release\echoless.exe` 改成 `.\echoless.exe`，并把 `--config .\configs\example.toml` 改成 `--config .\example.toml`。

```powershell
.\echoless.exe run --config .\example.toml `
  --mic "<USB mic name or index>" `
  --reference system `
  --output "<VB-Cable input or monitor output>" `
  --processor nvidia_afx_aec `
  --reference-channels mono `
  --diagnostic-dir .\diagnostics\rtx-aec-realtime `
  --diagnostic-seconds 45 `
  --verbose
```

RTX AEC 不要与 AEC3 串联后直接下结论。先做：

- AEC3 mono
- AEC3 stereo
- RTX AEC standalone

## macOS / 跨平台边界

- macOS artifact 只用于验证 AEC3、LocalVQE、配置解析、CLI/GUI 基础路径。
- NVIDIA AFX / RTX AEC backend 目前是 Windows x64 only；macOS 不应尝试安装 RTX runtime 或运行 `nvafx offline/install`。
- `echoless nvafx doctor --json` 可以作为 GUI/安装器的统一能力探针；macOS 上应看到 `ok=false`，并包含 `platform=unsupported` 检查项。
- GUI 默认 backend 仍应是 `sonora_aec3`；RTX AEC 只在 doctor 通过的 Windows RTX 机器上显示为可选 backend。

## 结果回传格式

写到 `WINDOWS_RTX_AEC_TEST_RESULTS.md`，建议包含：

- 机器信息：Windows 版本、GPU、驱动、AFX runtime 版本、麦克风、外放设备、虚拟麦设备。
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
- AEC3 的 double-talk 忽大忽小优先从 `ns=false`、`agc=false`、tail、reference_channels 排查。
- NVIDIA AFX SDK 是外部二进制 SDK。先评估 license、redistribution、模型下载/安装、驱动门槛，再考虑正式集成。

## 下一步建议

Windows agent 完成测试后，把 `WINDOWS_RTX_AEC_TEST_RESULTS.md` 与至少一组诊断 session 打包回传。Mac 侧再根据样本决定：

- 是否保留 RTX AEC 为 Windows RTX 用户的独立 backend。
- 是否需要给 GUI 增加 `diagnostics`、`reference_channels`、`ns/agc/tail`、backend 切换的调参面板。
