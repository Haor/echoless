# Frontend Agent Handoff

本文档给负责 Echoless GUI/Tauri 的前端 agent 使用。后端/整体改造计划见 `docs/frontend/FRONTEND_ADAPTATION_PLAN.md`。前端实现时必须保留现有 CLI 能力,不要把 GUI 做成唯一入口。

## 必读文件

- `README.md`
- `configs/example.toml`
- `docs/frontend/FRONTEND_ADAPTATION_PLAN.md`
- `docs/localvqe_inference.md`
- `docs/research/rtx_aec_runtime_distribution.md`
- `docs/research/sonora_aec3_internal_map.md`

## 当前产品判断

首版 GUI 默认 backend 是 `sonora_aec3`。推荐默认配置:

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

理由:

- AEC3 内部处理域是 48k / 10ms。
- Mac 本机试听中,48k 比 44.1k 更稳。
- 音质保真优先,默认不开 NS / AGC。
- LocalVQE 和 RTX AEC 都是 standalone 可选 backend,不是默认后级。

## 前端首版范围

### 必须做

- 设备选择:
  - mic
  - reference
  - output
- Backend 选择:
  - AEC3
  - LocalVQE experimental
  - RTX AEC Windows only,doctor 通过才启用
  - Passthrough diagnostic
- AEC3 基础参数:
  - reference channels: mono / stereo
  - NS: off / low / moderate / high / veryhigh
  - AGC: advanced only,默认 off
- 启动/停止实时处理。
- 显示实时电平:
  - mic dBFS
  - reference dBFS
  - output dBFS
- 显示状态:
  - estimated user latency
  - AEC estimated delay
  - input drops
  - stale drops
  - ref underruns
  - output underruns
  - runtime errors
- 诊断录制:
  - 30s
  - 45s
  - custom seconds
  - 显示 diagnostics session 路径
- 保存/加载配置。

### 不要在首版做

- 不要默认启用 `sonora_aec3 + localvqe` 级联。
- 不要默认启用 RTX AEC。
- 不要在普通界面展示所有 AEC3 内部 suppressor 参数。
- 不要用 stdout 人类文本作为长期数据源。
- 不要把 CLI 命令删除或重命名。

## UI 信息架构

### 1. Main

目标: 用户打开后直接能看到当前链路是否工作。

内容:

- Start / Stop
- Backend selector
- Mic level meter
- Reference level meter
- Output level meter
- Status badge:
  - Ready
  - Running
  - No reference
  - High latency
  - Dropping audio
  - Runtime error
- Estimated user latency
- Diagnostics quick action

### 2. Devices

内容:

- Mic device dropdown
- Reference source dropdown:
  - System audio
  - None
  - Input devices
  - Output devices
- Output device dropdown
- Sample rate display,默认 48000
- Frame size display,默认 10ms

说明:

- macOS 用户可能需要 BlackHole / Virtual Desktop Mic / 其他虚拟设备。
- Windows 用户常见输出是 VB-Cable Input。

### 3. Processing

内容:

- Backend cards:
  - AEC3 Recommended
  - LocalVQE Experimental
  - RTX AEC Windows RTX
  - Passthrough Diagnostic
- AEC3 controls:
  - reference mono/stereo
  - noise suppression selector
  - AGC toggle in advanced
  - tail_ms in advanced
- LocalVQE controls:
  - model path
  - library path
  - threads
  - noise gate
- RTX controls:
  - doctor status
  - runtime dir
  - model path
  - intensity ratio
  - on runtime error

### 4. Diagnostics

内容:

- Record diagnostics button
- Duration selector
- Latest session list
- Stats summary:
  - max output queue latency
  - input drops
  - stale drops
  - output underruns
  - runtime errors
- Open session directory

## Target Data Types

这些类型是前端应围绕的目标 contract。字段可随后端实现微调,但语义不要变。

```ts
export type ReferenceChannels = "mono" | "stereo";

export type ProcessorKind =
  | "passthrough"
  | "sonora_aec3"
  | "localvqe"
  | "nvidia_afx_aec";

export interface PipelineConfig {
  mic: string;
  reference: string;
  output: string;
  sample_rate: number;
  frame_ms: number;
  reference_channels: ReferenceChannels;
  diagnostics?: DiagnosticsConfig;
  chain: ChainNode[];
}

export interface DiagnosticsConfig {
  record_dir?: string;
  max_seconds?: number;
}

export interface ChainNode {
  kind: ProcessorKind;
  [param: string]: unknown;
}
```

```ts
export interface RuntimeStatus {
  type: "status";
  elapsed_s: number;
  sample_rate: number;
  frame_ms: number;
  backend: ProcessorKind;
  mic_dbfs: number;
  ref_dbfs: number;
  out_dbfs: number;
  mic_q_samples: number;
  ref_q_samples: number;
  out_q_samples: number;
  output_queue_latency_ms: number;
  estimated_user_latency_ms: number;
  aec_estimated_delay_ms: number;
  ref_underruns: number;
  output_underruns: number;
  output_overruns: number;
  input_drops: number;
  stale_drops: number;
  node_process_time_ms: number;
  runtime_errors: number;
  diverged: boolean;
  last_backend_error?: string | null;
}
```

