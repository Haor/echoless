# Frontend Capability Handoff

本文档给负责 Echoless GUI/Tauri 的前端 agent 使用。它只说明后端能力、产品目的、接口 contract、配置语义和当前边界。信息架构、交互流程、视觉、文案和具体 UI/UX 由前端侧完成。

## 必读文件

- `README.md`
- `configs/example.toml`
- `docs/frontend/FRONTEND_PARAMETER_BOUNDARIES.md`
- `docs/frontend/FRONTEND_ADAPTATION_PLAN.md`
- `docs/localvqe_inference.md`
- `docs/research/rtx_aec_runtime_distribution.md`
- `docs/research/sonora_aec3_internal_map.md`
- `docs/productization/update_strategy.md`

## 产品目的

Echoless 是本地实时 reference-based AEC 工具,目标场景是:

```text
用户使用外放音箱 + 麦克风参加 Discord 等语音连麦
系统播放声音被麦克风重新收到
Echoless 用播放参考声消除麦克风里的回声
处理后的人声送到外部虚拟音频设备
```

核心链路:

```text
microphone near-end
  + system/reference far-end
  -> selected echo backend
  -> virtual audio output device
  -> downstream app
```

产品默认策略:

- 默认 backend: `sonora_aec3`。
- 默认采样: `48000 Hz / 10ms frame`。
- 默认 reference: `mono`。
- 默认音质策略: AEC 保真人声,NS/AGC 默认关闭。
- LocalVQE 和 RTX AEC 是独立可选 backend。
- 现有 CLI 保留为一等入口,GUI 通过同一套配置和 JSON contract 接入。

## 外部虚拟音频设备

Echoless 输出需要一个系统可见的虚拟音频设备,让 Discord 等语音应用能把它当作麦克风输入。

### Windows

推荐设备: VB-Audio VB-CABLE。

当前可行路径:

- Echoless 枚举设备,识别 `CABLE Input` / `VB-Audio Virtual Cable`。
- 用户已经安装时,前端把该设备作为 `output` 候选。
- 用户未安装时,前端可以提供显式的安装引导或安装器入口。
- 安装后重新枚举设备,确认 `CABLE Input` 出现在输出设备列表、`CABLE Output` 出现在输入设备列表。

调研结论:

- VB-Audio 官方说明 VB-CABLE 是 Windows audio driver / virtual audio cable。
- 官方安装步骤是解压 ZIP、以管理员运行 setup、安装/卸载后重启。
- 官方 licensing 页允许把公开 VB-CABLE package 随应用分发或嵌入安装包,包括 silent installation,前提是 donationware 模型对终端用户可见,并清楚标注来源和 donationware 属性。
- 面向公司/机构/专业分发时需要按官方 licensing 购买或确认授权。

建议产品口径:

- 首版使用“检测 + 显式安装引导 + 安装后验证”。
- 若后续把 VB-CABLE 嵌入安装器,安装流程必须让用户清楚知道正在安装 VB-Audio 的驱动,并处理 UAC、重启、许可声明和卸载入口。
- VB-CABLE A+B / C+D 的分发规则不同,首版只围绕基础 VB-CABLE。

参考来源:

- https://vb-audio.com/Cable/
- https://vb-audio.com/Services/licensing.htm

### macOS

推荐设备: BlackHole 2ch 或 VB-CABLE MAC。用户已经安装其他虚拟音频设备时,只有在 `devices --json` 枚举到可写 output endpoint 后才作为输出候选。

当前可行路径:

- `mic` 选择 Mac 原生麦克风或用户指定麦克风。
- `reference` 选择当前系统可见的参考源;如果系统声音无法直接作为 reference,使用 BlackHole/VB-CABLE MAC 等虚拟路由设备。
- `output` 选择 `devices --json` 枚举到的虚拟音频 output;下游应用再选择同一虚拟设备对应的输入端。

额外安装/授权点:

