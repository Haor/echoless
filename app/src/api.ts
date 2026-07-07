// 前端 ↔ Tauri 后端的调用层。所有数据都走 JSON 契约;不解析人类日志。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  DeviceList,
  DoctorAudio,
  NvafxDoctor,
  Platform,
  ProcessorManifest,
  RunEvent,
  ValidateResult,
} from "./types";
import {
  setAec3AgcControlLine,
  setAec3NsControlLine,
  setBypassControlLine,
  setInitialDelayMsControlLine,
  setLocalvqeNoiseGateControlLine,
  setNearDelayMsControlLine,
  setOutputLevelControlLine,
  startDiagnosticsControlLine,
  stopDiagnosticsControlLine,
} from "./runtimeControls";

export function getPlatform(): Promise<Platform> {
  return invoke<Platform>("get_platform");
}

export function listDevices(): Promise<DeviceList> {
  return invoke<DeviceList>("list_devices");
}

export function listProcessors(): Promise<ProcessorManifest> {
  return invoke<ProcessorManifest>("list_processors");
}

export function doctorAudio(): Promise<DoctorAudio> {
  return invoke<DoctorAudio>("doctor_audio");
}

// 用户点击「请求系统音频权限」:触发一次极短 Process Tap probe → macOS 授权弹窗,
// 回传更新后的 doctor(含 system_audio_permission)。仅用户主动调用。
export function requestSystemAudio(): Promise<DoctorAudio> {
  return invoke<DoctorAudio>("request_system_audio");
}

// macOS 主机信息(NVAFX 不可用态右栏填充)。非 mac / 字段缺失时相应字段为 null。
export interface MacSystemInfo {
  model?: string | null;
  os_version?: string | null;
  chip?: string | null;
  memory_gb?: number | null;
  cores?: number | null;
}
export function macSystemInfo(): Promise<MacSystemInfo> {
  return invoke<MacSystemInfo>("mac_system_info");
}

// LocalVQE model/runtime assets from the official HF repo.
export interface LocalvqeModel {
  filename: string;
  path: string;
  source: "downloaded" | string;
}
// native runtime 随包分发(2026-07-05 定案),不走下载;native_ready 只兜 dev 病态 case。
export interface LocalvqeAssets {
  models_dir: string;
  models: LocalvqeModel[];
  native_ready?: boolean;
  library_path?: string | null;
  native_dir?: string | null;
  native_files?: string[];
  cli_path?: string | null;
  process_tap_helper_path?: string | null;
}
export function localvqeAssets(): Promise<LocalvqeAssets> {
  return invoke<LocalvqeAssets>("localvqe_assets");
}
export function downloadLocalvqeModel(filename: string): Promise<string> {
  return invoke<string>("download_localvqe_model", { filename });
}

// 主动近端延迟侦测 / AEC 链路诊断。后端 shell `echoless probe-delay --json`,约 15 秒,
// 会外放一串蜂鸣 —— 调用前必须先停掉主 run。字段以 CLI `probe-delay --json` 实测为准(docs/CLI.md)。
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
export function probeDelay(p: {
  mic: string;
  reference: string;
  output: string;
}): Promise<NearDelayProbeResult> {
  return invoke<NearDelayProbeResult>("probe_delay", {
    mic: p.mic,
    reference: p.reference,
    output: p.output,
  });
}

export function nvafxDoctor(runtimeDir?: string): Promise<NvafxDoctor> {
  return invoke<NvafxDoctor>("nvafx_doctor", { runtimeDir: runtimeDir ?? null });
}

// RTX AEC runtime 安装:解压 common + 按架构 model zip,回传安装后的 doctor 报告。
export function nvafxInstall(p: {
  commonZip: string;
  modelZip: string;
  runtimeDir?: string;
}): Promise<NvafxDoctor> {
  return invoke<NvafxDoctor>("nvafx_install", {
    commonZip: p.commonZip,
    modelZip: p.modelZip,
    runtimeDir: p.runtimeDir ?? null,
  });
}

