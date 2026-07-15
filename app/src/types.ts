// Echoless 后端 JSON 契约的 TS 镜像。
// 形状以 `echoless <cmd> --json` 实测为准(见 docs/CLI.md)。

export type Platform = "windows" | "macos" | "linux";

// ---- devices --json ----
export interface SupportedSampleRateRange {
  min: number;
  max: number;
  channels: number;
  sample_format: string;
}

export interface AudioDevice {
  id: string;
  index: number;
  name: string;
  kind: "input" | "output";
  is_default: boolean;
  selector: string; // 设备索引字符串(跨重启不稳)
  stable_id: string; // 跨重启稳定 id(CoreAudio/WASAPI 派生);mic/output 配置优先用它
  // GUI fast device enumeration may intentionally skip driver format probing.
  default_sample_rate: number | null;
  supported_sample_rates?: SupportedSampleRateRange[] | { error: string };
  channels: number | null;
  sample_format: string | null;
  config_error: string | null;
}

export interface ReferenceSource {
  id: string; // "system" | "none" | "input:N" | "output:N"
  label: string;
  kind: "system" | "none" | "input" | "output";
  device_index?: number;
  available: boolean; // mac system 通常 false(无原生 loopback)
  hint?: string;
  stable_id: string;
  selector?: string; // 如 "input:coreaudio:..." — 直接可作 reference 配置值
}

export interface DeviceList {
  ok: boolean;
  inputs: AudioDevice[];
  outputs: AudioDevice[];
  reference_sources: ReferenceSource[];
}

// ---- processors --json ----
export interface ParamSpec {
  type: "bool" | "number" | "select" | "string" | "path";
  default?: unknown;
  values?: string[];
  min?: number;
  required?: boolean;
  advanced?: boolean;
  requires?: Record<string, unknown>;
}

export type ProcessorKind =
  | "passthrough"
  | "aec3"
  | "localvqe"
  | "nvidia_afx_aec"
  | "webrtc_ns"
  | "rnnoise";

export type NoiseMode = "webrtc" | "rnnoise" | "off";

export interface NoiseModeManifestEntry {
  id: NoiseMode;
  processor_kind: "webrtc_ns" | "rnnoise" | null;
}

export interface LocalvqeNoiseCapability {
  file: string;
  version: string;
  capability: "built_in_ns" | "pure_aec" | "unknown";
  allowed_modes: NoiseMode[];
}

export interface NoiseSuppressionManifest {
  modes: NoiseModeManifestEntry[];
  engine_defaults: Record<string, NoiseMode[]>;
  localvqe_models: LocalvqeNoiseCapability[];
  unknown_localvqe_allowed_modes: NoiseMode[];
}

export interface Processor {
  kind: ProcessorKind;
  label: string;
  platforms: Platform[];
  default: boolean;
  experimental: boolean;
  diagnostic?: boolean;
  requires_doctor_ok?: boolean;
  role?: "noise_suppression";
  constraints?: Record<string, unknown>;
  params: Record<string, ParamSpec>;
}

export interface ProcessorManifest {
  noise_suppression: NoiseSuppressionManifest;
  processors: Processor[];
}

// ---- config validate --json ----
export interface ValidateError {
  path: string;
  message: string;
}
export interface ValidateResult {
  ok: boolean;
  errors: ValidateError[];
}

