# Frontend Integration Contract Plan

本文档记录 Echoless 后端为了接入 Tauri/GUI 需要提供的稳定能力。前端负责产品设计和 UI/UX;后端负责配置语义、JSON contract、运行状态、诊断证据和外部依赖边界。

## 后端目标

- 保留 CLI 作为一等入口。
- GUI 与 CLI 共用 `PipelineConfig` / `NodeConfig`。
- 前端消费 JSON/JSONL,不解析人类可读日志。
- 默认 backend 使用 `aec3`。
- LocalVQE 与 RTX AEC 作为独立可选 backend。
- 提供运行健康指标:电平、估算用户延迟、AEC 回声对齐延迟、drop/underrun、backend runtime error。
- 提供虚拟音频设备安装状态的可检测边界,让前端可以做安装引导或 installer 集成。

## 当前基础

### 配置模型

核心配置在 `echoless-core`:

- `PipelineConfig`
  - `mic`
  - `reference`
  - `output`
  - `sample_rate`
  - `frame_ms`
  - `reference_channels`
  - `near_delay_ms`
  - `output_level`
  - `diagnostics`
  - `chain`
- `DiagnosticsConfig`
  - `record_dir`
  - `max_seconds`
- `NodeConfig`
  - `kind`
  - flattened `params`

`configs/example.toml` 是配置语义蓝本。GUI 可以生成等价 JSON/TOML,但配置语义以 core 类型为准。

### 处理器

当前可用 `kind`:

- `passthrough`
- `aec3`
- `localvqe`
- `nvidia_afx_aec`

默认配置:

```toml
sample_rate = 48000
frame_ms = 10
reference_channels = "mono"
near_delay_ms = 25
output_level = 50

[[chain]]
kind = "aec3"
ns = false
agc = false
```

### Diagnostics

实时 diagnostics 可写出:

- `mic.wav`
- `ref.wav`
- `out.wav`
- `stats.csv`
- metadata

该能力用于生成可交接证据,尤其适合处理用户反馈里的回声、断音、音量骤降和延迟问题。

## Sidecar 集成形态

首版建议使用 Tauri sidecar 管理 `echoless` 进程:

```text
Tauri app
  -> Rust command / process manager
     -> spawn echoless sidecar
        -> JSON/JSONL contract
```

后端需要保证:

- `echoless devices --json`
- `echoless processors --json`
- `echoless config validate --config <file> --json`
- `echoless run --config <file> --status-json`
- `echoless nvafx doctor --json`

中期可以把 realtime runtime 从 CLI 下沉为可复用 runtime:

```text
Tauri Rust backend
  -> EcholessRuntime implements ControlApi
     -> PipelineConfig
     -> ProcessorChain
     -> platform audio IO
```

## JSON Contract

### 1. Devices

已实现:

```bash
echoless devices --json
```

目标语义:

```json
{
  "ok": true,
  "inputs": [
    {
      "id": "1",
      "stable_id": "AppleHDAEngineInput:1B,0,1,0:1",
      "index": 1,
      "name": "MacBook Pro麦克风",
      "kind": "input",
      "is_default": true,
      "selector": "1",
      "default_sample_rate": 48000,
      "supported_sample_rates": [
        { "min": 48000, "max": 48000, "channels": 1, "sample_format": "f32" }
      ],
      "channels": 1,
      "sample_format": "f32",
      "config_error": null
    }
  ],
  "outputs": [
    {
      "id": "0",
      "stable_id": "AppleHDAEngineOutput:1B,0,1,0:0",
      "index": 0,
      "name": "MacBook Pro扬声器",
      "kind": "output",
      "is_default": true,
      "selector": "0",
      "default_sample_rate": 48000,
      "supported_sample_rates": [
        { "min": 48000, "max": 48000, "channels": 2, "sample_format": "f32" }
      ],
      "channels": 2,
      "sample_format": "f32",
      "config_error": null
    }
  ],
  "reference_sources": [
    { "id": "system", "stable_id": "system", "label": "System audio", "kind": "system", "available": false, "hint": "macOS needs routed reference audio" },
    { "id": "none", "stable_id": "none", "label": "No reference", "kind": "none", "available": true },
    { "id": "input:1", "stable_id": "input:AppleHDAEngineInput:1B,0,1,0:1", "label": "MacBook Pro麦克风", "kind": "input", "device_index": 1, "available": true },
    { "id": "output:0", "stable_id": "output:AppleHDAEngineOutput:1B,0,1,0:0", "label": "MacBook Pro扬声器", "kind": "output", "device_index": 0, "available": true }
  ]
}
```