延迟计算:

```ts
export function estimateUserLatencyMs(status: {
  frame_ms: number;
  sample_rate: number;
  out_q_samples: number;
  algorithmic_latency_ms?: number;
}) {
  return (
    status.frame_ms / 2 +
    (status.algorithmic_latency_ms ?? 0) +
    (status.out_q_samples / status.sample_rate) * 1000
  );
}
```

注意:

- `aec_estimated_delay_ms` 是算法估计的回声对齐延迟。
- `estimated_user_latency_ms` 是用户说话到虚拟麦输出的估算延迟。
- 两者不能混用。

## 当前 CLI 可用命令

这些命令现在就可用于人工调试:

```bash
echoless devices
echoless processors
echoless run --config configs/example.toml
echoless run --config configs/example.toml --verbose --stats-interval-ms 1000
echoless run --config configs/example.toml --diagnostic-dir diagnostics/aec3 --diagnostic-seconds 45 --verbose
echoless offline --mic m.wav --reference r.wav --out o.wav --chain sonora_aec3
echoless nvafx doctor
echoless nvafx doctor --json
```

这些 JSON 命令是前端适配目标,可能需要后端 agent 先实现:

```bash
echoless devices --json
echoless processors --json
echoless run --config config.toml --status-json
echoless config validate --config config.toml --json
```

前端首版可以先用 mock adapter 对齐这些 contract。不要把现有中文 stdout 文本当作稳定 API。

## Backend Visibility Rules

### AEC3

显示条件:

- 所有平台默认显示。

默认:

- sample rate: 48000
- frame: 10ms
- reference: mono
- NS: off
- AGC: off

### LocalVQE

显示条件:

- artifact 内存在模型和动态库,或用户手动选择路径。

标记:

- Experimental

说明:

- 16k mono 推理。
- `algorithmic_latency_ms` 约 16ms。
- 不默认级联 AEC3。

### RTX AEC

显示条件:

- Windows。
- `nvafx doctor` 通过。

禁用条件:

- macOS。
- 没有 RTX runtime。
- doctor 报 runtime / GPU / driver 不可用。

说明:

- 只支持 48000 Hz。
- 只支持 10ms frame。
- 只支持 mono reference。
- 作为独立 backend 测试,不默认和 AEC3 级联。

### Passthrough

显示条件:

- 高级/诊断模式。

用途:

- 排查虚拟麦、设备链路、延迟和 drop 是否来自 AEC backend。

## Frontend State Machine

建议状态:

```ts
export type RuntimeState =
  | "idle"
  | "starting"
  | "running"
  | "stopping"
  | "error";
```

行为:

- `idle -> starting`: spawn sidecar / call start。
- `starting -> running`: 收到第一条 status event。
- `running -> stopping`: 用户 stop。
- `stopping -> idle`: sidecar 退出。
- `any -> error`: 启动失败、设备丢失、runtime error 不可恢复。

设备或 backend 变更:

- 首版直接提示需要重启 runtime。
- 停止后应用新配置。

## UX Copy Guidance

普通用户文案要避免把内部概念混在一起:

- "Estimated app latency" 用于 `estimated_user_latency_ms`。
- "AEC alignment delay" 用于 `aec_estimated_delay_ms`。
- "Audio drop detected" 用于 `input_drops/stale_drops/output_underruns > 0`。
- "Reference is silent" 用于 ref 电平长期接近 -120dB。

不要把 `10ms frame` 写成总延迟。它只是处理粒度。

## Test Checklist

前端完成后至少覆盖:

- macOS:
  - AEC3 48k / mono / NS off 可启动。
  - 设备列表可刷新。
  - runtime status 能持续更新。
  - diagnostics 生成 session。
  - RTX AEC 禁用。
- Windows:
  - AEC3 可启动。
  - VB-Cable / CABLE Input 可选。
  - `nvafx doctor` 通过时 RTX AEC 可选。
  - `nvafx doctor` 失败时 RTX AEC 显示原因。
- Cross-platform:
  - 保存配置后重新打开仍能加载。
  - Stop 后没有残留 running 状态。
  - backend 切换不会生成无效配置。
  - `echoless run --config configs/example.toml` CLI 仍可用。

## Do Not Change Without Backend Coordination

- `PipelineConfig` 字段语义。
- `NodeConfig.kind` 名称。
- `reference` 字符串约定: `system` / `none` / `input:<name>` / `output:<name>`。
- `reference_channels` 值: `mono` / `stereo`。
- `nvidia_afx_aec` 约束: Windows + 48k + 10ms + mono reference。
- CLI 命令名和现有 flags。

