# Frontend Capability Handoff

本文档给负责 Echoless GUI/Tauri 的前端 agent 使用。它只说明后端能力、产品目的、接口 contract、配置语义和当前边界。信息架构、交互流程、视觉、文案和具体 UI/UX 由前端侧完成。

## 必读文件

- `README.md`
- `configs/example.toml`
- `docs/frontend/FRONTEND_PARAMETER_BOUNDARIES.md`
- `docs/frontend/FRONTEND_ADAPTATION_PLAN.md`
- `docs/frontend/NEAR_DELAY_PROBE_HANDOFF.md`
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
- `reference` 默认选择 `System Audio (Process Tap)`;如果系统声音无法直接作为 reference,
  使用 BlackHole/VB-CABLE MAC 等虚拟路由设备。
- `output` 选择 `devices --json` 枚举到的虚拟音频 output;下游应用再选择同一虚拟设备对应的输入端。
- 当前 Process Tap reference 仅支持全局 pipeline `sample_rate = 48000`。
  LocalVQE 仍可在 macOS 与 `System Audio (Process Tap)` 一起使用:它的 native domain
  是 16 kHz mono,但 `ProcessorChain` 会在 48 kHz runtime 与 16 kHz LocalVQE node
  之间做边界适配。前端不要因为 LocalVQE 的 native 16 kHz 置灰 `reference=system`;
  只有当用户把全局 `sample_rate` 改成非 48000 时,才需要阻止/提示 Process Tap 不支持。

额外安装/授权点:

- BlackHole 是 macOS virtual audio loopback driver,官方支持 installer 和 Homebrew cask。
- BlackHole 安装说明要求关闭音频应用,打开 pkg 安装,需要时重启;Homebrew cask 页面也标注安装后需要 reboot。
- VB-CABLE MAC 是 VB-Audio 的 macOS audio driver,支持 Intel / Apple Silicon,许可证为 Donationware Simple。
- Echoless/Tauri app 需要 macOS 麦克风权限;Process Tap 路径还需要系统音频录制权限。

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
| Output level | 可配置 | `output_level = 0..100`;0 静音,50 原声,100 约 3x 增益;曲线 `gain=(level/50)^log2(3)` |
| Diagnostics recording | 可用 | `--diagnostic-dir` / config diagnostics |
| Runtime status | 可用 | JSONL status events |
| LocalVQE | 可选实验 backend | 需要模型和动态库 |
| RTX AEC | Windows 可选 backend | 需要 `nvafx doctor --json` 通过 |
| Passthrough | 可用 | 诊断路由和设备链路 |
| Offline processing | 可用 | `echoless offline ...` |

设备采样率边界:

- `sample_rate` 是 Echoless 管线采样率,默认仍建议 `48000`。
- 真实 mic/reference/output 设备可以不是 48k/16k;后端会在设备 I/O 边界做固定比率重采样。
- `devices --json` 会返回每个设备的 `default_sample_rate` 与 `supported_sample_rates`;
  后者正常为 `{ min, max, channels, sample_format }[]`,失败时为 `{ error }`。
- `run --status-json` 的 `started` 事件会返回实际打开的设备采样率和 `io_resampling` 标记。
- 当前边界 SRC 是线性固定比率实现,不是 drift 自适应高质量 SRC;长时间漂移/断续仍以 diagnostics 证据判断。

## 默认配置

```toml
mic = "default"
reference = "system"
output = "default"
sample_rate = 48000
frame_ms = 10
reference_channels = "mono"
near_delay_ms = 25 # macOS default; Windows/Linux default is 0
output_level = 50

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
| `near_delay_ms` | macOS `25`,else `0` | near/mic 进入处理器前的人为对齐延迟;运行中可热控 |
| `output_level` | `50` | 全局最终输出电平;范围 `0..100`,0 静音,50 原声,100 约 3x 增益;曲线 `gain=(level/50)^log2(3)`,后端做软限幅保护 |
| `diagnostics.record_dir` | null | diagnostics session 输出目录 |
| `diagnostics.max_seconds` | null | diagnostics 最大录制秒数 |

### AEC3

| 参数 | 默认 | 建议层级 | 含义 |
|---|---:|---|---|
| `ns` | `false` | 常规/高级均可 | AEC3 内置降噪开关;运行中可热控 |
| `ns_level` | `low` | 高级 | `low/moderate/high/veryhigh`;运行中可热控 |
| `agc` | `false` | 高级 | 自动增益;运行中可热控 |
| `initial_delay_ms` | null | 高级 | AEC3 初始延迟 hint;范围 `0..500`;运行中可热控;运行时仍会估计回声对齐 |
| `tail_ms` | null | 高级 | echo tail 长度;修改需要重启 runtime |
| `delay_num_filters` | null | 高级 | 延迟搜索窗大小;修改需要重启 runtime |
| `linear_stable_echo_path` | `false` | 高级 | 稳定线性 echo path 配置;修改需要重启 runtime |

### LocalVQE

| 参数 | 默认 | 建议层级 | 含义 |
|---|---:|---|---|
| `model` | required | 实验 | GGUF 模型路径 |
| `library` | auto | 实验 | LocalVQE 动态库路径 |
| `threads` | auto | 高级 | CPU 线程数 |
| `backend` | auto | 高级 | 上游 backend 字符串 hint |
| `device` | auto | 高级 | 上游 device 数字 index;不填表示 auto |
| `noise_gate` | `false` | 高级 | LocalVQE noise gate;运行中可热控 |
| `noise_gate_threshold_dbfs` | `-45.0` | 高级 | noise gate 阈值;运行中可热控 |

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
echoless doctor audio --json
echoless doctor audio --request-system-audio --json
echoless processors --json
echoless config validate --config config.toml --json
echoless run --config config.toml --status-json
echoless probe-delay --json
echoless nvafx doctor --json
```