// ---- run --status-json (JSONL) ----
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
  input_queue_latency_ms: number;
  output_queue_latency_ms: number;
  algorithmic_latency_ms: number;
  estimated_user_latency_ms: number;
  aec_estimated_delay_ms: number;
  mic_input_drops: number;
  ref_input_drops: number;
  input_drops: number;
  mic_stale_drops?: number;
  ref_stale_drops?: number;
  ref_underruns: number;
  output_underruns: number;
  output_overruns: number;
  stale_drops: number;
  // 时钟漂移(2026-07 起 status 常驻):输出/参考时钟相对麦克风的偏差百分比。
  // 旧 CLI 无此字段,须按可选处理。
  output_skew_pct?: number;
  ref_skew_pct?: number;
  clock_skew_warning?: boolean;
  clock_skew_ref_correlated?: boolean;
  clock_skew_direction?: ClockSkewDirection | null;
  node_process_time_ms: number;
  runtime_errors: number;
  diverged: boolean;
  last_backend_error?: string | null;
  diagnostics_session_dir?: string | null;

  // P8-D1:穿透中(OFF = mic 直通虚拟麦,AEC 保温)。status 常驻,默认 false。
  bypassed?: boolean;

  // 诊断录制实时态(后端 2026-06 起):录制中为 true + 已录帧/秒 + writer 丢帧。
  recording?: boolean;
  diagnostics_frames?: number;
  diagnostics_elapsed_s?: number;
  diagnostics_drops?: number;

  // 真实示波波形:每路 64 点,当前 stats interval 内的 peak 包络,范围 [0,1]。
  mic_wave?: number[];
  ref_wave?: number[];
  out_wave?: number[];
}

// 诊断录制收尾事件(writer 线程 finalize 后发;手动停为 "stopped")。
export interface DiagnosticsDoneEvent {
  type: "diagnostics_done";
  session_dir: string;
  frames: number;
  seconds: number;
  reason: "max_seconds" | "stopped" | "run_exit" | "error" | string;
  drops: number;
  ok: boolean;
}

// stdin 就地控制录制后的回执事件。
export interface DiagnosticsStartedEvent {
  type: "diagnostics_started";
  session_dir: string;
  max_seconds: number | null;
  recording: boolean;
}
export interface DiagnosticsStoppingEvent {
  type: "diagnostics_stopping";
  session_dir: string;
}
export interface ControlErrorEvent {
  type: "control_error";
  cmd: string | null;
  message: string;
}
export interface StreamErrorEvent {
  type: "stream_error";
  stream: string;
  message: string;
  fatal: boolean;
  recoverable?: boolean;
}
export type ClockSkewDirection =
  | "output_faster_than_capture"
  | "capture_faster_than_output";
interface ClockSkewEventFields {
  output_skew_pct: number;
  ref_skew_pct: number;
  ref_correlated: boolean;
  direction: ClockSkewDirection | null;
  hint: string;
}
export interface ClockSkewWarningEvent extends ClockSkewEventFields {
  type: "clock_skew_warning";
}
export interface ClockSkewResolvedEvent extends ClockSkewEventFields {
  type: "clock_skew_resolved";
}
export interface RuntimeErrorEvent {
  type: "error";
  message: string;
}
// set_output_level 实时生效后的回执(值由前端驱动,UI 仅忽略)。
export interface OutputLevelChangedEvent {
  type: "output_level_changed";
  output_level: number;
  output_gain_db: number | null;
}
// set_near_delay_ms 实时生效后的回执(值由前端驱动,UI 仅忽略)。
export interface NearDelayChangedEvent {
  type: "near_delay_changed";
  near_delay_ms: number;
  near_delay_samples: number;
}
export interface InitialDelayChangedEvent {
  type: "initial_delay_changed";
  initial_delay_ms: number;
}
export interface Aec3AgcChangedEvent {
  type: "aec3_agc_changed";
  agc: boolean;
}
export interface LocalvqeNoiseGateChangedEvent {
  type: "localvqe_noise_gate_changed";
  noise_gate: boolean;
  noise_gate_threshold_dbfs: number;
}
// set_bypass 实时生效后的回执(P8-D1:OFF = 穿透,mic 直通虚拟麦)。
export interface BypassChangedEvent {
  type: "bypass_changed";
  bypassed: boolean;
}

