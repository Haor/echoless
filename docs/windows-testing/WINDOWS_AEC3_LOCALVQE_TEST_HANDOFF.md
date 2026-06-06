# Windows AEC3 / LocalVQE v1.3 音质测试 Handoff

## 目标

Windows 侧先测试 Echoless 当前已经可运行的两条路径：

1. `sonora_aec3` 保真基线
2. `localvqe` v1.3 standalone

只有第一阶段完成、诊断录制样本已经生成后，再进入 RTX / NVIDIA AFX AEC 调研。不要一开始就安装或测试 RTX AEC，也不要把 `AEC3 -> LocalVQE` 级联作为默认结论。

## 当前 artifact

- Repo: `Haor/echoless`
- Actions run: <https://github.com/Haor/echoless/actions/runs/27058593366>
- Subtree commit: `81c3f810d23e4d655f31f2bbbc5046bb691b58cc`
- Artifact: `echoless-windows-X64`

解压后至少应确认：

- `echoless.exe`
- `example.toml`
- `localvqe.dll`
- GGML / LocalVQE 依赖 DLL
- `models/localvqe-v1.3-4.8M-f32.gguf`

## 必读文档

从当前 GitHub 仓库读取：

1. `docs/windows-testing/WINDOWS_AEC3_LOCALVQE_TEST_HANDOFF.md`
2. `docs/windows-testing/WINDOWS_RTX_AEC_TEST_HANDOFF.md`
3. `docs/research/windows_aec_research.md`
4. `docs/research/sonora_aec3_internal_map.md`
5. `docs/research/reference_repos_exploration_report.md`
6. `docs/research/cross_platform_architecture.md`

阅读重点：

- AEC3 为什么是当前主线。
- LocalVQE v1.3 是独立可选方案，不是默认 AEC3 后级。
- `ns=false`、`agc=false` 是当前保真优先默认。
- `reference_channels=mono/stereo` 的差异需要实听。
- RTX AEC 是后续 SDK backend 候选，不等于 NVIDIA Broadcast GUI。

## 第一阶段：AEC3 音质基线

先枚举设备：

```powershell
.\echoless.exe devices
```

记录：

- USB 麦克风名称/索引
- 系统外放设备名称/索引
- VB-Cable / 虚拟麦输出名称/索引

### AEC3 mono far reference

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

### AEC3 stereo far reference

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

每轮测试场景：

- near-only：只说话，不播放声音。
- far-only：播放人声/视频，自己不说话。
- double-talk：播放人声，同时自己说话。
- movement：人在麦克风左右移动，听回声残留和人声稳定性。

## 第二阶段：LocalVQE v1.3 standalone

复制 `example.toml` 为 `localvqe-v13.toml`，注释掉 `sonora_aec3`，启用：

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

对照同样四个场景：near-only、far-only、double-talk、movement。

## 诊断录制输出

每个 `diagnostics/<case>/session-*` 目录应包含：

- `mic.wav`: 原始麦克风输入
- `ref.wav`: far reference
- `out.wav`: 处理后输出
- `stats.csv`: 每帧电平、queue、drop、underrun、overrun
- `metadata.txt`: sample rate、frame、reference channel 等

主观听感和 `stats.csv` 要一起看：

- 如果听到卡顿/断续，检查 drop、underrun、overrun。
- 如果 double-talk 忽大忽小，优先确认 `agc=false`、`ns=false`、下游链路没有再做自动增益。
- 如果 stereo 比 mono 更差，记录具体场景；stereo far reference 是实验项，不是默认承诺。

## 第三阶段：RTX AEC 调研

第一阶段和第二阶段完成后，才阅读并执行：

- `docs/windows-testing/WINDOWS_RTX_AEC_TEST_HANDOFF.md`

RTX 调研要尽量复用前面生成的 `mic.wav` / `ref.wav`，避免用不同说话内容、不同房间状态做比较。

如果只能测试 NVIDIA Broadcast GUI，请明确标记为 downstream closed-box 测试，不要写成 NVIDIA AFX SDK AEC backend 结果。

## 结果文件

先输出：

- `WINDOWS_AEC3_LOCALVQE_TEST_RESULTS.md`

如果完成 RTX 调研，再输出：

- `WINDOWS_RTX_AEC_TEST_RESULTS.md`

`WINDOWS_AEC3_LOCALVQE_TEST_RESULTS.md` 至少包含：

- 机器信息：Windows 版本、CPU、GPU、驱动、麦克风、外放设备、虚拟麦设备。
- Artifact 信息：Actions run id、commit SHA、artifact 名称。
- 每个 case 的命令、配置、诊断 session 路径。
- 主观听感：人声保真、锯齿/电音、忽大忽小、回声残留、双讲稳定性。
- 客观线索：`stats.csv` 是否有 drop / underrun / overrun / queue 持续增长。
- 结论：AEC3 mono、AEC3 stereo、LocalVQE v1.3 分别是 `ship-default` / `optional` / `compare-only` / `reject-for-now`。
