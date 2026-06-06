# Echoless Frontend Adaptation Plan

本文档把 Echoless 从 CLI-first 工具演进到 Tauri/GUI 可控应用的改造计划落成到仓库内。目标是让前端可以稳定地列设备、生成配置、启动/停止实时 AEC、显示诊断指标，同时继续保留现有 CLI 能力。

## 目标

- 保留 CLI 作为一等入口: `devices`、`processors`、`offline`、`run`、`nvafx doctor/install/offline` 继续可用。
- GUI 与 CLI 共用同一套 `PipelineConfig` / `NodeConfig` 配置模型。
- GUI 不解析人类可读日志,而是消费 JSON 配置、JSON 状态事件和结构化诊断结果。
- 首版 GUI 默认以 `sonora_aec3` 保真人声路径为主,LocalVQE 与 RTX AEC 作为独立可选 backend。
- 前端能显示对用户有意义的运行健康指标: 电平、估算用户延迟、AEC 估计回声延迟、drop/underrun、backend runtime error。

## 非目标

- 不在首版把 CLI 替换成 GUI。
- 不在首版要求参数热更新覆盖所有 backend。需要切换设备、采样率、backend 时可以重启 runtime。
- 不默认级联 `sonora_aec3 + localvqe` 或 `sonora_aec3 + nvidia_afx_aec`。
- 不把所有 AEC3 内部 suppressor 参数直接暴露给普通用户。
- 不在 GUI 首版实现原生虚拟麦驱动。MVP 仍使用 VB-Cable / BlackHole / Virtual Desktop Mic 等外部虚拟设备。

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
  - `diagnostics`
  - `chain`
- `DiagnosticsConfig`
  - `record_dir`
  - `max_seconds`
- `NodeConfig`
  - `kind`
  - flattened `params`

`configs/example.toml` 已经是 GUI 配置蓝本。后续 GUI 应生成等价 JSON/TOML,不要维护另一套独立配置语义。

### 处理器

当前可用 `kind`:

- `passthrough`
- `sonora_aec3`
- `localvqe`
- `nvidia_afx_aec`

默认产品路径:

```toml
sample_rate = 48000
frame_ms = 10
reference_channels = "mono"

[[chain]]
kind = "sonora_aec3"
ns = false
agc = false
```

### 已有诊断能力

实时 diagnostics 可写出:

- `mic.wav`
- `ref.wav`
- `out.wav`
- `stats.csv`
- metadata

`stats.csv` 已包含电平、队列长度、drop/underrun、node runtime error 等字段。GUI 首版可以把“一键诊断录制”作为产品能力。

## CLI 保留契约

后续前端适配不得破坏以下能力:

- `echoless devices`
- `echoless processors`
- `echoless offline --mic ... --reference ... --out ...`
- `echoless run --config configs/example.toml`
- `echoless run` 的命令行覆盖能力:
  - `--mic`
  - `--reference`
  - `--output`
  - `--sample-rate`
  - `--frame-ms`
  - `--reference-channels`
  - `--processor`
  - `--ns` / `--no-ns`
  - `--ns-level`
  - `--tail-ms`
  - `--diagnostic-dir`
  - `--diagnostic-seconds`
  - `--verbose`
  - `--stats-interval-ms`
- Windows-only RTX AEC commands:
  - `echoless nvafx doctor`
  - `echoless nvafx doctor --json`
  - `echoless nvafx install`
  - `echoless nvafx offline`

CLI 文本输出可以继续优化,但 GUI 不应依赖文本格式。新增 JSON 输出时,文本输出仍保留给人工调试。

## 目标架构

首版建议使用 Tauri sidecar 模式:

```text
Tauri frontend
  -> Tauri Rust commands
     -> spawn/manage echoless sidecar
        -> JSON commands / JSONL status events
           -> existing echoless runtime
```

中期再把实时 runtime 从 `echoless-cli/src/realtime.rs` 下沉为可复用 runtime,实现 `echoless-core::ControlApi`:

```text
Tauri frontend
  -> Tauri Rust backend
     -> EcholessRuntime implements ControlApi
        -> PipelineConfig
        -> ProcessorChain
        -> platform audio IO
```

选择 sidecar 首版的原因:

- 不破坏现有 CLI 发布形态。
- 音频权限、崩溃、长运行进程更容易隔离。
- Windows/macOS artifact 打包更直接。
- 前端可以先基于稳定 JSON contract 开发。