// 从公共 GitHub release 下载 + 安装(后端按 GPU 架构自动选模型)。回传安装后 doctor。
export function nvafxDownloadInstall(p: {
  runtimeDir?: string;
}): Promise<NvafxDoctor> {
  return invoke<NvafxDoctor>("nvafx_download_install", {
    runtimeDir: p.runtimeDir ?? null,
  });
}

export function openUrl(url: string): Promise<void> {
  return invoke<void>("open_url", { url });
}

export function defaultDiagDir(): Promise<string> {
  return invoke<string>("default_diag_dir");
}

export function openPath(path: string): Promise<void> {
  return invoke<void>("open_path", { path });
}

export function validateConfig(tomlText: string): Promise<ValidateResult> {
  return invoke<ValidateResult>("validate_config", { tomlText });
}

export function startRun(
  tomlText: string,
  statsIntervalMs = 80,
): Promise<void> {
  return invoke<void>("start_run", { tomlText, statsIntervalMs });
}

export function stopRun(): Promise<void> {
  return invoke<void>("stop_run");
}

// 向运行中的子进程 stdin 写一行 JSON 控制命令。具体能力以 started.supported_controls 为准。
function sendRunControl(line: string): Promise<void> {
  return invoke<void>("send_run_control", { line });
}
export function startDiagnostics(
  recordDir: string,
  maxSeconds: number | null,
): Promise<void> {
  return sendRunControl(startDiagnosticsControlLine(recordDir, maxSeconds));
}
export function stopDiagnostics(): Promise<void> {
  return sendRunControl(stopDiagnosticsControlLine());
}
// 运行中实时改输出电平(0-100),逐 buffer 生效、零掉音。仅在 run 存活时调用。
export function setOutputLevel(level: number): Promise<void> {
  return sendRunControl(setOutputLevelControlLine(level));
}
// 运行中实时改近端对齐延迟(ms),只调整处理线程里的 delay buffer,不重启 run。
export function setNearDelayMs(nearDelayMs: number): Promise<void> {
  return sendRunControl(setNearDelayMsControlLine(nearDelayMs));
}
export function setInitialDelayMs(initialDelayMs: number): Promise<void> {
  return sendRunControl(setInitialDelayMsControlLine(initialDelayMs));
}
export function setAec3Ns(ns: boolean, nsLevel: string): Promise<void> {
  return sendRunControl(setAec3NsControlLine(ns, nsLevel));
}
export function setAec3Agc(agc: boolean): Promise<void> {
  return sendRunControl(setAec3AgcControlLine(agc));
}
// P8-D1:穿透开关(OFF = mic 原样直通虚拟麦)。chain 级 bypass,AEC 保温,
// 15ms crossfade;运行中实时生效。
export function setBypass(enabled: boolean): Promise<void> {
  return sendRunControl(setBypassControlLine(enabled));
}
// Windows 托盘偏好(P5 契约):
// 启动时与每次变更时同步到 Rust;非 Windows 平台后端忽略。
export function setTrayPrefs(closeToTray: boolean): Promise<void> {
  return invoke("set_tray_prefs", { closeToTray });
}
export function setLocalvqeNoiseGate(
  noiseGate: boolean,
  noiseGateThresholdDbfs: number,
): Promise<void> {
  return sendRunControl(
    setLocalvqeNoiseGateControlLine(noiseGate, noiseGateThresholdDbfs),
  );
}

// 订阅 run 的事件流(started + status 都走这个通道)。返回取消订阅函数。
export function onRunEvent(cb: (e: RunEvent) => void): Promise<UnlistenFn> {
  return listen<RunEvent>("echoless://status", (e) => cb(e.payload));
}
export function onRunExit(
  cb: (e: { intentional?: boolean }) => void,
): Promise<UnlistenFn> {
  return listen<{ intentional?: boolean }>("echoless://exit", (e) =>
    cb(e.payload ?? {}),
  );
}
export function onRunLog(cb: (line: string) => void): Promise<UnlistenFn> {
  return listen<string>("echoless://log", (e) => cb(e.payload));
}
// 原生侧设备热插拔通知(macOS CoreAudio 监听;WKWebView 不触发 devicechange)。
export function onDevicesChanged(cb: () => void): Promise<UnlistenFn> {
  return listen("echoless://devices-changed", () => cb());
}
// probe-delay 进度(CLI stderr JSONL 转发):beep_train_start 携带蜂鸣节奏,
// 前端据此把进度灯对齐到真实播放时刻。
export interface ProbeProgress {
  type?: string;
  stage?: string;
  pre_roll_ms?: number;
  beep_ms?: number;
  gap_ms?: number;
  beeps?: number;
}
export function onProbeProgress(
  cb: (p: ProbeProgress) => void,
): Promise<UnlistenFn> {
  return listen<ProbeProgress>("echoless://probe-progress", (e) =>
    cb(e.payload ?? {}),
  );
}