集成规则:

- `run --status-json` 的 stdout 是 JSONL status events。
- 人类日志走 stderr。
- `devices --json` 返回输入、输出、reference source;设备包含 `stable_id`,
  `default_sample_rate`, `supported_sample_rates`;reference source 包含 `available` / `hint`。
- `doctor audio --json` 返回虚拟音频设备候选、推荐驱动、安装状态、权限状态和 reference 可用性。
  - `permission_state` = 麦克风 / 输入设备权限状态。
  - `system_audio_permission` = macOS Process Tap / 系统音频录制权限状态:
    `granted | denied | undetermined | unknown`。当前 regular doctor 不触发系统弹窗;mac helper 可发现时返回 `undetermined`,非 mac 或 helper 缺失返回 `unknown`。
  - 用户点击权限按钮时调用 `doctor audio --request-system-audio --json`;该命令会触发一次极短
    Process Tap probe,返回 `system_audio_permission_probe: { requested, ok, state, detail }`。
- `processors --json` 返回 backend 能力、平台约束、参数类型、默认值和 `advanced` 标记。
- `probe-delay --json` 是 macOS/Windows 延迟诊断入口。macOS 使用 Process Tap reference,
  Windows 使用 WASAPI loopback reference。它会播放蜂鸣、临时录制 `mic/ref/out`
  diagnostics,输出 `recommended_near_delay_ms` 等测量结果。默认分析后清理 session;只有传
  `--keep-session` 或 `--out-dir` 时才保留 WAV/CSV。该命令是 `echoless` CLI 原生能力,
  不依赖 Python 或额外脚本;macOS 打包还需包含 Process Tap helper。
- `config validate --json` 返回 `{ ok, errors }`;配置无效时 exit code 为非 0。

## Runtime Status

`run --status-json` 在音频流启动后先输出一条 `started` 事件,随后每隔 `--stats-interval-ms`
输出 `status` 事件。若启用 diagnostics,录制文件 finalize 完成后还会输出
`diagnostics_done`。

```ts
export interface RuntimeStarted {
  type: "started";
  backend: string;
  sample_rate: number;
  frame_ms: number;
  near_delay_ms: number;
  near_delay_samples: number;
  output_level: number;
  output_gain_db?: number | null;
  reference_channels: "mono" | "stereo";
  algorithmic_latency_ms: number;
  reference_source: "none" | "cpal_input" | "cpal_output" | "macos_process_tap";
  mic_device_sample_rate: number;
  output_device_sample_rate: number;
  reference_device_sample_rate?: number | null;
  io_resampling: {
    mic: boolean;
    reference: boolean;
    output: boolean;
  };
  diagnostics_session_dir?: string | null;
}

export interface RuntimeStatus {
  type: "status";
  elapsed_s: number;
  frames: number;
  sample_rate: number;
  frame_ms: number;
  backend: string;
  near_delay_ms: number;
  near_delay_buffered_samples: number;
  output_level: number;
  output_gain_db?: number | null;
  mic_dbfs: number;
  ref_dbfs: number;
  out_dbfs: number;
  mic_wave: number[]; // 64 peak buckets, [0,1]
  ref_wave: number[]; // 64 peak buckets, [0,1]
  out_wave: number[]; // 64 peak buckets, [0,1]
  mic_q_samples: number;
  ref_q_samples: number;
  out_q_samples: number;
  input_queue_latency_ms: number;
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
  recording: boolean;
  diagnostics_frames: number;
  diagnostics_elapsed_s: number;
  diagnostics_drops: number;
}

export interface DiagnosticsDone {
  type: "diagnostics_done";
  session_dir: string;
  frames: number;
  seconds: number;
  reason: "max_seconds" | "stopped" | "run_exit" | "error";
  drops: number;
  ok: boolean;
}
```

延迟字段语义:

```text
estimated_user_latency_ms =
  frame_ms / 2
  + near_delay_ms
  + algorithmic_latency_ms
  + mic_q_samples / sample_rate * 1000
  + out_q_samples / sample_rate * 1000
```

- `estimated_user_latency_ms`: Echoless 软件管线内的用户说话到送入虚拟输出设备估算延迟;不含设备硬件缓冲、通话软件缓冲或网络延迟。首页建议标为 `Pipeline` / `管线延迟`。
- `near_delay_ms`: Echoless 在处理器前主动延后 near/mic 的固定对齐延迟。它不是 AEC3 动态估计值。
- `aec_estimated_delay_ms`: AEC3 估计的回声路径对齐延迟。
- `input_queue_latency_ms`: 麦克风输入队列积压贡献的可见软件管线延迟。
- `output_queue_latency_ms`: 输出队列贡献的可见延迟。