## 必须补的后端接口

### 1. 设备列表 JSON

新增:

```bash
echoless devices --json
```

目标输出:

```json
{
  "inputs": [
    {
      "id": "1",
      "index": 1,
      "name": "MacBook Pro麦克风",
      "kind": "input",
      "default_sample_rate": 48000,
      "channels": 1,
      "sample_format": "f32"
    }
  ],
  "outputs": [
    {
      "id": "0",
      "index": 0,
      "name": "MacBook Pro扬声器",
      "kind": "output",
      "default_sample_rate": 48000,
      "channels": 2,
      "sample_format": "f32"
    }
  ],
  "reference_sources": [
    { "id": "system", "label": "System audio", "kind": "system" },
    { "id": "none", "label": "No reference", "kind": "none" }
  ]
}
```

### 2. Processor manifest JSON

新增:

```bash
echoless processors --json
```

目标输出:

```json
{
  "processors": [
    {
      "kind": "sonora_aec3",
      "label": "AEC3",
      "platforms": ["windows", "macos", "linux"],
      "default": true,
      "params": {
        "ns": { "type": "bool", "default": false, "ui": "toggle" },
        "ns_level": {
          "type": "select",
          "values": ["low", "moderate", "high", "veryhigh"],
          "default": "low",
          "requires": { "ns": true }
        },
        "agc": { "type": "bool", "default": false, "ui": "toggle" },
        "tail_ms": { "type": "number", "default": null, "advanced": true },
        "delay_num_filters": { "type": "number", "default": null, "advanced": true },
        "linear_stable_echo_path": { "type": "bool", "default": false, "advanced": true }
      }
    }
  ]
}
```

### 3. Runtime status JSONL

新增一种机器可读运行模式:

```bash
echoless run --config config.toml --status-json
```

或者由 daemon/sidecar 统一输出 JSONL events:

```json
{
  "type": "status",
  "elapsed_s": 12,
  "sample_rate": 48000,
  "frame_ms": 10,
  "backend": "sonora_aec3",
  "mic_dbfs": -18.2,
  "ref_dbfs": -31.0,
  "out_dbfs": -20.5,
  "mic_q_samples": 320,
  "ref_q_samples": 640,
  "out_q_samples": 3000,
  "output_queue_latency_ms": 62.5,
  "estimated_user_latency_ms": 67.5,
  "aec_estimated_delay_ms": 48,
  "ref_underruns": 0,
  "output_underruns": 0,
  "output_overruns": 0,
  "input_drops": 0,
  "stale_drops": 0,
  "node_process_time_ms": 0.1,
  "runtime_errors": 0,
  "diverged": false,
  "last_backend_error": null
}
```

估算用户延迟:

```text
estimated_user_latency_ms =
  frame_ms / 2
  + algorithmic_latency_ms
  + out_q_samples / sample_rate * 1000
```

注意:

- `aec_estimated_delay_ms` 是 AEC3 估计的回声对齐延迟,不是用户感知延迟。
- `output_queue_latency_ms` 更接近用户说话进入虚拟麦前的可见队列延迟。

### 4. 配置校验

新增:

```bash
echoless config validate --config config.toml --json
```

首版也可以先在 `run` 前做同等校验。GUI 需要拿到结构化错误:

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

### 5. Runtime 控制

短期:

- Tauri backend spawn sidecar。
- Stop 通过 SIGINT / child process kill with graceful timeout。
- 参数变更后重启 sidecar。

中期:

- 抽出 `RealtimeRuntime`。
- 实现 `ControlApi::start/stop/set_chain/subscribe_stats`。
- 引入明确的 `RuntimeState`: `Idle` / `Starting` / `Running` / `Stopping` / `Error`。

## GUI 参数分层

### 普通模式

- Mic device
- Reference source
- Output device
- Backend
- Reference channels: mono / stereo
- Noise suppression: off / low / moderate / high
- Diagnostics duration: 30s / 45s / custom

### 高级模式

- `sample_rate`
- `frame_ms`
- `tail_ms`
- `delay_num_filters`
- `linear_stable_echo_path`
- `agc`
- LocalVQE:
  - `model`
  - `library`
  - `backend`
  - `device`
  - `threads`
  - `noise_gate`
  - `noise_gate_threshold_dbfs`