// ---- 配置生成:把 UI 选择拼成后端 PipelineConfig(TOML) ----
export interface PipelineCfg {
  sample_rate: number;
  frame_ms: number;
  reference_channels: "mono" | "stereo";
  // 顶层近端对齐延迟(ms)。undefined = 用后端默认(macOS 25 / 其它 0);侦测后写入实测推荐值。
  near_delay_ms?: number;
  // 最终输出电平 0-100(50=原声)。后端契约键名 output_level,曲线/软限幅都在后端;undefined = 50。
  output_level?: number;
}

const OUTPUT_LEVEL_UNITY = 50;
// 仅用于前端 tooltip 显示当前 dB;曲线与后端 output_level_gain 完全一致(gain=(v/50)^log2(3))。
const OUTPUT_LEVEL_EXP = Math.log2(3); // ≈1.58496
export function outputLevelToGain(level: number): number {
  const v = Math.max(0, Math.min(100, level));
  return Math.pow(v / OUTPUT_LEVEL_UNITY, OUTPUT_LEVEL_EXP);
}
export interface DiagnosticsCfg {
  record_dir: string;
  max_seconds: number | null;
}
export interface ConfigChoice {
  mic: string; // selector / stable_id / "default"
  output: string;
  reference: string; // "system" | "none" | "input:<stable_id>" | ...
  kind: string; // backend kind
  pipeline: PipelineCfg;
  params: Record<string, unknown>; // chain[0] 参数(不含 reference_channels)
  diagnostics?: DiagnosticsCfg | null; // 开启录制时写入 [diagnostics]
}

function tomlString(v: string): string {
  return `"${v.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

function tomlValue(v: unknown): string | null {
  if (v === null || v === undefined || v === "") return null;
  if (typeof v === "boolean") return v ? "true" : "false";
  if (typeof v === "number") return Number.isFinite(v) ? String(v) : null;
  return tomlString(String(v));
}

export function buildConfigToml(c: ConfigChoice): string {
  const lines = [
    `mic = ${tomlString(c.mic)}`,
    `reference = ${tomlString(c.reference)}`,
    `output = ${tomlString(c.output)}`,
    `sample_rate = ${c.pipeline.sample_rate}`,
    `frame_ms = ${c.pipeline.frame_ms}`,
    `reference_channels = ${tomlString(c.pipeline.reference_channels)}`,
  ];
  // 仅在显式设过(含 0)时 emit;不设则交由后端平台默认。
  if (c.pipeline.near_delay_ms != null)
    lines.push(`near_delay_ms = ${c.pipeline.near_delay_ms}`);
  // 输出电平:发原始 0-100 整数,曲线/软限幅由后端做(单一真理源,不在前端算 gain)。
  lines.push(`output_level = ${c.pipeline.output_level ?? OUTPUT_LEVEL_UNITY}`);
  lines.push(``);
  if (c.diagnostics) {
    lines.push(`[diagnostics]`);
    lines.push(`record_dir = ${tomlString(c.diagnostics.record_dir)}`);
    if (c.diagnostics.max_seconds != null)
      lines.push(`max_seconds = ${c.diagnostics.max_seconds}`);
    lines.push(``);
  }
  lines.push(`[[chain]]`, `kind = ${tomlString(c.kind)}`);
  for (const [k, raw] of Object.entries(c.params)) {
    if (k === "reference_channels") continue; // 顶层管线项,不重复
    const val = tomlValue(raw);
    if (val !== null) lines.push(`${k} = ${val}`);
  }
  return lines.join("\n") + "\n";
}
