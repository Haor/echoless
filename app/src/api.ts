// 前端 ↔ Tauri 后端的调用层。所有数据都走 JSON 契约;不解析人类日志。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  DeviceList,
  DoctorAudio,
  Platform,
  ProcessorManifest,
  RunEvent,
  ValidateResult,
} from "./types";

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

// 订阅 run 的事件流(started + status 都走这个通道)。返回取消订阅函数。
export function onRunEvent(cb: (e: RunEvent) => void): Promise<UnlistenFn> {
  return listen<RunEvent>("echoless://status", (e) => cb(e.payload));
}
export function onRunExit(cb: () => void): Promise<UnlistenFn> {
  return listen("echoless://exit", () => cb());
}
export function onRunLog(cb: (line: string) => void): Promise<UnlistenFn> {
  return listen<string>("echoless://log", (e) => cb(e.payload));
}

// ---- 配置生成:把 UI 选择拼成后端 PipelineConfig(TOML) ----
export interface ConfigChoice {
  mic: string; // selector / "default"
  output: string; // selector / "default"
  reference: string; // "system" | "none" | "input:N" | ...
  kind: string; // backend kind
  ns: boolean;
  referenceChannels?: "mono" | "stereo";
}

function tomlString(v: string): string {
  return `"${v.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

export function buildConfigToml(c: ConfigChoice): string {
  const lines = [
    `mic = ${tomlString(c.mic)}`,
    `reference = ${tomlString(c.reference)}`,
    `output = ${tomlString(c.output)}`,
    `sample_rate = 48000`,
    `frame_ms = 10`,
    `reference_channels = ${tomlString(c.referenceChannels ?? "mono")}`,
    ``,
    `[[chain]]`,
    `kind = ${tomlString(c.kind)}`,
  ];
  // sonora_aec3 才有 ns/agc;其它 backend 暂走默认。
  if (c.kind === "sonora_aec3") {
    lines.push(`ns = ${c.ns ? "true" : "false"}`);
    lines.push(`agc = false`);
  }
  return lines.join("\n") + "\n";
}