- BlackHole 是 macOS virtual audio loopback driver,官方支持 installer 和 Homebrew cask。
- BlackHole 安装说明要求关闭音频应用,打开 pkg 安装,需要时重启;Homebrew cask 页面也标注安装后需要 reboot。
- VB-CABLE MAC 是 VB-Audio 的 macOS audio driver,支持 Intel / Apple Silicon,许可证为 Donationware Simple。
- Echoless/Tauri app 需要 macOS 麦克风权限;如果后续使用更深的系统音频捕获能力,还要按实际实现处理对应系统权限。

参考来源:

- https://github.com/ExistentialAudio/BlackHole
- https://github.com/ExistentialAudio/BlackHole/wiki/Installation
- https://formulae.brew.sh/cask/blackhole-2ch
- https://shop.vb-audio.com/en/mac-apps/29-vb-cable-mac.html

## 当前功能清单

功能和参数暴露边界以 `docs/frontend/FRONTEND_PARAMETER_BOUNDARIES.md` 为准。Open Design / HTML prototype 只能作为视觉原型,不能作为配置 contract。

| 能力 | 后端状态 | 前端使用方式 |
|---|---|---|
| 设备枚举 | 可用 | `echoless devices --json` |
| 处理器枚举 | 可用 | `echoless processors --json` |
| 配置校验 | 可用 | `echoless config validate --config <file> --json` |
| AEC3 realtime | 默认主路径 | `echoless run --config <file> --status-json` |
| Reference mono/stereo | 可配置 | `reference_channels = "mono" | "stereo"` |
| Diagnostics recording | 可用 | `--diagnostic-dir` / config diagnostics |
| Runtime status | 可用 | JSONL status events |
| LocalVQE | 可选实验 backend | 需要模型和动态库 |
| RTX AEC | Windows 可选 backend | 需要 `nvafx doctor --json` 通过 |
| Passthrough | 可用 | 诊断路由和设备链路 |
| Offline processing | 可用 | `echoless offline ...` |

## 默认配置

```toml
mic = "default"
reference = "system"
output = "default"
sample_rate = 48000
frame_ms = 10
reference_channels = "mono"

[[chain]]
kind = "sonora_aec3"
ns = false
agc = false
```

## 可配置参数

这些是后端可理解的配置项。前端可以自行决定具体 UI,但参数值、平台约束和暴露层级不要突破 `FRONTEND_PARAMETER_BOUNDARIES.md`。

### 通用管线

| 参数 | 默认 | 含义 |
|---|---:|---|
| `mic` | `default` | 麦克风输入设备 |
| `reference` | `system` | far-end 参考源 |
| `output` | `default` | 输出设备,通常是虚拟音频设备 |
| `sample_rate` | `48000` | 管线采样率 |
| `frame_ms` | `10` | realtime frame 大小 |
| `reference_channels` | `mono` | far reference 声道模式 |
| `diagnostics.record_dir` | null | diagnostics session 输出目录 |
| `diagnostics.max_seconds` | null | diagnostics 最大录制秒数 |

### AEC3

| 参数 | 默认 | 建议层级 | 含义 |
|---|---:|---|---|
| `ns` | `false` | 常规/高级均可 | AEC3 内置降噪开关 |
| `ns_level` | `low` | 高级 | `low/moderate/high/veryhigh` |
| `agc` | `false` | 高级 | 自动增益 |
| `initial_delay_ms` | null | 高级 | AEC3 初始延迟 hint;运行时仍会估计回声对齐 |
| `tail_ms` | null | 高级 | echo tail 长度 |
| `delay_num_filters` | null | 高级 | 延迟搜索窗大小 |
| `linear_stable_echo_path` | `false` | 高级 | 稳定线性 echo path 配置 |

### LocalVQE

| 参数 | 默认 | 建议层级 | 含义 |
|---|---:|---|---|
| `model` | required | 实验 | GGUF 模型路径 |
| `library` | auto | 实验 | LocalVQE 动态库路径 |
| `threads` | auto | 高级 | CPU 线程数 |
| `backend` | auto | 高级 | 上游 backend 字符串 hint |
| `device` | auto | 高级 | 上游 device 数字 index;不填表示 auto |
| `noise_gate` | `false` | 高级 | LocalVQE noise gate |
| `noise_gate_threshold_dbfs` | `-45.0` | 高级 | noise gate 阈值 |

### RTX AEC