注意:

- `selector` 仍兼容设备索引,但前端应优先保存 `stable_id` 和用户可识别名称。
- `supported_sample_rates` 正常为范围数组,每项含 `min`、`max`、`channels`、`sample_format`;
  枚举失败时为 `{ "error": "..." }`。
- 设备不原生支持管线采样率时,后端会在设备 I/O 边界重采样;前端不应因此禁止运行。
- 当前 `stable_id` 优先来自 CPAL `DeviceId` 或设备地址,否则由设备名派生;不是最终 WASAPI endpoint id / CoreAudio UID 原生实现。
- 某些 macOS 会话可能返回空设备数组,需要支持刷新。

### 1.1 Audio Doctor

已实现:

```bash
echoless doctor audio --json
```

目标语义:

```json
{
  "ok": true,
  "platform": "macos",
  "virtual_output_detected": true,
  "candidate_outputs": [{ "name": "BlackHole 2ch", "selector": "1", "stable_id": "..." }],
  "candidate_inputs": [{ "name": "BlackHole 2ch", "selector": "4", "stable_id": "..." }],
  "recommended_driver": "blackhole-2ch",
  "install_status": "installed",
  "needs_reboot": false,
  "permission_state": "granted",
  "system_audio_permission": "undetermined",
  "reference_sources": []
}
```

`install_status` 取值为 `installed` / `missing` / `unknown`。当前 macOS `permission_state` 只能基于 input 枚举做轻量估计。
`system_audio_permission` 是 Process Tap / 系统音频录制权限态;regular doctor 不主动触发系统弹窗,
因此 mac helper 可发现时返回 `undetermined`,helper 缺失或非 macOS 返回 `unknown`。

### 2. Processor Manifest

已实现:

```bash
echoless processors --json
```

目标语义:

- `kind`: 后端稳定标识。
- `label`: 人类可读名称。
- `platforms`: 支持平台。
- `default`: 后端默认推荐标记。
- `experimental`: 实验能力标记。
- `constraints`: 采样率、frame、声道等约束。
- `params`: 参数类型、默认值、可选值、范围、required、advanced。

示例:

```json
{
  "kind": "aec3",
  "label": "AEC3",
  "platforms": ["windows", "macos", "linux"],
  "default": true,
  "experimental": false,
  "constraints": {
    "preferred_sample_rate": 48000,
    "preferred_frame_ms": 10
  },
  "params": {
    "reference_channels": {
      "type": "select",
      "values": ["mono", "stereo"],
      "default": "mono"
    },
    "ns": { "type": "bool", "default": false },
    "ns_level": {
      "type": "select",
      "values": ["low", "moderate", "high", "veryhigh"],
      "default": "low",
      "requires": { "ns": true }
    },
    "agc": { "type": "bool", "default": false, "advanced": true },
    "initial_delay_ms": { "type": "number", "default": null, "advanced": true },
    "tail_ms": { "type": "number", "default": null, "min": 4, "advanced": true }
  }
}
```

Manifest 只表达后端能力,不规定控件形态。

### 3. Runtime Status

已实现:

```bash
echoless run --config config.toml --status-json
```