- RTX AEC:
  - `runtime_dir`
  - `model_path`
  - `intensity_ratio`
  - `use_default_gpu`
  - `disable_cuda_graph`
  - `on_runtime_error`

### 暂不暴露给普通用户

- AEC3 suppressor / nearend detection 内部细节。
- external delay estimator 模式。
- ring buffer / stale drop 阈值。

这些应先做成预设:

- `voice_fidelity`: 保真人声,默认。
- `echo_removal`: 更强残余回声抑制,可能压人声。
- `diagnostic`: 更多日志和录制。

## 产品默认策略

### AEC3 默认

```toml
sample_rate = 48000
frame_ms = 10
reference_channels = "mono"

[[chain]]
kind = "sonora_aec3"
ns = false
agc = false
```

理由:

- AEC3 内部工作域就是 48k / 10ms。
- Mac 实测 48k 比 44.1k 更稳。
- `ns=false` / `agc=false` 更符合音质保真优先。
- 用户后面可能会级联 NVIDIA Broadcast,本层不应默认重度降噪。

### LocalVQE 默认

LocalVQE 只作为 standalone backend,不默认和 AEC3 级联。

首版 GUI 文案应标为实验:

- 需要模型和动态库。
- 当前 16k mono 推理。
- 可能增加延迟和吃字。

### RTX AEC 默认

RTX AEC 只在 Windows + RTX + doctor 通过时显示。

首版 GUI:

- macOS 上隐藏或禁用。
- Windows 上显示 doctor 状态。
- 不和 AEC3 默认级联。

## 验收标准

### 后端适配完成

- `echoless devices --json` 可被前端直接消费。
- `echoless processors --json` 提供参数 manifest。
- `echoless run --status-json` 或 sidecar JSONL 能持续输出 status event。
- status event 包含:
  - `mic_dbfs`
  - `ref_dbfs`
  - `out_dbfs`
  - `output_queue_latency_ms`
  - `estimated_user_latency_ms`
  - `aec_estimated_delay_ms`
  - `input_drops`
  - `stale_drops`
  - `ref_underruns`
  - `output_underruns`
  - `runtime_errors`
- `echoless run --config configs/example.toml` 继续可用。
- `echoless offline` 继续可用。
- RTX AEC JSON doctor 继续可用。

### 前端首版完成

- 能列设备并保存选择。
- 能启动/停止 AEC3 推荐配置。
- 能切换 mono/stereo reference。
- 能切换 NS off/low/moderate/high。
- 能显示运行状态和估算用户延迟。
- 能触发 diagnostics 录制并显示生成目录。
- Windows RTX backend 只在 doctor 通过时可选。
- LocalVQE 可作为实验 backend 配置,但不默认启用。

## 风险与缓解

### 风险: GUI 解析 CLI 文本导致脆弱

缓解:

- 所有前端消费接口必须使用 JSON。
- 人类文本 stdout 不纳入稳定 contract。

### 风险: GUI 参数太多导致用户误调

缓解:

- 普通模式只放少量安全参数。
- 高级模式用折叠面板。
- 默认配置固定为 AEC3 保真基线。

### 风险: 热更新复杂

缓解:

- 首版参数变更后重启 runtime。
- 后续再按 backend 能力逐步支持热更新。

### 风险: 延迟指标误导用户

缓解:

- 分开显示 `用户估算延迟` 和 `AEC 回声对齐延迟`。
- Tooltip 说明 `aec_estimated_delay_ms` 不是说话输出延迟。

### 风险: macOS/Windows 设备语义不同

缓解:

- 设备接口返回抽象字段。
- 平台特有说明交给前端文案。
- `system` / `none` / `input:<name>` / `output:<name>` 保持跨平台约定。

## 推荐实施顺序

1. 增加 JSON devices 和 processors。
2. 增加 JSONL runtime status。
3. 把 `estimated_delay_ms`、`output_queue_latency_ms`、`estimated_user_latency_ms` 纳入 status。
4. 加 config validate。
5. Tauri sidecar 读取 JSON 接口,实现主控页和设备页。
6. 接 diagnostics 录制。
7. 接 backend 调参页。
8. 再抽 `RealtimeRuntime` / `ControlApi` 实现,减少 CLI 和 GUI runtime 重复。