| 参数 | 默认 | 建议层级 | 含义 |
|---|---:|---|---|
| `runtime_dir` | auto | 高级 | NVIDIA AFX runtime 目录 |
| `model_path` | auto | 高级 | RTX AEC model 路径 |
| `intensity_ratio` | `1.0` | 高级 | RTX AEC 强度 |
| `use_default_gpu` | `true` | 高级 | 使用默认 GPU |
| `disable_cuda_graph` | `false` | 高级 | CUDA graph 开关 |
| `on_runtime_error` | `silence` | 高级 | `silence` 或 `bypass` |

## CLI 和 JSON Contract

人工调试命令:

```bash
echoless devices
echoless processors
echoless run --config configs/example.toml
echoless run --config configs/example.toml --diagnostic-dir diagnostics/aec3 --diagnostic-seconds 45 --verbose
echoless offline --mic m.wav --reference r.wav --out o.wav --chain sonora_aec3
echoless nvafx doctor
```

前端消费命令:

```bash
echoless devices --json
echoless processors --json
echoless config validate --config config.toml --json
echoless run --config config.toml --status-json
echoless nvafx doctor --json
```

集成规则:

- `run --status-json` 的 stdout 是 JSONL status events。
- 人类日志走 stderr。
- `devices --json` 返回输入、输出、reference source。
- `processors --json` 返回 backend 能力、平台约束、参数类型、默认值和 `advanced` 标记。
- `config validate --json` 返回 `{ ok, errors }`;配置无效时 exit code 为非 0。

## Runtime Status

`run --status-json` 每隔 `--stats-interval-ms` 输出一条状态事件。核心字段:

```ts
export interface RuntimeStatus {
  type: "status";
  elapsed_s: number;
  frames: number;
  sample_rate: number;
  frame_ms: number;
  backend: string;
  mic_dbfs: number;
  ref_dbfs: number;
  out_dbfs: number;
  mic_q_samples: number;
  ref_q_samples: number;
  out_q_samples: number;
  output_queue_latency_ms: number;
  algorithmic_latency_ms: number;
  estimated_user_latency_ms: number;
  aec_estimated_delay_ms: number;
  mic_input_drops: number;
  ref_input_drops: number;
  input_drops: number;
  ref_underruns: number;
  output_underruns: number;
  output_overruns: number;
  stale_drops: number;
  node_process_time_ms: number;
  runtime_errors: number;
  diverged: boolean;
  last_backend_error?: string | null;
  diagnostics_session_dir?: string | null;
}
```

延迟字段语义:

```text
estimated_user_latency_ms =
  frame_ms / 2
  + algorithmic_latency_ms
  + out_q_samples / sample_rate * 1000
```

- `estimated_user_latency_ms`: 用户说话进入麦克风到送入虚拟输出设备的估算延迟。
- `aec_estimated_delay_ms`: AEC3 估计的回声路径对齐延迟。
- `output_queue_latency_ms`: 输出队列贡献的可见延迟。

## Diagnostics

实时 diagnostics 可写出:

- `mic.wav`
- `ref.wav`
- `out.wav`
- `stats.csv`
- metadata

用途:

- Windows/macOS 真机测试。
- 用户反馈回声、断音、音量骤降、延迟时收集证据。
- AEC3 / LocalVQE / RTX AEC 对比。
- 回归分析。

## 前端接入验证点

- 能读取 `echoless devices --json`。
- 能读取 `echoless processors --json`。
- 能生成并校验 `PipelineConfig`。
- 能启动/停止 `sonora_aec3` 默认配置。
- 能读取 `run --status-json` 的 JSONL。
- 能区分 `estimated_user_latency_ms` 与 `aec_estimated_delay_ms`。
- 能触发 diagnostics 并拿到 session 路径。
- 能识别 Windows 虚拟音频设备是否已安装。
- 能识别 macOS 可用虚拟音频设备。
- Windows 上能根据 `nvafx doctor --json` 判断 RTX AEC 可用性。
- macOS 上能把 RTX AEC 标记为 unsupported。
- 配置变化后能重启 runtime 并应用新配置。
- `echoless run --config configs/example.toml` CLI 仍可人工使用。
