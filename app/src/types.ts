// Echoless 后端 JSON 契约的 TS 镜像。
// 形状以 `echoless <cmd> --json` 实测为准(见 docs/frontend/*.md)。

export type Platform = "windows" | "macos" | "linux";

// ---- devices --json ----
export interface AudioDevice {
  id: string;
  index: number;
  name: string;
  kind: "input" | "output";
  is_default: boolean;
  selector: string; // 设备索引字符串(跨重启不稳)
  stable_id: string; // 跨重启稳定 id(CoreAudio/WASAPI 派生);mic/output 配置优先用它
  default_sample_rate: number;
  channels: number;
  sample_format: string;
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
  | "sonora_aec3"
  | "localvqe"
  | "nvidia_afx_aec";

export interface Processor {
  kind: ProcessorKind;
  label: string;
  platforms: Platform[];
  default: boolean;
  experimental: boolean;
  diagnostic?: boolean;
  requires_doctor_ok?: boolean;
  constraints?: Record<string, unknown>;
  params: Record<string, ParamSpec>;
}

export interface ProcessorManifest {
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

  // 真实示波波形:每路 64 点,当前 stats interval 内的 peak 包络,范围 [0,1]。
  mic_wave?: number[];
  ref_wave?: number[];
  out_wave?: number[];
}

// run --status-json 在音频流启动后先发的一条事件。
export interface StartedEvent {
  type: "started";
  backend: string;
  sample_rate: number;
  frame_ms: number;
  reference_channels: string;
}

export type RunEvent = RuntimeStatus | StartedEvent;

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
  permission_state: "granted" | "denied" | "undetermined";
  hint?: string;
  reference_sources: ReferenceSource[];
}