stdout 为 JSONL events,stderr 为人类日志。音频流启动后先输出 `started`,默认 1000ms 一条
`status`,可用 `--stats-interval-ms` 覆盖。启用 diagnostics 时,录制文件 finalize 完成后还会输出
`diagnostics_done`。

核心字段:

```json
{
  "type": "started",
  "backend": "aec3",
  "sample_rate": 48000,
  "frame_ms": 10,
  "near_delay_ms": 25,
  "near_delay_samples": 1200,
  "output_level": 50,
  "output_gain_db": 0.0,
  "reference_source": "macos_process_tap",
  "diagnostics_session_dir": "diagnostics/session-1765000000"
}
```

```json
{
  "type": "status",
  "elapsed_s": 12,
  "frames": 576000,
  "sample_rate": 48000,
  "frame_ms": 10,
  "backend": "aec3",
  "near_delay_ms": 25,
  "near_delay_buffered_samples": 1200,
  "output_level": 50,
  "output_gain_db": 0.0,
  "mic_dbfs": -18.2,
  "ref_dbfs": -31.0,
  "out_dbfs": -20.5,
  "mic_wave": [0.0, 0.4, 0.2],
  "ref_wave": [0.0, 0.1, 0.1],
  "out_wave": [0.0, 0.3, 0.2],
  "input_queue_latency_ms": 0.0,
  "output_queue_latency_ms": 62.5,
  "algorithmic_latency_ms": 0.0,
  "estimated_user_latency_ms": 92.5,
  "aec_estimated_delay_ms": 48,
  "input_drops": 0,
  "ref_underruns": 0,
  "output_underruns": 0,
  "runtime_errors": 0,
  "diverged": false,
  "diagnostics_session_dir": "diagnostics/session-1765000000",
  "recording": true,
  "diagnostics_frames": 48000,
  "diagnostics_elapsed_s": 1.0,
  "diagnostics_drops": 0
}
```

```json
{
  "type": "diagnostics_done",
  "session_dir": "diagnostics/session-1765000000",
  "frames": 480000,
  "seconds": 10.0,
  "reason": "max_seconds",
  "drops": 0,
  "ok": true
}
```

延迟估算:

```text
estimated_user_latency_ms =
  frame_ms / 2
  + near_delay_ms
  + algorithmic_latency_ms
  + mic_q_samples / sample_rate * 1000
  + out_q_samples / sample_rate * 1000
```

字段语义:

- `estimated_user_latency_ms`: Echoless 软件管线内的用户说话到进入虚拟输出设备前估算延迟;不含设备硬件缓冲、通话软件缓冲或网络延迟。首页建议标为 `Pipeline` / `管线延迟`。
- `near_delay_ms`: Echoless 在处理器前主动延后 near/mic 的固定对齐延迟。
- `aec_estimated_delay_ms`: AEC3 对回声路径的动态对齐估计。
- `input_queue_latency_ms`: 麦克风输入队列积压贡献的延迟。
- `output_queue_latency_ms`: 输出队列贡献的延迟。

### 4. Config Validate

已实现:

```bash
echoless config validate --config config.toml --json
```

目标语义:

```json
{
  "ok": false,
  "errors": [
    {
      "path": "chain[0].tail_ms",
      "message": "tail_ms must be >= 4"
    }
  ]
}
```

配置无效时 stdout 仍输出 JSON,进程用非 0 exit code 表达失败。

## 外部虚拟音频设备集成

### Windows / VB-CABLE

调研结论:

- 基础 VB-CABLE 可以作为随应用分发/嵌入安装包的候选,官方 licensing 对 donationware 可见性和来源标注有要求。
- 驱动安装需要管理员权限,安装/卸载后通常需要重启。
- 面向专业/组织分发需要处理官方 licensing。

后端可提供的能力:

- 在 `devices --json` 中保留设备名称、kind、selector。
- 后续增加 `doctor audio --json`:
  - `virtual_output_detected`
  - `candidate_outputs`
  - `candidate_inputs`
  - `recommended_driver`
  - `install_status`
  - `needs_reboot`