## Active Near Delay Probe

当前主动侦测命令:

```bash
echoless probe-delay --json \
  --mic "MacBook Pro麦克风" \
  --reference system \
  --output "BlackHole 2ch"
```

这是项目原生能力:蜂鸣 WAV 生成、`echoless run` 子进程编排、diagnostics WAV 分析和 JSON 输出都在
`echoless-cli` 内完成。前端不要查找或打包独立 probe 脚本。macOS 用 Process Tap;
Windows 用 `reference=system` WASAPI loopback。
完整调用、字段解释和推荐值使用规则见 `docs/frontend/NEAR_DELAY_PROBE_HANDOFF.md`。

JSON 关键字段:

```ts
export interface NearDelayProbeResult {
  session_dir: string;
  session_retained: boolean;
  ref_dbfs: number;
  mic_dbfs: number;
  global_lag_ms: number;
  global_corr: number;
  event_count: number;
  event_detected: number;
  event_lag_mean_ms: number;
  event_lag_stddev_ms: number;
  event_lag_drift_ms: number;
  recommended_near_delay_ms: number;
  per_beep_lags: Array<{ index: number; time_s: number; lag_ms: number; corr: number }>;
  warnings: string[];
}
```

可由侦测结果主动设置的参数:

- `near_delay_ms`: 主设置项。仅当 `recommended_near_delay_ms > 0` 且结果稳定时写入,
  推荐值为 `0` 时不改配置,只展示诊断信息。手动调整可运行中热控;完整 probe 仍会暂停
  runtime,因为它需要独占设备播放蜂鸣和录制。
- AEC3 `initial_delay_ms`: 只作为高级 hint。可用
  `max(0, event_lag_mean_ms + recommended_near_delay_ms)` 估算后对齐残余,但默认不自动写入;
  AEC3 运行时仍会自己估计。用户手动调整时可运行中热控。

不要由这次 probe 自动设置:

- `delay_num_filters`: 只有长期、多次测量都稳定时才考虑缩小搜索窗;当前不建议前端自动改。
- `tail_ms`: 由房间混响/echo tail 决定,不是 ref/mic 起点延迟决定。
- `frame_ms`, `sample_rate`, `reference_channels`: 不从延迟侦测推导。

LocalVQE / RTX AEC 也只消费顶层 `near_delay_ms`;它们没有当前可暴露的内部 delay hint。

前端自动填入规则:

- `recommended_near_delay_ms > 0`: 表示 reference 晚于 mic,可自动填入顶层 `near_delay_ms`。
- `recommended_near_delay_ms = 0`: 表示方向正常或无需主动补偿,只展示诊断信息,不改配置。

## Diagnostics

实时 diagnostics 可写出:

- `mic.wav`
- `ref.wav`
- `out.wav`
- `stats.csv`
- metadata

写入实现:

- 运行中先写 `*.part`,writer 线程 finalize 后 rename 成正式文件。
- `diagnostics_done` 只在文件已经 flush/finalize/rename 后发出,前端收到后可以直接打开目录。
- `run --status-json` 监听 stdin JSONL 控制命令:
  - `{"cmd":"start_diagnostics","record_dir":"…","max_seconds":10}`
  - `{"cmd":"stop_diagnostics"}`
  - `{"cmd":"set_output_level","level":50}`
  - `{"cmd":"set_near_delay_ms","near_delay_ms":25}`
  - `{"cmd":"set_initial_delay_ms","initial_delay_ms":0}`
  - `{"cmd":"set_aec3_ns","ns":true,"ns_level":"low"}`
  - `{"cmd":"set_aec3_agc","agc":false}`
  - `{"cmd":"set_localvqe_noise_gate","noise_gate":false,"noise_gate_threshold_dbfs":-45}`
  启停录制不需要重启 run。启动成功会发 `diagnostics_started`;停止请求会发
  `diagnostics_stopping`;最终仍以 `diagnostics_done` 为文件可用信号。
  `set_output_level` 只改运行态最终输出电平,范围 `0..100`,成功后发
  `output_level_changed`。前端音量旋钮应防抖发送该命令,避免重启引擎造成掉音。
  AEC3 / LocalVQE 热控命令成功后分别发 `initial_delay_changed`、`aec3_ns_changed`、
  `aec3_agc_changed`、`localvqe_noise_gate_changed`;前端可以忽略这些回执,以本地控件状态和后续 status 为准。

用途:

- Windows/macOS 真机测试。
- 用户反馈回声、断音、音量骤降、延迟时收集证据。
- AEC3 / LocalVQE / RTX AEC 对比。
- 回归分析。

## 前端接入验证点

- 能读取 `echoless devices --json`。
- 能读取 `echoless doctor audio --json`。
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
- restart-required 配置变化后能重启 runtime 并应用新配置;hot-control 参数可运行中生效。
- `echoless run --config configs/example.toml` CLI 仍可人工使用。