// run --status-json 在音频流启动后先发的一条事件。
export interface StartedEvent {
  type: "started";
  cli_version?: string;
  supported_controls?: string[];
  backend: string;
  sample_rate: number;
  frame_ms: number;
  near_delay_ms?: number;
  near_delay_samples?: number;
  reference_channels: string;
  mic_device_sample_rate?: number | null;
  output_device_sample_rate?: number | null;
  reference_device_sample_rate?: number | null;
  io_resampling?: {
    mic: boolean;
    reference: boolean;
    output: boolean;
  };
  diagnostics_session_dir?: string | null;
  // 实际生效的参考源:mac Process Tap 时为 "macos_process_tap";其它见后端 status_name。
  reference_source?: string | null;
}

export type RunEventPayload =
  | RuntimeStatus
  | StartedEvent
  | DiagnosticsDoneEvent
  | DiagnosticsStartedEvent
  | DiagnosticsStoppingEvent
  | ControlErrorEvent
  | StreamErrorEvent
  | ClockSkewWarningEvent
  | ClockSkewResolvedEvent
  | RuntimeErrorEvent
  | OutputLevelChangedEvent
  | NearDelayChangedEvent
  | InitialDelayChangedEvent
  | Aec3AgcChangedEvent
  | LocalvqeNoiseGateChangedEvent
  | BypassChangedEvent;

export type RunEvent = RunEventPayload & { run_id: number };

export interface RunExitEvent {
  run_id: number;
  intentional?: boolean;
  recoverable?: boolean;
}

// ---- doctor audio --json(虚拟声卡检测) ----
export interface DoctorCandidate {
  index: number;
  kind: "input" | "output";
  name: string;
  selector: string;
  stable_id: string;
}
export interface DoctorAudio {
  ok: boolean;
  platform: Platform;
  virtual_output_detected: boolean;
  candidate_inputs: DoctorCandidate[];
  candidate_outputs: DoctorCandidate[];
  recommended_driver: string; // "vb-cable" | "blackhole-2ch" | "vb-cable-mac" ...
  install_status: "installed" | "missing" | "unknown";
  needs_reboot: boolean;
  // 非 macOS 当前返回 "unknown";macOS 返回 granted/denied/undetermined。
  permission_state: "granted" | "denied" | "undetermined" | "unknown";
  // 系统音频录制权限(mac Process Tap reference 用)。helper 可发现=undetermined;
  // 缺失/非 mac=unknown。普通 doctor 不主动触发系统弹窗(首次启动 tap 录制才触发)。
  system_audio_permission?: "granted" | "denied" | "undetermined" | "unknown";
  // --request-system-audio 时回传:probe 结果与失败原因(detail 直达 UI 错误条)。
  system_audio_permission_probe?: {
    state: string;
    ok: boolean;
    requested?: boolean;
    detail?: string;
  } | null;
  hint?: string;
  reference_sources: ReferenceSource[];

  // ↓ 后端建议补的字段(见 RTX/虚拟麦 handoff)。前端向后兼容:缺省时从上面字段派生。
  virtual_route_ready?: boolean; // 同时检测到可输出虚拟设备 + 对应可作 mic 的输入端
  route_status?: "ready" | "incomplete" | "missing" | string;
  recommended_output?: DoctorCandidate | null; // Echoless 应输出到的设备(如 CABLE Input)
  recommended_app_mic?: DoctorCandidate | null; // 通话软件里应选的麦(如 CABLE Output)
}

// ---- nvafx doctor --json(RTX AEC 引擎就绪探针) ----
export type GpuArch = "turing" | "ampere" | "ada" | "blackwell";
export type CheckStatus = "ok" | "warning" | "missing" | "unsupported";

export interface NvafxGpu {
  name: string;
  driver_version: string;
  compute_capability: string;
  arch: GpuArch | null;
}
export interface NvafxCheck {
  name: string;
  status: CheckStatus;
  detail: string;
  action: string | null;
}
export interface NvafxReport {
  runtime_dir: string;
  runtime_dir_source: string;
  gpus: NvafxGpu[];
  selected_arch: GpuArch | null;
  checks: NvafxCheck[];
}
export interface NvafxDoctor {
  ok: boolean;
  report: NvafxReport;
}