- 后续 installer 可以调用供应商安装包,但要采用显式用户授权流程。

首版前端可依赖的最小能力:

- 通过设备名识别 `CABLE Input` / `VB-Audio Virtual Cable`。
- 未识别到时展示“需要安装虚拟音频设备”的产品状态。
- 安装后重新调用 `devices --json` 验证。

### macOS / BlackHole 或 VB-CABLE MAC

调研结论:

- BlackHole 是开源 macOS virtual audio loopback driver,支持 installer 和 Homebrew cask。
- BlackHole 安装后可能需要重启。
- VB-CABLE MAC 是 VB-Audio 的 macOS audio driver,支持 Intel / Apple Silicon。
- Echoless 需要麦克风权限;虚拟音频设备由系统音频设置和第三方 driver 提供。

后端可提供的能力:

- 在 `devices --json` 中暴露当前系统实际枚举到的 BlackHole / VB-CABLE MAC / Virtual Desktop Mic 等设备。
- 后续 `doctor audio --json` 可以给出平台建议:
  - `recommended_driver: "blackhole-2ch" | "vb-cable-mac"`
  - `install_methods: ["official_installer", "homebrew"]`
  - `permission_state`
  - `needs_reboot`

首版前端可依赖的最小能力:

- 通过设备名识别 `BlackHole`、`VB-CABLE`、`Virtual Desktop` 等候选。
- 用户安装或授权后刷新设备列表。
- 输出设备和下游 app input 选择同一个虚拟设备。

## 参数暴露策略

完整功能和参数边界见 `docs/frontend/FRONTEND_PARAMETER_BOUNDARIES.md`。后端只提供参数元数据、默认值、平台约束和校验结果;前端可以自行决定具体 UI,但必须遵守以下 contract。

### 硬性 contract

- backend kind 只能来自 `processors --json`;当前用户相关 kind 是 `aec3`、`localvqe`、`nvidia_afx_aec`。`rtx_aec` 不是有效 kind。
- AEC3 `ns_level` 的提交值只能是 `low`、`moderate`、`high`、`veryhigh`;UI 可以显示 "very high",但不能提交 `very`。
- RTX AEC v1 只支持 Windows + `sample_rate = 48000` + `frame_ms = 10` + `reference_channels = "mono"`。
- LocalVQE 的 `device` 是数字 device index;`auto/cpu/gpu` 不能写入 `device`。
- 设备下拉只能来自 `devices --json` 当前枚举结果;不要硬编码 Virtual Desktop Mic、BlackHole 或 VB-CABLE 为一定可用。
- macOS 系统声 reference 不能假设存在原生 loopback;通常需要 BlackHole/VB-CABLE MAC 等外部路由。

### 建议作为常规能力

- `mic`
- `reference`
- `output`
- `reference_channels`
- `output_level`
- `diagnostics.record_dir`
- `diagnostics.max_seconds`
- `ns`
- `ns_level`

### 建议作为高级设置

- `sample_rate`
- `frame_ms`
- `near_delay_ms`
- `agc`
- `initial_delay_ms`
- `tail_ms`
- `delay_num_filters`
- `linear_stable_echo_path`
- LocalVQE: `model`、`library`、`threads`、`backend`、`device`、`noise_gate`、`noise_gate_threshold_dbfs`
- RTX AEC: `runtime_dir`、`model_path`、`intensity_ratio`、`use_default_gpu`、`disable_cuda_graph`、`on_runtime_error`

首版默认推荐保持 `48000 Hz / 10ms / mono reference / aec3 / ns=false / agc=false`。高级项可以暴露给愿意调参的用户,但每次应用前必须走 `config validate --json`。

### 后端内部项

这些由后端维护,不进入稳定用户配置 contract:

- ring buffer 阈值。
- stale drop 阈值。
- AEC3 suppressor 内部结构。
- nearend detection 内部结构。
- diagnostics 文件内部轮转策略。

## 产品自更新预留

产品自更新和运行时参数变更是两件事:

- 运行时参数变更:设备、backend、模型和多数 AEC 结构参数变化时重启 runtime;`output_level`、
  `near_delay_ms`、AEC3 `initial_delay_ms`、AEC3 `ns/ns_level/agc`、LocalVQE
  `noise_gate/noise_gate_threshold_dbfs` 走 runtime hot control。
- 产品自更新:GUI app、`echoless` sidecar、模型/runtime assets、配置 schema 升级。

前端可预留的后端抽象:

```text
UpdateService
  -> getStatus()
  -> check()
  -> download()
  -> applyAndRestart()
  -> setChannel()
```

策略:

- GUI 若走 Tauri,优先评估 Tauri updater plugin。
- 若需要 installer、delta update、release channel 和对象存储部署,再评估 Velopack。
- app update 前停止 `echoless` sidecar 并保存配置。
- release/update workflow 由 tag 或手动 release 触发。
- RTX AEC runtime/model 的再分发许可单独确认。

详细调研见 `docs/productization/update_strategy.md`。

## 后端完成度

| 模块 | 完成度 | 前端可接入程度 | 说明 |
|---|---:|---|---|
| CLI text commands | 90% | 可用 | `devices/processors/run/offline/nvafx` 保留 |
| JSON devices | 85% | 可接 | schema 可用;含 `stable_id` 与 reference availability;设备为空态需处理 |
| JSON processor manifest | 85% | 可接 | 参数 manifest 可用;已去掉 UI 指令字段 |
| Config validate JSON | 80% | 可接 | 结构校验可用;常见字段错误有 path |
| Realtime AEC3 | 80% | 可接 | 主路径可用;真实设备仍要继续调参 |
| Runtime JSONL status | 85% | 可接 | started event、电平、波形、延迟、drop、runtime error 已输出 |
| Diagnostics recording | 85% | 可接 | WAV/stats/metadata 可用 |
| LocalVQE backend | 60% | 实验可接 | 真实推理已接入,音质仍需测试 |
| RTX AEC backend | 70% | Windows 可接 | doctor/install/offline/realtime 已有 |
| External audio device doctor | 70% | 可接 | `doctor audio --json` 可用;权限状态仍是轻量估计 |
| Config save/load | 60% | 前端自行实现 | 后端有 schema 和 validate |
| Process lifecycle API | 60% | sidecar 可实现 | 首版 spawn/stop;后续抽 runtime |
| Product auto update | 10% | 预留接口 | 见 update strategy |

## 推荐实施顺序

1. 固化 `devices --json` 和 `processors --json` schema。
2. 固化 `run --status-json` status event。
3. 固化 `config validate --json` 错误结构。
4. 前端 sidecar adapter 接入 JSON 接口。
5. 接入 diagnostics 录制。
6. 接入 backend 参数配置。
7. 增加 `doctor audio --json` 以支持虚拟音频设备检测和安装状态。
8. 抽 `RealtimeRuntime` / `ControlApi` 减少 CLI 与 GUI runtime 重复。

## 验收标准

- `echoless devices --json` 可被前端直接消费。
- `echoless processors --json` 提供 backend/参数 manifest。
- `echoless doctor audio --json` 可被前端直接消费。
- `echoless run --status-json` 先输出 started event,随后持续输出 status event。
- status event 包含电平、波形、延迟、drop/underrun、runtime error。
- `echoless config validate --config <file> --json` 可用。
- `echoless run --config configs/example.toml` 继续可用。
- `echoless offline` 继续可用。
- RTX AEC JSON doctor 继续可用。
- 前端能识别 Windows/macOS 虚拟音频设备安装状态。
- restart-required 配置变化后重启 runtime 能应用新配置;hot-control 参数可运行中生效。
