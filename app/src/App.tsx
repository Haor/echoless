import {
  useCallback,
  useEffect,
  useReducer,
  useRef,
  useState,
  type Dispatch,
  type MutableRefObject,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import {
  buildConfigToml,
  defaultDiagDir,
  doctorAudio,
  getAutostartEnabled,
  getStartupMode,
  getPlatform,
  listDevices,
  listProcessors,
  nvafxDoctor,
  nvafxDownloadInstall,
  nvafxInstall,
  onDevicesChanged,
  onNvafxProgress,
  onRunEvent,
  onRunExit,
  onRunLog,
  openPath,
  requestSystemAudio,
  setAec3Agc,
  setBypass,
  setInitialDelayMs,
  setLocalvqeNoiseGate,
  setNearDelayMs,
  setOutputLevel,
  setAutostartEnabled as persistAutostartEnabled,
  settleStartupLaunch,
  setTrayPrefs,
  startDiagnostics,
  startRun,
  stopDiagnostics,
  stopRun,
  validateConfig,
  type PipelineCfg,
} from "./api";
import type {
  AudioDevice,
  DeviceList,
  DoctorAudio,
  NvafxDoctor,
  NoiseMode,
  NoiseSuppressionManifest,
  Platform,
  Processor,
} from "./types";
import { useI18n } from "./i18n";
import {
  AppIcon,
  CapClose,
  CapMax,
  CapMin,
  IcoInput,
  IcoModel,
  IcoNoise,
  IcoOutput,
} from "./components/icons";
import type { Telemetry } from "./components/Scope";
import { Dropdown } from "./components/Dropdown";
import { ScrambleText } from "./components/ScrambleText";
import { SlideSwitch } from "./components/SlideSwitch";
import { VolumeWheel } from "./components/VolumeWheel";
import { RuntimeDiagnosticsPage } from "./components/RuntimeDiagnosticsPage";
import { RuntimeFooterBars } from "./components/RuntimeFooterBars";
import { RuntimeSignalPanel } from "./components/RuntimeSignalPanel";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { Hint } from "./components/Hint";
import {
  acceptRunEvent,
  acceptRunExit,
  INITIAL_RUN_GENERATION,
  observeRunStart,
  type RunGeneration,
} from "./runGeneration";
import { controlErrorMessage, streamErrorMessage } from "./runEventDisplay";
import {
  RuntimeStatusStrip,
  RuntimeSubline,
  useRunStatusKind,
} from "./components/RuntimeStatusStrip";
import { AdvancedPage } from "./pages/AdvancedPage";
import { EnginePage } from "./pages/EnginePage";
import { RtxSetupPage } from "./pages/RtxSetupPage";
import { MicSetupPage } from "./pages/MicSetupPage";
import { simNvafxDoctor, type RtxState } from "./nvafx";
import { simMicDoctor, type MicState } from "./mic";
import {
  allowedNoiseModes,
  modelFileName,
  normalizeNoiseMode,
  patchNoiseModeParam,
  shouldSelectNoiseMode,
  isNearDelayOnlyPatch,
  hotInitialDelayValue,
  hotLocalvqeNoiseGateValue,
  platformNearDelayDefault,
  createRunIntentGuard,
  createSerialQueue,
  bypassToggleTarget,
  settleBypassObservation,
  clearBypassPending,
  routeEngineKindSelection,
  shouldPickLocalvqeModel,
  canChangePipeline,
  pipelineForEngineKind,
} from "./engineLogic";
import {
  publishRuntimeStatus,
  resetRuntimeHealth,
  resetRuntimeLive,
  setDiagnosticsSessionDir,
} from "./runtimeTelemetry";
import { REQUIRED_RUN_CONTROLS } from "./runtimeControls";
import { createAsyncListenerScope } from "./asyncListener";
import { settleBootGate } from "./bootGate";
import {
  consumeStreamRecovery,
  INITIAL_STREAM_RECOVERY,
  requestStreamRecovery,
  type StreamRecoveryState,
} from "./streamRecovery";
import {
  createStartupCleanup,
  expectStartupRun,
  INITIAL_STARTUP_RUN_HANDSHAKE,
  observeStartupRunStarted,
  shouldAttemptAutoStart,
  shouldRevealWindow,
  startupDataReady,
  type StartupMode,
} from "./startupMode";
import {
  beginAutostartChange,
  displayAutostartEnabled,
  rejectAutostartChange,
  settleAutostart,
  type AutostartPreference,
} from "./autostartPreference";

const appWindow = getCurrentWindow();
const ensureStartupRuntimeCleanup = createStartupCleanup(stopRun);

function revealMainWindow(): void {
  const window = getCurrentWindow();
  window
    .show()
    .then(() => window.setFocus())
    .catch(() => {});
}

const DEVICE_SELECTION_KEY = "echoless.deviceSelection.v1";

// Windows 托盘偏好(P5 契约):持久化在前端,启动/变更时推给 Rust。
// UI 只留「关闭到托盘」一个开关(用户定案 2026-07-05:符合一般使用习惯);
// 最小化到托盘退役,Rust 端恒收 false(旧存档里的 minimizeToTray 忽略)。
const TRAY_PREFS_KEY = "echoless.trayPrefs.v1";
export type TrayPrefsState = { closeToTray: boolean };
function readTrayPrefs(): TrayPrefsState {
  try {
    const raw = localStorage.getItem(TRAY_PREFS_KEY);
    const p = raw ? JSON.parse(raw) : null;
    return { closeToTray: Boolean(p?.closeToTray) };
  } catch {
    return { closeToTray: false };
  }
}

// 设备选择值统一用 stable_id(跨重启稳定;mic/output 配置直接吃它)。
// 选默认输出:优先虚拟声卡(VB-CABLE / BlackHole),否则系统默认。
function pickDefaultOutput(outs: AudioDevice[]): string {
  const virt = outs.find((d) => /cable|blackhole|vb-?audio|echoless|null/i.test(d.name));
  if (virt) return virt.stable_id;
  return (
    outs.find((d) => d.is_default)?.stable_id ?? outs[0]?.stable_id ?? "default"
  );
}
function pickDefaultInput(ins: AudioDevice[]): string {
  return ins.find((d) => d.is_default)?.stable_id ?? ins[0]?.stable_id ?? "default";
}

type SavedDeviceSelection = {
  input?: string;
  output?: string;
  reference?: string;
};

function readSavedDeviceSelection(): SavedDeviceSelection {
  try {
    const raw = localStorage.getItem(DEVICE_SELECTION_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return {};
    return {
      input: typeof parsed.input === "string" ? parsed.input : undefined,
      output: typeof parsed.output === "string" ? parsed.output : undefined,
      reference: typeof parsed.reference === "string" ? parsed.reference : undefined,
    };
  } catch {
    return {};
  }
}

function saveDeviceSelection(selection: SavedDeviceSelection) {
  try {
    localStorage.setItem(DEVICE_SELECTION_KEY, JSON.stringify(selection));
  } catch {
    // Best-effort preference cache; failing to persist must not block audio.
  }
}

function deviceSelectionStillExists(devices: AudioDevice[], value: string): boolean {
  if (value === "default" || value === "") return false;
  return devices.some((d) => d.stable_id === value || d.selector === value);
}

function referenceSelectionStillExists(devices: DeviceList, value: string): boolean {
  return devices.reference_sources.some(
    (r) => r.available && (r.selector ?? r.id) === value,
  );
}

function pickReference(devices: DeviceList, current: string): string {
  if (referenceSelectionStillExists(devices, current)) return current;
  const sys = devices.reference_sources.find((r) => r.id === "system");
  if (sys?.available) return "system";
  const monitor = devices.reference_sources.find(
    (r) => r.available && r.kind === "input" && /monitor/i.test(r.label),
  );
  if (monitor) return monitor.selector ?? monitor.id;
  return "none";
}

function parseDevPlatform(value: string | null): Platform | null {
  if (value === "win" || value === "windows") return "windows";
  if (value === "mac" || value === "macos") return "macos";
  if (value === "linux") return "linux";
  return null;
}

function cycleDevPlatform(current: Platform | null): Platform | null {
  if (current === null || current === "macos") return "windows";
  if (current === "windows") return "linux";
  return null;
}

function platformTag(platform: Platform): string {
  if (platform === "windows") return "WIN";
  if (platform === "linux") return "LINUX";
  return "MAC";
}

function ioBackendLabel(platform: Platform): string {
  if (platform === "macos") return "COREAUDIO";
  if (platform === "linux") return "PIPEWIRE";
  return "WASAPI";
}

const MODELS: { kind: string; label: string }[] = [
  { kind: "aec3", label: "AEC3" },
  { kind: "localvqe", label: "LVQE" },
  { kind: "nvidia_afx_aec", label: "NVAFX" },
];

function modelName(kind: string): string {
  return MODELS.find((m) => m.kind === kind)?.label ?? kind.toUpperCase();
}

// 由 manifest 推导某 backend 的 chain 参数默认值(reference_channels 归到 pipeline)。
function defaultParams(proc: Processor | undefined): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  if (!proc) return out;
  for (const [k, spec] of Object.entries(proc.params)) {
    if (k === "reference_channels") continue;
    out[k] =
      spec.default !== undefined
        ? spec.default
        : spec.type === "bool"
          ? false
          : null;
  }
  return out;
}

const EMPTY_DEVICES: DeviceList = {
  ok: true,
  inputs: [],
  outputs: [],
  reference_sources: [],
};

type View =
  | "overview"
  | "engine"
  | "advanced"
  | "diagnostics"
  | "rtxsetup"
  | "micsetup";

type IoResamplingState = {
  mic: boolean;
  micRate: number | null;
} | null;

type AppState = {
  platform: Platform;
  devices: DeviceList;
  processors: Processor[];
  noiseSuppression: NoiseSuppressionManifest | null;
  powerOn: boolean;
  busy: boolean;
  err: string | null;
  view: View;
  doctor: DoctorAudio | null;
  nvafx: NvafxDoctor | null;
  nvafxBusy: boolean;
  nvafxPct: number | null;
  nvafxStage: "runtime" | "model" | null;
  nvafxRecv: number | null;
  dev: boolean;
  devRtxState: RtxState;
  devMicState: MicState;
  devPlatform: Platform | null;
  io: IoResamplingState;
  rec: boolean;
  // P8-D1:穿透中(sidecar 活着,mic 直通,AEC 保温)。UI 的「电源 OFF」= 此态。
  bypassed: boolean;
  bypassPending: boolean | null;
  diagSeconds: number | null;
  diagDir: string;
};

type SelectionState = {
  selInput: string;
  selOutput: string;
  reference: string;
};

type EngineState = {
  kind: string;
  noiseMode: NoiseMode;
  noiseParamsByMode: Partial<
    Record<NoiseMode, Record<string, unknown>>
  >;
  pipeline: PipelineCfg;
  params: Record<string, unknown>;
};

type Override = Partial<{
  mic: string;
  output: string;
  reference: string;
  kind: string;
  noiseMode: NoiseMode;
  pipeline: PipelineCfg;
  params: Record<string, unknown>;
}>;

type Patch<T> = Partial<T> | ((state: T) => T);

function patchReducer<T>(state: T, patch: Patch<T>): T {
  return typeof patch === "function" ? patch(state) : { ...state, ...patch };
}

const INITIAL_PIPELINE: PipelineCfg = {
  sample_rate: 48000,
  frame_ms: 10,
  reference_channels: "mono",
};

const INITIAL_APP_STATE: AppState = {
  platform: "macos",
  devices: EMPTY_DEVICES,
  processors: [],
  noiseSuppression: null,
  powerOn: false,
  busy: false,
  err: null,
  view: "overview",
  doctor: null,
  nvafx: null,
  nvafxBusy: false,
  nvafxPct: null,
  nvafxStage: null,
  nvafxRecv: null,
  dev: false,
  devRtxState: "runtime_not_installed",
  devMicState: "missing",
  devPlatform: null,
  io: null,
  rec: false,
  bypassed: false,
  bypassPending: null,
  diagSeconds: null,
  diagDir: "",
};

const INITIAL_ENGINE_STATE: EngineState = {
  kind: "aec3",
  noiseMode: "off",
  noiseParamsByMode: {},
  pipeline: INITIAL_PIPELINE,
  params: {},
};

// 引擎配置持久化:kind + pipeline(含 near_delay/output_level)+ 每引擎参数。
// 模块加载时读一次;之后每次变更由 effect 写回。
const ENGINE_STATE_KEY = "echoless.engine.v2";

type SavedEngineState = {
  kind?: string;
  noiseMode?: NoiseMode;
  noiseParamsByMode?: Partial<
    Record<NoiseMode, Record<string, unknown>>
  >;
  pipeline?: Partial<PipelineCfg>;
  paramsByKind?: Record<string, Record<string, unknown>>;
};

function readSavedEngineState(): SavedEngineState {
  try {
    const raw = localStorage.getItem(ENGINE_STATE_KEY);
    if (!raw) return {};
    const p = JSON.parse(raw);
    if (!p || typeof p !== "object") return {};
    return {
      kind: typeof p.kind === "string" ? p.kind : undefined,
      noiseMode:
        p.noiseMode === "webrtc" ||
        p.noiseMode === "rnnoise" ||
        p.noiseMode === "off"
          ? p.noiseMode
          : undefined,
      noiseParamsByMode:
        p.noiseParamsByMode && typeof p.noiseParamsByMode === "object"
          ? p.noiseParamsByMode
          : undefined,
      pipeline:
        p.pipeline && typeof p.pipeline === "object" ? p.pipeline : undefined,
      paramsByKind:
        p.paramsByKind && typeof p.paramsByKind === "object"
          ? p.paramsByKind
          : undefined,
    };
  } catch {
    return {};
  }
}

const SAVED_ENGINE = readSavedEngineState();

function initEngineState(): EngineState {
  const pl = SAVED_ENGINE.pipeline ?? {};
  const kind = SAVED_ENGINE.kind ?? INITIAL_ENGINE_STATE.kind;
  const savedPipeline: PipelineCfg = {
    sample_rate:
      typeof pl.sample_rate === "number"
        ? pl.sample_rate
        : INITIAL_PIPELINE.sample_rate,
    frame_ms:
      typeof pl.frame_ms === "number"
        ? pl.frame_ms
        : INITIAL_PIPELINE.frame_ms,
    reference_channels: pl.reference_channels === "stereo" ? "stereo" : "mono",
    near_delay_ms:
      typeof pl.near_delay_ms === "number" ? pl.near_delay_ms : undefined,
    output_level:
      typeof pl.output_level === "number" ? pl.output_level : undefined,
  };
  return {
    kind,
    noiseMode: SAVED_ENGINE.noiseMode ?? "off",
    noiseParamsByMode: SAVED_ENGINE.noiseParamsByMode ?? {},
    pipeline: pipelineForEngineKind(kind, savedPipeline),
    params: SAVED_ENGINE.paramsByKind?.[kind] ?? {},
  };
}

function initSelection(): SelectionState {
  const saved = readSavedDeviceSelection();
  return {
    selInput: saved.input ?? "default",
    selOutput: saved.output ?? "default",
    reference: saved.reference ?? "system",
  };
}

// 浏览器预览直达(设计稿 hash 直链的 app 版):?view=advanced&dev=1&os=linux。
// Tauri 里没有 query,恒回落初始值。
function initAppState(): AppState {
  try {
    const q = new URLSearchParams(window.location.search);
    const v = q.get("view");
    const views: View[] = [
      "overview",
      "engine",
      "advanced",
      "diagnostics",
      "rtxsetup",
      "micsetup",
    ];
    return {
      ...INITIAL_APP_STATE,
      view: views.includes(v as View) ? (v as View) : "overview",
      dev: import.meta.env.DEV && q.has("dev"),
      devPlatform: parseDevPlatform(q.get("os")),
    };
  } catch {
    return INITIAL_APP_STATE;
  }
}

type DeviceEnumerationDeps = {
  doctorRef: MutableRefObject<DoctorAudio | null>;
  powerOnRef: MutableRefObject<boolean>;
  telRef: MutableRefObject<Telemetry>;
  applyChangeRef: MutableRefObject<(next: Override) => void>;
  updateApp: Dispatch<Patch<AppState>>;
  noteError: (err: string | null) => void;
};

function useDeviceEnumeration({
  doctorRef,
  powerOnRef,
  telRef,
  applyChangeRef,
  updateApp,
  noteError,
}: DeviceEnumerationDeps) {
  const [selection, updateSelection] = useReducer(
    patchReducer<SelectionState>,
    undefined,
    initSelection,
  );
  const selectionRef = useRef(selection);
  selectionRef.current = selection;

  // 返回 promise 供就绪门等待首次枚举完成(后续热插拔刷新忽略返回值即可)。
  const refreshDevices = useCallback(() => {
    return listDevices()
      .then((d) => {
        updateApp({ devices: d });
        const cur = selectionRef.current;
        const next: SelectionState = {
          selInput: deviceSelectionStillExists(d.inputs, cur.selInput)
            ? cur.selInput
            : pickDefaultInput(d.inputs),
          selOutput: deviceSelectionStillExists(d.outputs, cur.selOutput)
            ? cur.selOutput
            : pickDefaultOutput(d.outputs),
          // 默认 reference:system 可用就用 system,否则退到 none;用户改过则保留。
          reference: pickReference(d, cur.reference),
        };
        // 立即同步 ref:一次插拔常连发多个 devicechange,防止后续 refresh
        // 用陈旧选择重复判定、重复重启。
        selectionRef.current = next;
        updateSelection(next);
        // 运行中设备被拔,选择被迫回退 → 把新设备真正应用到管线(重启 run)。
        // 只改选中值不重启的话,sidecar 仍抱着已死的输入流:波形冻结、
        // 采样率徽标停留在旧设备,直到用户手动重选。
        const override: Override = {};
        if (next.selInput !== cur.selInput) override.mic = next.selInput;
        if (next.selOutput !== cur.selOutput) override.output = next.selOutput;
        if (next.reference !== cur.reference)
          override.reference = next.reference;
        if (Object.keys(override).length > 0) applyChangeRef.current(override);

        // 系统音频权限是外部可变状态(用户随时可在系统设置里改;dev 下 TCC 把授权
        // 记在 responsible process 头上,终端/Cursor 更新即被重置)——doctor 只在
        // mount 查一次会让「授予权限」按钮在授予后仍挂着。未授予期间搭设备刷新的
        // 车重查;已授予则零开销。
        const perm = doctorRef.current?.system_audio_permission;
        if (perm === "denied" || perm === "undetermined") {
          doctorAudio()
            .then((doc) => {
              updateApp({ doctor: doc });
              // 授权前创建的 Process Tap 永远输出静音(CoreAudio 不给旧 tap 补活),
              // 而 P8 电源开关只是 bypass 不重建管线 —— 刚授予 + 正在跑 + 参考静音
              // 就自动重启一次管线,让 tap 带着新权限重建。
              if (
                doc.system_audio_permission === "granted" &&
                powerOnRef.current &&
                selectionRef.current.reference === "system" &&
                telRef.current.ref <= -100
              ) {
                applyChangeRef.current({});
              }
            })
            .catch(() => {});
        }
      })
      .catch((e) => noteError(String(e)));
  }, [applyChangeRef, doctorRef, noteError, powerOnRef, telRef, updateApp]);

  useEffect(() => {
    saveDeviceSelection({
      input: selection.selInput,
      output: selection.selOutput,
      reference: selection.reference,
    });
  }, [selection.selInput, selection.selOutput, selection.reference]);

  return { selection, updateSelection, refreshDevices };
}

type EngineConfigDeps = {
  processors: Processor[];
  noiseSuppression: NoiseSuppressionManifest | null;
  platform: Platform;
  dev: boolean;
  nvafx: NvafxDoctor | null;
  powerOnRef: MutableRefObject<boolean>;
  applyChangeRef: MutableRefObject<(next: Override) => void>;
  stopForEngineSetupRef: MutableRefObject<() => void>;
  noteError: (err: string | null) => void;
  gotoView: (view: View) => void;
  hasRunControl: (cmd: string) => boolean;
  reportMissingRunControl: (cmd: string) => void;
};

function useEngineConfig({
  processors,
  noiseSuppression,
  platform,
  dev,
  nvafx,
  powerOnRef,
  applyChangeRef,
  stopForEngineSetupRef,
  noteError,
  gotoView,
  hasRunControl,
  reportMissingRunControl,
}: EngineConfigDeps) {
  const [engineState, updateEngine] = useReducer(
    patchReducer<EngineState>,
    undefined,
    initEngineState,
  );
  const { kind, noiseMode, noiseParamsByMode, pipeline, params } = engineState;
  const noiseModeRef = useRef(noiseMode);
  noiseModeRef.current = noiseMode;
  const noiseParamsByModeRef = useRef(noiseParamsByMode);
  noiseParamsByModeRef.current = noiseParamsByMode;
  const pipelineRef = useRef(pipeline);
  pipelineRef.current = pipeline;
  const paramsRef = useRef(params);
  paramsRef.current = params;
  // 记住每个引擎的参数(如 LocalVQE 选的模型),切换引擎再切回来不丢。跨重启持久化。
  const paramsByKind = useRef<Record<string, Record<string, unknown>>>(
    SAVED_ENGINE.paramsByKind ?? {},
  );
  const kindRef = useRef(kind);
  kindRef.current = kind;

  // 引擎就绪判定:AEC3 永远就绪;LocalVQE 需模型;NVAFX 需平台支持 + doctor 通过。
  // 开发态(dev)临时解开 NVAFX 的平台/doctor 门槛,用于走通前端流程。
  function engineReady(k: string): boolean {
    const proc = processors.find((p) => p.kind === k);
    if (proc && !proc.platforms.includes(platform) && !dev) return false;
    // LocalVQE 是否就绪要看它自己持久化的模型(可能当前激活的是别的引擎),
    // 否则在 AEC3 激活时点 LocalVQE 会因 params.model 为空而误判未就绪、每次跳引擎页。
    if (k === "localvqe")
      return Boolean(
        k === kindRef.current
          ? paramsRef.current.model
          : paramsByKind.current["localvqe"]?.model,
      );
    if (k === "nvidia_afx_aec") return dev || Boolean(nvafx?.ok);
    return true;
  }

  // 切 backend:优先恢复该引擎上次的参数(保住 LocalVQE 选过的模型),否则用 manifest 默认。
  function changeKind(k: string) {
    routeEngineKindSelection(kindRef, k, engineReady(k), {
      setup: (target) => {
        stopForEngineSetupRef.current();
        gotoView(target === "nvidia_afx_aec" ? "rtxsetup" : "engine");
      },
      apply: (previous, target) => {
        paramsByKind.current[previous] = paramsRef.current;
        const np =
          paramsByKind.current[target] ??
          defaultParams(processors.find((p) => p.kind === target));
        paramsByKind.current[target] = np;
        paramsRef.current = np;
        const nextPipeline = pipelineForEngineKind(
          target,
          pipelineRef.current,
        );
        const nextNoiseMode = normalizeNoiseMode(
          noiseSuppression,
          target,
          np,
          noiseModeRef.current,
        );
        pipelineRef.current = nextPipeline;
        noiseModeRef.current = nextNoiseMode;
        updateEngine({
          kind: target,
          noiseMode: nextNoiseMode,
          params: np,
          pipeline: nextPipeline,
        });
        applyChangeRef.current({
          kind: target,
          noiseMode: nextNoiseMode,
          params: np,
          pipeline: nextPipeline,
        });
      },
    });
  }

  // 改单个 chain 参数(NOISE / Advanced)。
  function setParam(key: string, val: unknown) {
    const np = { ...paramsRef.current, [key]: val };
    paramsRef.current = np; // 同步更新 ref:探测后自动恢复引擎时能立刻读到新 initial_delay_ms
    paramsByKind.current[kind] = np;
    updateEngine({ params: np });
    if (kind === "aec3" && key === "initial_delay_ms") {
      if (powerOnRef.current) {
        if (!hasRunControl("set_initial_delay_ms")) {
          reportMissingRunControl("set_initial_delay_ms");
          return;
        }
        const delayMs = hotInitialDelayValue(val);
        if (delayMs == null) {
          noteError("initial_delay_ms must be a finite number");
          return;
        }
        setInitialDelayMs(delayMs).catch((e) => noteError(String(e)));
      }
      return;
    }
    if (kind === "aec3" && key === "agc") {
      if (powerOnRef.current) {
        if (!hasRunControl("set_aec3_agc")) {
          reportMissingRunControl("set_aec3_agc");
          return;
        }
        setAec3Agc(Boolean(val)).catch((e) => noteError(String(e)));
      }
      return;
    }
    if (
      kind === "localvqe" &&
      (key === "noise_gate" || key === "noise_gate_threshold_dbfs")
    ) {
      if (powerOnRef.current) {
        if (!hasRunControl("set_localvqe_noise_gate")) {
          reportMissingRunControl("set_localvqe_noise_gate");
          return;
        }
        const gate = hotLocalvqeNoiseGateValue(np);
        if (gate == null) {
          noteError("noise_gate_threshold_dbfs must be a finite number");
          return;
        }
        setLocalvqeNoiseGate(gate.enabled, gate.thresholdDbfs).catch((e) =>
          noteError(String(e)),
        );
      }
      return;
    }
    applyChangeRef.current({ params: np });
  }

  // 选 LocalVQE 模型(清单常驻):原子地切到 localvqe 引擎并设 model,避免把 model 写到当前引擎上。
  function pickLocalvqeModel(path: string) {
    const selectedModel =
      kindRef.current === "localvqe"
        ? paramsRef.current.model
        : paramsByKind.current["localvqe"]?.model;
    if (!shouldPickLocalvqeModel(selectedModel, path)) return;

    const base =
      paramsByKind.current["localvqe"] ??
      defaultParams(processors.find((p) => p.kind === "localvqe"));
    const np = { ...base, model: path };
    paramsByKind.current[kindRef.current] = paramsRef.current;
    paramsByKind.current["localvqe"] = np;
    const nextNoiseMode = normalizeNoiseMode(
      noiseSuppression,
      "localvqe",
      np,
      noiseModeRef.current,
    );
    kindRef.current = "localvqe";
    paramsRef.current = np;
    noiseModeRef.current = nextNoiseMode;
    updateEngine({
      kind: "localvqe",
      noiseMode: nextNoiseMode,
      params: np,
    });
    applyChangeRef.current({
      kind: "localvqe",
      noiseMode: nextNoiseMode,
      params: np,
    });
  }

  function selectNoiseMode(next: NoiseMode) {
    const allowed = allowedNoiseModes(
      noiseSuppression,
      kindRef.current,
      paramsRef.current,
    );
    if (!shouldSelectNoiseMode(noiseModeRef.current, next, allowed)) return;
    noiseModeRef.current = next;
    updateEngine({ noiseMode: next });
    applyChangeRef.current({ noiseMode: next });
  }

  function setNoiseParam(key: string, val: unknown) {
    const mode = noiseModeRef.current;
    const next = patchNoiseModeParam(
      noiseParamsByModeRef.current,
      mode,
      key,
      val,
    );
    if (!next) return;
    noiseParamsByModeRef.current = next;
    updateEngine({ noiseParamsByMode: next });
    applyChangeRef.current({ noiseMode: mode });
  }

  function hotNearDelayValue(next: PipelineCfg): number {
    return next.near_delay_ms ?? platformNearDelayDefault(platform);
  }

  // 改管线项。near_delay_ms 可运行中热控;采样率/帧长/参考声道仍需重启。
  function changePipeline(patch: Partial<PipelineCfg>) {
    if (!canChangePipeline(kindRef.current, patch)) return;
    const npl = { ...pipelineRef.current, ...patch };
    pipelineRef.current = npl; // 同步更新 ref:探测后自动恢复引擎时能立刻读到新 near_delay
    updateEngine({ pipeline: npl });
    if (isNearDelayOnlyPatch(patch)) {
      if (powerOnRef.current) {
        if (!hasRunControl("set_near_delay_ms")) {
          reportMissingRunControl("set_near_delay_ms");
          return;
        }
        setNearDelayMs(hotNearDelayValue(npl)).catch((e) =>
          noteError(String(e)),
        );
      }
      return;
    }
    applyChangeRef.current({ pipeline: npl });
  }

  // 输出音量(滚轮 0-100):落进 pipeline(下次 start 用);运行中走 stdin 实时控制,
  // 逐 buffer 生效、零掉音(不 applyChange —— 那会 stop+start 抖音频)。
  function changeOutVolume(v: number) {
    const npl = { ...pipelineRef.current, output_level: v };
    pipelineRef.current = npl;
    updateEngine({ pipeline: npl });
    if (powerOnRef.current) {
      if (!hasRunControl("set_output_level")) {
        reportMissingRunControl("set_output_level");
        return;
      }
      setOutputLevel(v).catch((e) => noteError(String(e)));
    }
  }

  // 引擎配置持久化:kind/pipeline/params 任一变更即写回(paramsByKind 随写,
  // 切引擎再切回、重启 app 都不丢)。
  useEffect(() => {
    paramsByKind.current[kind] = params;
    try {
      localStorage.setItem(
        ENGINE_STATE_KEY,
        JSON.stringify({
          kind,
          noiseMode,
          noiseParamsByMode,
          pipeline,
          paramsByKind: paramsByKind.current,
        }),
      );
    } catch {
      /* 持久化失败不阻塞 */
    }
  }, [kind, noiseMode, noiseParamsByMode, pipeline, params]);

  return {
    engineState,
    updateEngine,
    noiseModeRef,
    noiseParamsByModeRef,
    pipelineRef,
    paramsRef,
    paramsByKind,
    engineReady,
    changeKind,
    setParam,
    selectNoiseMode,
    setNoiseParam,
    pickLocalvqeModel,
    changePipeline,
    changeOutVolume,
  };
}

type RunLifecycleDeps = {
  busy: boolean;
  powerOn: boolean;
  bypassed: boolean;
  bypassPending: boolean | null;
  rec: boolean;
  diagSeconds: number | null;
  selInput: string;
  selOutput: string;
  reference: string;
  kind: string;
  engineReady: (kind: string) => boolean;
  noiseModeRef: MutableRefObject<NoiseMode>;
  noiseParamsByModeRef: MutableRefObject<
    Partial<Record<NoiseMode, Record<string, unknown>>>
  >;
  pipelineRef: MutableRefObject<PipelineCfg>;
  paramsRef: MutableRefObject<Record<string, unknown>>;
  telRef: MutableRefObject<Telemetry>;
  cliVersionRef: MutableRefObject<string | null>;
  runControlsRef: MutableRefObject<Set<string> | null>;
  powerOnRef: MutableRefObject<boolean>;
  applyChangeRef: MutableRefObject<(next: Override) => void>;
  stopForEngineSetupRef: MutableRefObject<() => void>;
  updateApp: Dispatch<Patch<AppState>>;
  noteError: (err: string | null) => void;
  gotoView: (view: View) => void;
  hasRunControl: (cmd: string) => boolean;
  reportMissingRunControl: (cmd: string) => void;
  onRunReservedRef: MutableRefObject<(runId: number) => void>;
  onRunStartedRef: MutableRefObject<(runId: number) => void>;
};

function useRunLifecycle({
  busy,
  powerOn,
  bypassed,
  bypassPending,
  rec,
  diagSeconds,
  selInput,
  selOutput,
  reference,
  kind,
  engineReady,
  noiseModeRef,
  noiseParamsByModeRef,
  pipelineRef,
  paramsRef,
  telRef,
  cliVersionRef,
  runControlsRef,
  powerOnRef,
  applyChangeRef,
  stopForEngineSetupRef,
  updateApp,
  noteError,
  gotoView,
  hasRunControl,
  reportMissingRunControl,
  onRunReservedRef,
  onRunStartedRef,
}: RunLifecycleDeps) {
  const [runtimeCleanupReady, setRuntimeCleanupReady] = useState(false);
  // 当前 run 实际生效的参考源(由 started 给出),供 status 判断是否 Process Tap。
  const refSourceRef = useRef<string | null>(null);
  const runGenerationRef = useRef<RunGeneration>(INITIAL_RUN_GENERATION);
  // 子进程最近一条 stderr 日志(用于在非预期退出时报错)。
  const lastLogRef = useRef<string>("");
  const probeBorrowedRunRef = useRef(false);
  const engineSetupStopPendingRef = useRef(false);
  const runIntentRef = useRef(createRunIntentGuard(powerOn));
  const streamRecoveryRef = useRef<StreamRecoveryState>(
    INITIAL_STREAM_RECOVERY,
  );
  const streamRecoveryTimerRef = useRef<number | null>(null);
  const restartRunRef = useRef<() => Promise<number | null>>(async () => null);
  const bypassTargetRef = useRef(bypassPending ?? bypassed);
  bypassTargetRef.current = bypassPending ?? bypassed;
  if (!powerOn) engineSetupStopPendingRef.current = false;
  const recRef = useRef(rec);
  recRef.current = rec;
  const diagSecondsRef = useRef(diagSeconds);
  diagSecondsRef.current = diagSeconds;

  const clearStreamRecoveryTimer = useCallback(() => {
    if (streamRecoveryTimerRef.current == null) return;
    window.clearTimeout(streamRecoveryTimerRef.current);
    streamRecoveryTimerRef.current = null;
  }, []);

  const resetStreamRecovery = useCallback(() => {
    clearStreamRecoveryTimer();
    streamRecoveryRef.current = INITIAL_STREAM_RECOVERY;
  }, [clearStreamRecoveryTimer]);

  // 录制就地起停命令(运行中改录制态用 stdin,不重启 run)。
  const startDiag = useCallback(() => {
    if (!hasRunControl("start_diagnostics")) {
      reportMissingRunControl("start_diagnostics");
      return;
    }
    startDiagnostics(diagSecondsRef.current).catch((e) => noteError(String(e)));
  }, [hasRunControl, noteError, reportMissingRunControl]);

  useEffect(() => {
    // 清理可能残留的 sidecar(前端 reload 后 Rust 子进程可能还活着 → 状态脱同步)。
    let active = true;
    void ensureStartupRuntimeCleanup().then(() => {
      if (active) setRuntimeCleanupReady(true);
    });
    const listeners = createAsyncListenerScope();
    listeners.listen(onRunEvent, (ev) => {
          const decision = acceptRunEvent(runGenerationRef.current, ev);
          runGenerationRef.current = decision.generation;
          if (!decision.accepted) return;
          if (ev.type === "started") {
            onRunStartedRef.current(ev.run_id);
            telRef.current.on = true;
            cliVersionRef.current = ev.cli_version ?? null;
            runControlsRef.current = Array.isArray(ev.supported_controls)
              ? new Set(ev.supported_controls)
              : null;
            const missingControls = REQUIRED_RUN_CONTROLS.filter(
              (cmd) => !hasRunControl(cmd),
            );
            if (missingControls.length > 0) {
              noteError(
                `CLI ${cliVersionRef.current ?? "unknown"} is missing runtime controls: ${missingControls.join(", ")}. Rebuild or replace the bundled echoless CLI.`,
              );
            }
            refSourceRef.current = ev.reference_source ?? null;
            updateApp({
              io: {
                mic: Boolean(ev.io_resampling?.mic),
                micRate: ev.mic_device_sample_rate ?? null,
              },
            });
            // run 已起;若录制开关为开,就地下发 start_diagnostics(power-on-with-rec /
            // 改设置重启 后的统一入口)。session 目录随后由 diagnostics_started 给出。
            if (recRef.current) startDiag();
            return;
          }
          // 录制已就地启动:拿到 session 目录。
          if (ev.type === "diagnostics_started") {
            setDiagnosticsSessionDir(ev.session_dir);
            return;
          }
          if (ev.type === "diagnostics_stopping") {
            return; // 等 diagnostics_done 收尾
          }
          if (ev.type === "control_error") {
            if (ev.cmd === "set_bypass") {
              updateApp((state) => ({
                ...state,
                ...clearBypassPending(state),
              }));
            }
            noteError(controlErrorMessage(ev));
            return;
          }
          if (ev.type === "stream_error") {
            const message = streamErrorMessage(ev);
            lastLogRef.current = message;
            if (!ev.fatal) {
              noteError(message);
              return;
            }
            if (ev.recoverable && runIntentRef.current.wantsRun()) {
              const next = requestStreamRecovery(
                streamRecoveryRef.current,
                Date.now(),
              );
              streamRecoveryRef.current = next;
              if (next.pendingDelayMs != null) {
                telRef.current.on = false;
                resetRuntimeLive();
                updateApp({ busy: true, io: null, err: null });
                return;
              }
            }
            resetStreamRecovery();
            runIntentRef.current.request(false);
            powerOnRef.current = false;
            telRef.current.on = false;
            resetRuntimeLive();
            updateApp({
              powerOn: false,
              bypassed: false,
              bypassPending: null,
              io: null,
              err: message,
            });
            return;
          }
          // 实时音量变更回执:值由前端驱动,无需处理(否则会被当成 status 读到一堆 undefined,
          // 让 MIC/REF/OUT 表瞬间跳成「—」)。
          if (ev.type === "output_level_changed") {
            return;
          }
          // 实时 near_delay 变更回执:值由前端驱动,下一条 status 会带同样读数。
          if (ev.type === "near_delay_changed") {
            return;
          }
          if (ev.type === "initial_delay_changed") {
            return;
          }
          if (ev.type === "aec3_agc_changed") {
            return;
          }
          if (ev.type === "localvqe_noise_gate_changed") {
            return;
          }
          // 穿透回执:pending 只由后端回执/status 落定,避免写入竞态把假状态钉死。
          if (ev.type === "bypass_changed") {
            updateApp((state) => ({
              ...state,
              ...settleBypassObservation(state, ev.bypassed),
            }));
            return;
          }
          // 诊断录制收尾:writer 已 finalize 文件。「录满 max_seconds」和
          // 「手动关录制 stopped」都打开会话目录(录完 = 想看文件);
          // run_exit / error 不弹(停机/出错时弹窗打扰)。
          if (ev.type === "diagnostics_done") {
            if (ev.reason === "max_seconds") {
              recRef.current = false;
              updateApp({ rec: false });
            }
            if (
              (ev.reason === "max_seconds" || ev.reason === "stopped") &&
              ev.session_dir
            ) {
              openPath(ev.session_dir).catch(() => {});
            }
            return;
          }
          // status —— 白名单判定,绝不兜底。此前这里是「黑名单排除已知事件、
          // 剩余全当 status」:后端每加一种新事件(实锤案例:clock_skew_warning
          // 走同一条 echoless://status 通道)就会掉进兜底、被当 status 读出一堆
          // undefined,灌进遥测后在 dash 的 toFixed 上炸掉整棵 React 树(黑屏)。
          if (ev.type !== "status") return;
          const s = ev;
          // Process Tap 参考收到真实信号 = 系统音频录制权限确已授予 →
          // 把 doctor 的 system_audio_permission 修正为 granted,清掉「请求权限」提示。
          if (
            refSourceRef.current === "macos_process_tap" &&
            (s.ref_dbfs ?? -120) > -90
          ) {
            updateApp((state) =>
              state.doctor && state.doctor.system_audio_permission !== "granted"
                ? {
                    ...state,
                    doctor: {
                      ...state.doctor,
                      system_audio_permission: "granted",
                    },
                  }
                : state,
            );
          }
          // status 常驻 bypassed 字段:兜底同步(如回执丢失/前端重载)。
          if (typeof s.bypassed === "boolean") {
            const sb = s.bypassed;
            updateApp((state) => ({
              ...state,
              ...settleBypassObservation(state, sb),
            }));
          }
          const tel = telRef.current;
          tel.mic = s.mic_dbfs;
          tel.ref = s.ref_dbfs;
          tel.out = s.out_dbfs;
          tel.on = true;
          tel.micWave = s.mic_wave;
          tel.refWave = s.ref_wave;
          tel.outWave = s.out_wave;
          publishRuntimeStatus(s);
    });
    listeners.listen(onRunExit, (ev) => {
          const decision = acceptRunExit(runGenerationRef.current, ev);
          runGenerationRef.current = decision.generation;
          if (!decision.accepted) return;
          telRef.current.on = false;
          resetRuntimeLive(); // 清掉停机后残留的 dBFS / 延迟读数
          updateApp({ io: null, bypassed: false, bypassPending: null });
          refSourceRef.current = null;
          cliVersionRef.current = null;
          runControlsRef.current = null;
          // 后端按子进程标记:intentional=主动停/重启 → 正常,不报错。
          if (ev.intentional) return;
          if (
            ev.recoverable &&
            runIntentRef.current.wantsRun() &&
            streamRecoveryRef.current.pendingDelayMs == null
          ) {
            streamRecoveryRef.current = requestStreamRecovery(
              streamRecoveryRef.current,
              Date.now(),
            );
          }
          const recovery = consumeStreamRecovery(streamRecoveryRef.current);
          streamRecoveryRef.current = recovery.state;
          if (
            recovery.delayMs != null &&
            runIntentRef.current.wantsRun()
          ) {
            updateApp({ busy: true, io: null, err: null });
            clearStreamRecoveryTimer();
            streamRecoveryTimerRef.current = window.setTimeout(() => {
              streamRecoveryTimerRef.current = null;
              if (!runIntentRef.current.wantsRun()) return;
              void restartRunRef.current();
            }, recovery.delayMs);
            return;
          }
          // 非预期退出(子进程自己挂了,如设备不支持采样率)→ 如实反映失败 + 报错。
          // 稍等让 stderr 末行(真正的错误原因)到达,再显示。
          if (runIntentRef.current.wantsRun()) {
            const exitIntent = runIntentRef.current.request(false);
            powerOnRef.current = false;
            window.setTimeout(() => {
              if (runIntentRef.current.snapshot() !== exitIntent) return;
              updateApp({
                powerOn: false,
                err: lastLogRef.current || "运行已停止:子进程意外退出",
              });
            }, 150);
          }
    });
    listeners.listen(onRunLog, (line) => {
      if (line.trim()) lastLogRef.current = line;
    });
    return () => {
      active = false;
      clearStreamRecoveryTimer();
      listeners.dispose();
    };
  }, [
    clearStreamRecoveryTimer,
    hasRunControl,
    noteError,
    resetStreamRecovery,
    startDiag,
    updateApp,
  ]);

  function currentToml(over?: Override, bypass = false) {
    const selectedNoiseMode = over?.noiseMode ?? noiseModeRef.current;
    return buildConfigToml({
      mic: over?.mic ?? selInput,
      output: over?.output ?? selOutput,
      reference: over?.reference ?? reference,
      kind: over?.kind ?? kind,
      noiseMode: selectedNoiseMode,
      noiseParams: noiseParamsByModeRef.current[selectedNoiseMode] ?? {},
      pipeline: over?.pipeline ?? pipelineRef.current,
      params: over?.params ?? paramsRef.current,
      bypass,
      // 录制改由 stdin 就地控制(start/stop_diagnostics),不再写进 toml。
      diagnostics: null,
    });
  }

  async function start(recovering = false): Promise<number | null> {
    if (!recovering) resetStreamRecovery();
    const startIntent = runIntentRef.current.request(true);
    const restartBypassed = recovering ? bypassTargetRef.current : false;
    powerOnRef.current = true;
    updateApp({ busy: true, err: null });
    resetRuntimeHealth();
    resetRuntimeLive();
    lastLogRef.current = ""; // 清掉上次的 stderr,避免旧错误误报
    try {
      const toml = currentToml(undefined, restartBypassed);
      const v = await validateConfig(toml);
      if (!v.ok) {
        if (runIntentRef.current.allowsStart(startIntent)) {
          runIntentRef.current.request(false);
          powerOnRef.current = false;
        }
        updateApp({
          err: v.errors.map((e) => `${e.path}: ${e.message}`).join("; "),
          powerOn: false,
          busy: false,
        });
        return null;
      }
      if (!runIntentRef.current.allowsStart(startIntent)) return null;
      telRef.current.on = true;
      const runId = await startRun(toml, 80);
      onRunReservedRef.current(runId);
      if (!runIntentRef.current.allowsStart(startIntent)) {
        telRef.current.on = false;
        await stopRun().catch(() => {});
        return null;
      }
      runGenerationRef.current = observeRunStart(
        runGenerationRef.current,
        runId,
      );
      probeBorrowedRunRef.current = false;
      bypassTargetRef.current = restartBypassed;
      // 用户主动启动始终进入 AEC on;仅结构性重启会保留 bypass。
      updateApp({
        powerOn: true,
        bypassed: restartBypassed,
        bypassPending: null,
      });
      return runId;
    } catch (e) {
      if (runIntentRef.current.allowsStart(startIntent)) {
        runIntentRef.current.request(false);
        powerOnRef.current = false;
        noteError(String(e));
        telRef.current.on = false;
        updateApp({ powerOn: false });
      }
      return null;
    } finally {
      updateApp({ busy: false });
    }
  }

  restartRunRef.current = () => start(true);

  async function stop() {
    resetStreamRecovery();
    runIntentRef.current.request(false);
    powerOnRef.current = false;
    bypassTargetRef.current = false;
    updateApp({ busy: true });
    try {
      await stopRun();
    } catch (e) {
      noteError(String(e));
    } finally {
      telRef.current.on = false;
      updateApp({
        powerOn: false,
        bypassed: false,
        bypassPending: null,
        busy: false,
      });
    }
  }

  stopForEngineSetupRef.current = () => {
    if (!runIntentRef.current.wantsRun() || engineSetupStopPendingRef.current)
      return;
    engineSetupStopPendingRef.current = true;
    void stop();
  };

  // 延迟侦测专用:probe 需独占麦克风/输出 → AdvancedPage 在探测前后调这个停/起引擎。
  // 恢复时走 start(),会用上探测刚写入的 near_delay/initial_delay(refs 已同步)。
  async function setRunForProbe(on: boolean) {
    if (on) {
      if (!probeBorrowedRunRef.current) return;
      probeBorrowedRunRef.current = false;
      await start();
      return;
    }
    probeBorrowedRunRef.current = powerOnRef.current;
    if (probeBorrowedRunRef.current) await stop();
  }

  // P8-D1 语义(用户拍板 2026-07-04):电源 OFF = 穿透,mic 绝不变哑。
  //   sidecar 未跑 → 启动(AEC on);
  //   AEC on     → set_bypass(true):mic 直通,引擎保温,15ms crossfade;
  //   穿透中     → set_bypass(false):瞬时恢复 AEC(零重收敛)。
  // 「完全停机」不是用户级操作(退出应用 = 停);stop() 只服务重启/probe/错误路径。
  async function togglePower() {
    if (busy) return;
    if (powerOn) {
      if (!hasRunControl("set_bypass")) {
        // 旧 CLI 兼容:没有热穿透命令时退回整机停转。
        await stop();
        return;
      }
      const next = bypassToggleTarget({ bypassed, bypassPending });
      if (next == null) return;
      updateApp({ bypassPending: next });
      setBypass(next).catch((e) => {
        updateApp((state) => ({
          ...state,
          ...clearBypassPending(state, next),
        }));
        noteError(String(e));
      });
      return;
    }
    // 引擎未就绪(无模型 / doctor 未过)→ 先去 Engine 配置,避免启动即失败。
    if (!engineReady(kind)) {
      gotoView("engine");
      return;
    }
    // A5 后:tap 采样率由 helper 上报并在后端重采样,系统参考不再要求 48k。
    await start();
  }

  // Applies changes that still require rebuilding the sidecar runtime.
  // Hot controls bypass this path to avoid an audio dropout.
  // 审计 B-04:手动切换与热插拔自动刷新(devicechange → applyChangeRef)可
  // 重叠,交错的 stop→start 可能终态「UI 显示 OFF 但 sidecar 仍在采集」。
  // 串行队列合并 delta、只跑最后一次;run 经 ref 读最新 doApplyChange 避免闭包过期,
  // 电源态在真正执行的时刻判定(排队期间可能已被关掉)。
  const doApplyRef = useRef<(next: Override) => Promise<void>>(async () => {});
  doApplyRef.current = doApplyChange;
  const applyQueueRef = useRef(
    createSerialQueue<Override>((merged) => {
      if (!runIntentRef.current.wantsRun()) return Promise.resolve();
      return doApplyRef.current(merged);
    }),
  );

  function applyChange(next: Override) {
    resetStreamRecovery();
    // 每次新配置都抢占旧重启事务。旧事务即使已经 stop 完成，也不能用陈旧
    // TOML 再启动；串行队列会把后续 delta 合并后交给最新 generation。
    if (runIntentRef.current.wantsRun()) runIntentRef.current.request(true);
    applyQueueRef.current.enqueue(next);
  }

  applyChangeRef.current = applyChange;

  async function doApplyChange(next: Override) {
    const applyIntent = runIntentRef.current.snapshot();
    if (!runIntentRef.current.allowsStart(applyIntent)) return;
    const restartBypassed = bypassTargetRef.current;
    updateApp({ busy: true });
    try {
      await stopRun();
      if (!runIntentRef.current.allowsStart(applyIntent)) return;
      const toml = currentToml(next, restartBypassed);
      const v = await validateConfig(toml);
      if (!v.ok) {
        if (runIntentRef.current.allowsStart(applyIntent)) {
          runIntentRef.current.request(false);
          powerOnRef.current = false;
        }
        updateApp({
          err: v.errors.map((e) => `${e.path}: ${e.message}`).join("; "),
          powerOn: false,
          bypassed: false,
          bypassPending: null,
        });
        bypassTargetRef.current = false;
        telRef.current.on = false;
        return;
      }
      if (!runIntentRef.current.allowsStart(applyIntent)) return;
      telRef.current.on = true;
      const runId = await startRun(toml, 80);
      if (!runIntentRef.current.allowsStart(applyIntent)) {
        telRef.current.on = false;
        await stopRun().catch(() => {});
        return;
      }
      runGenerationRef.current = observeRunStart(
        runGenerationRef.current,
        runId,
      );
      bypassTargetRef.current = restartBypassed;
      updateApp({
        powerOn: true,
        bypassed: restartBypassed,
        bypassPending: null,
      });
      noteError(null);
    } catch (e) {
      if (!runIntentRef.current.allowsStart(applyIntent)) return;
      runIntentRef.current.request(false);
      powerOnRef.current = false;
      noteError(String(e));
      telRef.current.on = false;
      bypassTargetRef.current = false;
      updateApp({ powerOn: false, bypassed: false, bypassPending: null });
    } finally {
      updateApp({ busy: false });
    }
  }

  // 诊断录制开关:运行中 → 经 stdin 就地起停(不重启 run);未运行 → 仅置位,
  // 等 run 启动后由 started 处理。
  const setRecording = useCallback((on: boolean) => {
    updateApp({ rec: on });
    recRef.current = on;
    if (!powerOnRef.current) return;
    if (on) startDiag();
    else {
      if (!hasRunControl("stop_diagnostics")) {
        reportMissingRunControl("stop_diagnostics");
        return;
      }
      stopDiagnostics().catch((e) => noteError(String(e)));
    }
  }, [hasRunControl, noteError, reportMissingRunControl, startDiag, updateApp]);

  // 时长:仅更新状态。录制中改动 → 重发 start_diagnostics 让新参数立即生效
  // (后端先收尾旧 session 再开新的)。
  const setRecSeconds = useCallback((v: number | null) => {
    updateApp({ diagSeconds: v });
    diagSecondsRef.current = v;
    if (powerOnRef.current && recRef.current) startDiag();
  }, [startDiag, updateApp]);
  return {
    applyChange,
    start,
    runtimeCleanupReady,
    setRunForProbe,
    togglePower,
    setRecording,
    setRecSeconds,
  };
}

function AppShell() {
  const [appState, updateApp] = useReducer(
    patchReducer<AppState>,
    undefined,
    initAppState,
  );
  // 就绪门:首屏数据+字体就位前整窗隐藏,一次性淡入——消除空壳骨架闪烁与
  // 字标 FOUT(fallback 字体宽度不同 → 点阵字标从窄变宽跳)。
  const [booted, setBooted] = useState(false);
  const [startupMode, setStartupMode] = useState<StartupMode>("unknown");
  const [startupCoreReady, setStartupCoreReady] = useState(false);
  const [nvafxChecked, setNvafxChecked] = useState(false);
  const autoStartAttemptedRef = useRef(false);
  const autostartHadStartedRef = useRef(false);
  const autostartHandshakeRef = useRef(INITIAL_STARTUP_RUN_HANDSHAKE);
  const startupModeRef = useRef(startupMode);
  startupModeRef.current = startupMode;
  const onRunReservedRef = useRef<(runId: number) => void>(() => {});
  const onRunStartedRef = useRef<(runId: number) => void>(() => {});
  const updateAutostartHandshake = (next: typeof INITIAL_STARTUP_RUN_HANDSHAKE) => {
    const wasSettled = autostartHandshakeRef.current.settled;
    autostartHandshakeRef.current = next;
    if (wasSettled || !next.settled) return;
    autostartHadStartedRef.current = true;
    settleStartupLaunch().catch(() => {});
  };
  onRunReservedRef.current = (runId) => {
    if (startupModeRef.current !== "autostart") return;
    updateAutostartHandshake(
      expectStartupRun(autostartHandshakeRef.current, runId),
    );
  };
  onRunStartedRef.current = (runId) => {
    if (startupModeRef.current !== "autostart") return;
    updateAutostartHandshake(
      observeStartupRunStarted(autostartHandshakeRef.current, runId),
    );
  };
  // 独立兜底:无论数据 effect 内部发生什么(异常/promise 不 resolve),
  // 字体就绪即揭幕,最迟 1.2s 硬封顶保证绝不卡在空屏。字体本地 woff2
  // 加载 <200ms,常态是数据 effect 先揭幕(见下),这里只兜底。
  useEffect(() => {
    let cancelled = false;
    const lift = () => {
      if (!cancelled) setBooted(true);
    };
    void settleBootGate(
      document.fonts?.ready,
      new Promise((resolve) => setTimeout(resolve, 1200)),
      lift,
    );
    return () => {
      cancelled = true;
    };
  }, []);
  useEffect(() => {
    getStartupMode()
      .then(setStartupMode)
      .catch(() => setStartupMode("manual"));
  }, []);
  // 窗口以 visible:false 创建;手动启动在首屏就绪后显示,autostart 则留在托盘。
  // booted 有 1.2s 硬封顶;手动启动另有 Rust 5s 显窗兜底。
  useEffect(() => {
    if (shouldRevealWindow(booted, startupMode)) revealMainWindow();
  }, [booted, startupMode]);
  const {
    platform,
    devices,
    processors,
    noiseSuppression,
    powerOn,
    busy,
    err,
    view,
    doctor,
    nvafx,
    nvafxBusy,
    nvafxPct,
    nvafxStage,
    nvafxRecv,
    dev,
    devRtxState,
    devMicState,
    devPlatform,
    io,
    rec,
    bypassed,
    bypassPending,
    diagSeconds,
    diagDir,
  } = appState;

  const telRef = useRef<Telemetry>({ mic: -120, ref: -120, out: -120, on: false });
  const cliVersionRef = useRef<string | null>(null);
  const runControlsRef = useRef<Set<string> | null>(null);
  const powerOnRef = useRef(powerOn);
  const renderedPowerOnRef = useRef(powerOn);
  if (renderedPowerOnRef.current !== powerOn) {
    renderedPowerOnRef.current = powerOn;
    powerOnRef.current = powerOn;
  }
  const applyChangeRef = useRef<(next: Override) => void>(() => {});
  const stopForEngineSetupRef = useRef<() => void>(() => {});
  const doctorRef = useRef(doctor);
  doctorRef.current = doctor;
  const { t } = useI18n();

  const noteError = useCallback((err: string | null) => {
    updateApp({ err });
  }, []);

  const { selection, updateSelection, refreshDevices } = useDeviceEnumeration({
    doctorRef,
    powerOnRef,
    telRef,
    applyChangeRef,
    updateApp,
    noteError,
  });
  const { selInput, selOutput, reference } = selection;

  const gotoView = useCallback((view: View) => {
    updateApp({ view });
  }, []);

  const chooseDevRtxState = useCallback((devRtxState: RtxState) => {
    updateApp({ devRtxState });
  }, []);

  const chooseDevMicState = useCallback((devMicState: MicState) => {
    updateApp({ devMicState });
  }, []);

  const hasRunControl = useCallback((cmd: string): boolean => {
    return runControlsRef.current?.has(cmd) ?? false;
  }, []);

  const reportMissingRunControl = useCallback((cmd: string) => {
    noteError(
      `CLI ${cliVersionRef.current ?? "unknown"} does not support runtime control "${cmd}". Rebuild or replace the bundled echoless CLI.`,
    );
  }, [noteError]);

  const {
    engineState,
    updateEngine,
    noiseModeRef,
    noiseParamsByModeRef,
    pipelineRef,
    paramsRef,
    paramsByKind,
    engineReady,
    changeKind,
    setParam,
    selectNoiseMode,
    setNoiseParam,
    pickLocalvqeModel,
    changePipeline,
    changeOutVolume,
  } = useEngineConfig({
    processors,
    noiseSuppression,
    platform,
    dev,
    nvafx,
    powerOnRef,
    applyChangeRef,
    stopForEngineSetupRef,
    noteError,
    gotoView,
    hasRunControl,
    reportMissingRunControl,
  });
  const { kind, noiseMode, noiseParamsByMode, pipeline, params } = engineState;

  const {
    applyChange,
    start,
    runtimeCleanupReady,
    setRunForProbe,
    togglePower,
    setRecording,
    setRecSeconds,
  } = useRunLifecycle({
    busy,
    powerOn,
    bypassed,
    bypassPending,
    rec,
    diagSeconds,
    selInput,
    selOutput,
    reference,
    kind,
    engineReady,
    noiseModeRef,
    noiseParamsByModeRef,
    pipelineRef,
    paramsRef,
    telRef,
    cliVersionRef,
    runControlsRef,
    powerOnRef,
    applyChangeRef,
    stopForEngineSetupRef,
    updateApp,
    noteError,
    gotoView,
    hasRunControl,
    reportMissingRunControl,
    onRunReservedRef,
    onRunStartedRef,
  });
  const autoStartRunRef = useRef(start);
  autoStartRunRef.current = start;

  useEffect(() => {
    const dataReady = startupDataReady(startupCoreReady, kind, nvafxChecked);
    if (
      !shouldAttemptAutoStart({
        mode: startupMode,
        dataReady,
        cleanupReady: runtimeCleanupReady,
        attempted: autoStartAttemptedRef.current,
        running: powerOn,
      })
    ) {
      return;
    }
    autoStartAttemptedRef.current = true;
    if (!engineReady(kind)) {
      gotoView(kind === "nvidia_afx_aec" ? "rtxsetup" : "engine");
      noteError("Auto Start could not run because the selected engine is not ready.");
      revealMainWindow();
      settleStartupLaunch().catch(() => {});
      return;
    }
    void autoStartRunRef.current().then((runId) => {
      if (runId != null) return;
      revealMainWindow();
      settleStartupLaunch().catch(() => {});
    });
  }, [kind, nvafxChecked, powerOn, runtimeCleanupReady, startupCoreReady, startupMode]);

  useEffect(() => {
    if (
      startupMode === "autostart" &&
      autostartHadStartedRef.current &&
      !powerOn &&
      err
    ) {
      revealMainWindow();
    }
  }, [err, powerOn, startupMode]);

  // 平台 + 设备/处理器枚举 + 设备热插拔
  useEffect(() => {
    const platformReady = getPlatform()
      .then((platform) => updateApp({ platform }))
      .catch(() => {});
    const devicesReady = refreshDevices();
    const processorsReady = listProcessors()
      .then((m) => {
        updateApp({
          processors: m.processors,
          noiseSuppression: m.noise_suppression,
        });
        // manifest defaults 打底 + 持久化参数覆盖:新版本新增参数时老存档不缺键。
        updateEngine((cur) => {
          const proc = m.processors.find((p) => p.kind === cur.kind);
          const mergedParams = {
            ...defaultParams(proc),
            ...(Object.keys(cur.params).length
              ? cur.params
              : (paramsByKind.current[cur.kind] ?? {})),
          };
          const normalizedNoiseMode = normalizeNoiseMode(
            m.noise_suppression,
            cur.kind,
            mergedParams,
            cur.noiseMode,
          );
          const mergedNoiseParamsByMode = {
            ...cur.noiseParamsByMode,
          };
          for (const mode of m.noise_suppression.modes) {
            if (!mode.processor_kind) continue;
            const noiseProcessor = m.processors.find(
              (processor) => processor.kind === mode.processor_kind,
            );
            mergedNoiseParamsByMode[mode.id] = {
              ...defaultParams(noiseProcessor),
              ...(cur.noiseParamsByMode[mode.id] ?? {}),
            };
          }
          paramsRef.current = mergedParams;
          paramsByKind.current[cur.kind] = mergedParams;
          noiseModeRef.current = normalizedNoiseMode;
          noiseParamsByModeRef.current = mergedNoiseParamsByMode;
          return {
            ...cur,
            noiseMode: normalizedNoiseMode,
            noiseParamsByMode: mergedNoiseParamsByMode,
            params: mergedParams,
          };
        });
      })
      .catch((e) => noteError(String(e)));
    // 常态揭幕:首批关键数据(平台/设备/引擎清单)就位即亮屏,通常远早于
    // 上面的 1.2s 兜底。allSettled 不因单路失败而卡;硬封顶由独立 effect 兜底。
    Promise.allSettled([platformReady, devicesReady, processorsReady]).then(
      () => {
        setStartupCoreReady(true);
        setBooted(true);
      },
    );
    doctorAudio()
      .then((doctor) => updateApp({ doctor }))
      .catch(() => {});
    nvafxDoctor()
      .then((nvafx) => updateApp({ nvafx }))
      .catch(() => {})
      .finally(() => setNvafxChecked(true));
    defaultDiagDir()
      .then((diagDir) => updateApp({ diagDir }))
      .catch(() => {});

    // 设备热插拔:三路触发 → 同一个防抖刷新。
    //   ① 原生 CoreAudio 监听(macOS;WKWebView 不触发 devicechange)
    //   ② webview devicechange(Windows WebView2 可靠)
    //   ③ 窗口聚焦 + 下拉展开(兜底)
    // 300ms 防抖合并连发——一次插拔常触发多个事件,每次刷新都 spawn 一次 CLI 枚举。
    let devChangeTimer = 0;
    const refreshDevicesSoon = () => {
      window.clearTimeout(devChangeTimer);
      devChangeTimer = window.setTimeout(refreshDevices, 300);
    };
    navigator.mediaDevices?.addEventListener?.(
      "devicechange",
      refreshDevicesSoon,
    );
    window.addEventListener("focus", refreshDevicesSoon);

    const listeners = createAsyncListenerScope();
    listeners.listen(onDevicesChanged, refreshDevicesSoon);
    return () => {
      window.clearTimeout(devChangeTimer);
      navigator.mediaDevices?.removeEventListener?.(
        "devicechange",
        refreshDevicesSoon,
      );
      window.removeEventListener("focus", refreshDevicesSoon);
      listeners.dispose();
    };
  }, [noteError, refreshDevices]);

  // NVAFX 下载进度:CLI download-install 在 stderr 打的 JSONL,后端转成事件。
  // label 是「common runtime」/「model」两段,归一成 stage 让 UI 分别标注。
  // 默认固定资产使用内置大小显示百分比;自定义 tag 无大小时显示已接收字节。
  useEffect(() => {
    let alive = true;
    const un = onNvafxProgress((p) => {
      if (!alive) return;
      const stage = /model/i.test(p.label) ? "model" : "runtime";
      updateApp({ nvafxPct: p.pct, nvafxStage: stage, nvafxRecv: p.received });
    });
    return () => {
      alive = false;
      un.then((f) => f());
    };
  }, [updateApp]);

  // Esc 始终有意义:在次级页按 Esc 返回 Overview。
  useEffect(() => {
    if (view === "overview") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") gotoView("overview");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [gotoView, view]);

  // 桌面 app:禁用 Tab 键焦点移动(避免按钮出现键盘选中框)。
  useEffect(() => {
    const onTab = (e: KeyboardEvent) => {
      if (e.key === "Tab") e.preventDefault();
    };
    window.addEventListener("keydown", onTab);
    return () => window.removeEventListener("keydown", onTab);
  }, []);

  // 桌面 app:禁用 WebView 默认右键菜单(重新加载/检查元素等网页项)。
  // 输入框/文本域放行,保留右键粘贴;dev 构建放行,保留调试菜单。
  useEffect(() => {
    const onContextMenu = (e: MouseEvent) => {
      if (import.meta.env.DEV) return;
      const el = e.target as HTMLElement | null;
      if (el && /^(INPUT|TEXTAREA)$/.test(el.tagName)) return;
      e.preventDefault();
    };
    window.addEventListener("contextmenu", onContextMenu);
    return () => window.removeEventListener("contextmenu", onContextMenu);
  }, []);

  // 开发态快捷键:按 ~ 切换(在输入框里则正常输入,不触发)。
  // 仅 dev 构建存在:正式包 import.meta.env.DEV=false,快捷键与 dev 模式
  // 一并从产物里消失(?dev=1 直链在 Tauri 里本就没有 query,双保险)。
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const onTilde = (e: KeyboardEvent) => {
      if (e.key !== "~") return;
      const el = document.activeElement;
      if (el && /^(INPUT|TEXTAREA)$/.test(el.tagName)) return;
      e.preventDefault();
      updateApp((state) => ({ ...state, dev: !state.dev }));
    };
    window.addEventListener("keydown", onTilde);
    return () => window.removeEventListener("keydown", onTilde);
  }, []);

  // 开发态下按 `(同一物理键不按 Shift)在 当前平台 / Windows / Linux 间切换。
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const onBacktick = (e: KeyboardEvent) => {
      if (e.key !== "`") return;
      const el = document.activeElement;
      if (el && /^(INPUT|TEXTAREA)$/.test(el.tagName)) return;
      if (!dev) return;
      e.preventDefault();
      updateApp((state) => ({
        ...state,
        devPlatform: cycleDevPlatform(state.devPlatform),
      }));
    };
    window.addEventListener("keydown", onBacktick);
    return () => window.removeEventListener("keydown", onBacktick);
  }, [dev]);

  function recheckNvafx() {
    if (dev) return; // dev 模拟:状态由 dev 切换条控制
    nvafxDoctor()
      .then((nvafx) => updateApp({ nvafx }))
      .catch(() => {});
  }

  // 重跑虚拟声卡检测(MIC SETUP 向导的 recheck)。
  function recheckAudio() {
    doctorAudio()
      .then((doctor) => updateApp({ doctor }))
      .catch(() => {});
  }

  // 用户主动请求系统音频录制权限:helper 显式调 TCCAccessRequest 弹窗,回传更新 doctor。
  // 未授予时不再自动跳设置(用户否决 2026-07-05),把 CLI 的失败原因如实显示。
  const probeSystemAudio = useCallback(() => {
    noteError(null);
    requestSystemAudio()
      .then((doctor) => {
        updateApp({ doctor });
        if (doctor.system_audio_permission !== "granted") {
          const detail = doctor.system_audio_permission_probe?.detail;
          noteError(detail || "system audio permission was not granted");
        }
      })
      .catch((e) => noteError(String(e)));
  }, [noteError]);

  // RTX runtime 安装:解压 common + 架构 model,回传安装后 doctor 报告。
  // dev 模拟:不调后端,延迟后置 ready,以便走通"安装中 → 就绪"。
  function installNvafx(commonZip: string, modelZip: string) {
    if (dev) {
      updateApp({ nvafxBusy: true });
      window.setTimeout(() => {
        updateApp({ devRtxState: "ready", nvafxBusy: false });
      }, 900);
      return;
    }
    updateApp({ nvafxBusy: true, err: null });
    nvafxInstall({ commonZip, modelZip })
      .then((nvafx) => updateApp({ nvafx }))
      .catch((e) => noteError(String(e)))
      .finally(() => updateApp({ nvafxBusy: false }));
  }

  // 从公共 GitHub release 下载并安装(按 GPU 架构自动选模型)。dev 下模拟。
  function downloadInstallNvafx() {
    if (dev) {
      // 模拟真实下载的两段(runtime → model),预览百分比与字节读数。
      const RUNTIME_TOTAL = 955 * 1024 * 1024;
      const MODEL_TOTAL = 46 * 1024 * 1024;
      updateApp({
        nvafxBusy: true,
        nvafxStage: "runtime",
        nvafxPct: 0,
        nvafxRecv: 0,
        err: null,
      });
      let phase = 0;
      let pct = 0;
      const timer = window.setInterval(() => {
        pct += 6;
        if (pct >= 100) {
          phase += 1;
          if (phase >= 2) {
            window.clearInterval(timer);
            updateApp({
              devRtxState: "ready",
              nvafxBusy: false,
              nvafxPct: null,
              nvafxStage: null,
              nvafxRecv: null,
            });
            return;
          }
          pct = 0; // 下一个 asset 从头开始
        }
        if (phase === 0) {
          const p = Math.min(pct, 99);
          updateApp({
            nvafxStage: "runtime",
            nvafxPct: p,
            nvafxRecv: Math.round((p / 100) * RUNTIME_TOTAL),
          });
        } else {
          updateApp({
            nvafxStage: "model",
            nvafxPct: Math.min(pct, 99),
            nvafxRecv: Math.round((Math.min(pct, 99) / 100) * MODEL_TOTAL),
          });
        }
      }, 150);
      return;
    }
    updateApp({
      nvafxBusy: true,
      nvafxPct: null,
      nvafxStage: null,
      nvafxRecv: null,
      err: null,
    });
    nvafxDownloadInstall()
      .then((nvafx) => updateApp({ nvafx }))
      .catch((e) => noteError(String(e)))
      .finally(() =>
        updateApp({
          nvafxBusy: false,
          nvafxPct: null,
          nvafxStage: null,
          nvafxRecv: null,
        }),
      );
  }

  const platformView: Platform = dev && devPlatform ? devPlatform : platform;

  // dev 模拟 Windows 时,系统 render loopback 原生可用 → 注入一个 system 参考源,
  // 让 win 预览忠实(真实 win 上后端本就返回 system available;mac 才退 none)。
  const refSources =
    dev && platformView === "windows"
      ? [
          {
            id: "system",
            label: "System audio",
            kind: "system" as const,
            available: true,
            stable_id: "system",
            selector: "system",
          },
          ...(devices?.reference_sources ?? []).filter((r) => r.id !== "system"),
        ]
      : devices?.reference_sources ?? [];
  // dev win 下默认就选 system;dev linux 没有 system 项,默认退到 none。
  const referenceView =
    dev && platformView === "windows"
      ? "system"
      : dev && platformView === "linux"
        ? "none"
        : reference;
  // reference 概念 = 系统正在播放的声音(输出内容)。只保留有意义的参考源:
  //   system(Process Tap / loopback)、none、output 设备回环、以及承载系统声的虚拟声卡输入
  //   (BlackHole / VB-CABLE)。隐藏物理麦克风等(选它们当参考无意义)。
  const VIRTUAL_REF =
    /blackhole|vb-?cable|vb-?audio|cable|loopback|stereo\s*mix|soundflower|monitor|echoless|null/i;
  // A2:排除自环 —— Echoless 自己的输出设备(及其同名输入侧,如 BlackHole 的 in 口)
  // 作参考会把处理后的输出再喂回来,形成回授;从候选里剔掉。
  const selOutDevName = devices?.outputs.find(
    (d) => d.stable_id === selOutput,
  )?.name;
  const isSelfLoop = (r: (typeof refSources)[number]) =>
    (r.kind === "output" || r.kind === "input") &&
    ((r.selector ?? r.id) === selOutput ||
      r.stable_id === selOutput ||
      (selOutDevName != null && r.label === selOutDevName));
  const availRefs = refSources.filter(
    (r) =>
      r.available &&
      !isSelfLoop(r) &&
      (r.kind === "system" ||
        r.kind === "none" ||
        r.kind === "output" ||
        (r.kind === "input" && VIRTUAL_REF.test(r.label))),
  );
  // 仅当同名设备同时以 input/output 出现(如 BlackHole 既可作 in 又可作 out)才标方向,
  // 避免「全是 · in」的冗余噪声。
  const refLabelKinds = new Map<string, Set<string>>();
  for (const r of availRefs) {
    if (r.kind !== "input" && r.kind !== "output") continue;
    if (!refLabelKinds.has(r.label)) refLabelKinds.set(r.label, new Set());
    refLabelKinds.get(r.label)!.add(r.kind);
  }
  const refOptions = availRefs.map((r) => {
    const ambiguous =
      (r.kind === "input" || r.kind === "output") &&
      (refLabelKinds.get(r.label)?.size ?? 0) > 1;
    return {
      value: r.selector ?? r.id,
      label: ambiguous
        ? `${r.label} · ${r.kind === "input" ? "in" : "out"}`
        : r.label,
    };
  });

  const isMac = platformView === "macos";
  const refSel = dev ? referenceView : reference;
  const compatibleNoiseModes = allowedNoiseModes(
    noiseSuppression,
    kind,
    params,
  );
  const activeLocalvqeCapability =
    kind === "localvqe"
      ? noiseSuppression?.localvqe_models.find(
          (entry) => entry.file === modelFileName(params.model),
        )?.capability
      : null;
  const noiseHint =
    activeLocalvqeCapability === "built_in_ns"
      ? t("noiseBuiltIn")
      : noiseMode === "webrtc"
        ? t("noiseWebrtcHint")
        : noiseMode === "rnnoise"
          ? t("noiseRnnoiseHint")
          : t("noiseOffHint");
  // 通话 app 里要选的"麦克风"名:由所选输出设备名推导(CABLE Input→CABLE Output;其余同名)。
  const outDev = devices?.outputs.find((d) => d.stable_id === selOutput);
  const cableName = outDev
    ? /cable input/i.test(outDev.name)
      ? outDev.name.replace(/input/i, "Output")
      : outDev.name
    : "CABLE Output";
  // footer 规格徽章随 pipeline 实时变。
  const stamp = `${pipeline.reference_channels.toUpperCase()} · ${
    pipeline.sample_rate / 1000
  }K · ${pipeline.frame_ms}MS`;

  const viewTitle =
    view === "overview"
      ? t("overview")
      : view === "engine"
        ? t("engine")
        : view === "rtxsetup"
          ? t("rtxSetup")
          : view === "micsetup"
            ? t("micSetup")
            : view === "advanced"
              ? t("advanced")
              : t("diagnostics");
  // 当前引擎是否就绪(未就绪时 overview 提示去 Engine 配置)。
  const activeReady = engineReady(kind);
  // dev:用模拟 doctor 驱动 Engine 卡片 + RTX 向导,让 mac 也能逐屏走流程。
  const nvafxView = dev ? simNvafxDoctor(devRtxState) : nvafx;
  // dev:同样用模拟 doctor 驱动诊断行 + 虚拟麦向导,逐屏走 missing→ready(随模拟平台切换)。
  const doctorView = dev ? simMicDoctor(devMicState, platformView) : doctor;

  // 系统音频录制权限引导:仅 mac + 用 system(Process Tap)reference 时相关。
  // denied → 可点去隐私设置;undetermined → 提示首次运行会请求(OS 届时弹窗)。
  const sysAudioPerm = doctorView?.system_audio_permission;
  const usingSysRef = isMac && referenceView === "system";
  const sysAudioDenied = usingSysRef && sysAudioPerm === "denied";
  const sysAudioUndet = usingSysRef && sysAudioPerm === "undetermined";
  // UI 电源视觉态 = sidecar 在跑且未穿透。穿透时波形照常流动(mic 活着),
  // 但字标熄灭 + 控制件调暗(AEC 不在工作)。
  const uiOn = powerOn && !bypassed;

  // 运行五态(含 A4 防抖):状态盒 / srail 状态字 / zsub 共用同一判定。
  const statusKind = useRunStatusKind(powerOn, refSel, dev, bypassed);

  // Windows 托盘偏好:持久化 + 每次变更(含首个渲染 = 启动同步)推给 Rust。
  const [trayPrefs, updateTrayPrefs] = useState<TrayPrefsState>(readTrayPrefs);
  useEffect(() => {
    try {
      localStorage.setItem(TRAY_PREFS_KEY, JSON.stringify(trayPrefs));
    } catch {
      /* 持久化失败不阻塞 */
    }
    setTrayPrefs(trayPrefs.closeToTray).catch(() => {});
  }, [trayPrefs]);

  // Windows 启动项以系统注册状态为真理源,不再复制一份 localStorage 偏好。
  // 用户可在任务管理器/系统设置里关闭启动项;窗口重新聚焦时同步实际状态。
  const [autostartPreference, setAutostartPreference] =
    useState<AutostartPreference>({ enabled: null, pending: null });
  const autostartRequestRef = useRef(0);
  const autostartChangePendingRef = useRef(false);
  const refreshAutostart = useCallback(() => {
    if (platform !== "windows" || autostartChangePendingRef.current) return;
    const request = ++autostartRequestRef.current;
    getAutostartEnabled()
      .then((enabled) => {
        if (request !== autostartRequestRef.current) return;
        setAutostartPreference((current) =>
          current.pending == null ? settleAutostart(enabled) : current,
        );
      })
      .catch((e) => {
        if (request === autostartRequestRef.current) noteError(String(e));
      });
  }, [noteError, platform]);
  useEffect(() => {
    if (platform !== "windows") return;
    refreshAutostart();
    window.addEventListener("focus", refreshAutostart);
    return () => window.removeEventListener("focus", refreshAutostart);
  }, [platform, refreshAutostart]);

  function changeAutostart(enabled: boolean) {
    if (dev && platformView === "windows" && platform !== "windows") {
      setAutostartPreference(settleAutostart(enabled));
      return;
    }
    autostartChangePendingRef.current = true;
    const request = ++autostartRequestRef.current;
    setAutostartPreference((current) =>
      beginAutostartChange(current, enabled),
    );
    persistAutostartEnabled(enabled)
      .then((actual) => {
        if (request !== autostartRequestRef.current) return;
        setAutostartPreference(settleAutostart(actual));
      })
      .catch((e) => {
        if (request !== autostartRequestRef.current) return;
        setAutostartPreference((current) => rejectAutostartChange(current));
        noteError(String(e));
      })
      .finally(() => {
        if (request === autostartRequestRef.current) {
          autostartChangePendingRef.current = false;
        }
      });
  }

  // zmeta 版本号(tauri.conf.json 为源)。
  const [appVersion, setAppVersion] = useState("");
  useEffect(() => {
    getVersion()
      .then(setAppVersion)
      .catch(() => {});
  }, []);

  // v14/v17:字标随电源亮灭 —— 熄→亮播 crton(磷光渐暖),亮→熄播 crtoff(衰减)。
  // render 期 prev 比较(不走 useEffect):首帧不播动画,切换帧同 commit 内定类名。
  const [wordAnim, setWordAnim] = useState("");
  const prevPowerRef = useRef(uiOn);
  if (prevPowerRef.current !== uiOn) {
    prevPowerRef.current = uiOn;
    setWordAnim(uiOn ? "igniting" : "dying");
  }

  return (
    <div
      className={`window ${isMac ? "mac" : "win"} ${uiOn ? "" : "sysoff"} ${booted ? "" : "booting"}`}
    >
      {/* ---- titlebar ---- */}
      <header className="tbar" data-tauri-drag-region>
        <AppIcon />
        <span className="screen">
          <ScrambleText text={viewTitle} />
        </span>
        <span className="hatch" />
        {dev && (
          <span className="devtag">
            DEV · {platformTag(platformView)}
          </span>
        )}
        <span className="uid">{modelName(kind)}</span>
        {!isMac && (
          <span className="caption">
            <button type="button" className="cbtn" onClick={() => appWindow.minimize()}>
              <CapMin />
            </button>
            <button type="button" className="cbtn" onClick={() => appWindow.toggleMaximize()}>
              <CapMax />
            </button>
            <button type="button" className="cbtn close" onClick={() => appWindow.close()}>
              <CapClose />
            </button>
          </span>
        )}
      </header>

      {/* ---- content ---- */}
      <main className="content">
        {view === "overview" && (
        // v12:铭牌分格 plate —— A 铭牌 / B 电源格 / C 信号链 / D 仪器区
        <div className="plate">
        <section className="zone za">
          <div className="kick">
            <span className="d">
              <i />
              <i />
              <i />
            </span>{" "}
            {t("kicker")}
          </div>
          {/* v6.1/v14:半调点阵字标,随电源亮灭 */}
          <div className="word">
            <span className={`wtxt ${uiOn ? "lit" : ""} ${wordAnim}`}>
              ECHOLESS
            </span>
          </div>
          {/* v14:zmeta = 真实运行参数(引擎/管线/采集后端/版本) */}
          <div className="zmeta">
            ENGINE{" "}
            <b>
              <ScrambleText text={modelName(kind)} />
            </b>{" "}
            · {pipeline.sample_rate / 1000} KHZ / {pipeline.frame_ms} MS BLOCK
            · I/O{" "}
            <b>
              <ScrambleText text={ioBackendLabel(platformView)} />
            </b>
            {appVersion ? ` · ECHOLESS V${appVersion}` : ""}
          </div>
        </section>

        <section className="zone zb">
          <div className="zhead">Power</div>
          <SlideSwitch
            on={uiOn}
            onToggle={togglePower}
            disabled={busy || bypassPending != null}
          />
          <RuntimeStatusStrip statusKind={statusKind} />
          <RuntimeSubline
            statusKind={statusKind}
            activeReady={activeReady}
            sysAudioDenied={sysAudioDenied}
            sysAudioUndet={sysAudioUndet}
            onEngineSetup={() => gotoView("engine")}
            onProbeSystemAudio={probeSystemAudio}
            onCheckSetup={() => gotoView("diagnostics")}
          />
        </section>

        {/* ---- C 信号链:01-04 站点 ---- */}
        <section className="zone zc">
          <div className="station">
            <span className="stnum">01</span>
            <span className="stkey">{t("input")}</span>
            <span className="co">:</span>
            <span className="v">
              <Dropdown
                value={selInput}
                onOpen={refreshDevices}
                options={(devices?.inputs ?? []).map((d) => ({
                  value: d.stable_id,
                  label: d.name,
                }))}
                onChange={(v) => {
                  updateSelection({ selInput: v });
                  applyChange({ mic: v });
                }}
              />
            </span>
            <span className="sp" />
            <span className="meta">
              {t("micNearEnd")}
              {io?.mic && io.micRate ? (
                <span className="rsmp">
                  {" "}
                  · {io.micRate / 1000}k→{pipeline.sample_rate / 1000}k
                </span>
              ) : null}
            </span>
            <span className="ico">
              <IcoInput />
            </span>
          </div>

          <div className="station">
            <span className="stnum">02</span>
            <span className="stkey">{t("model")}</span>
            <span className="co">:</span>
            <div className="segg" id="models">
              {MODELS.map((m) => {
                const proc = processors.find((p) => p.kind === m.kind);
                const supported =
                  !proc || proc.platforms.includes(platform) || dev;
                const rdy = engineReady(m.kind);
                const active = kind === m.kind;
                return (
                  <button
                    type="button"
                    key={m.kind}
                    className={`b ${active ? "active" : ""} ${
                      supported && !rdy ? "unready" : ""
                    }`}
                    disabled={busy || !supported || active}
                    onClick={() => changeKind(m.kind)}
                  >
                    {m.label}
                  </button>
                );
              })}
            </div>
            <span className="sp" />
            <span className="meta">
              {t("reference")}{" "}
              <Dropdown
                compact
                align="right"
                warn={referenceView === "none"}
                value={referenceView}
                onOpen={refreshDevices}
                options={refOptions}
                onChange={(v) => {
                  updateSelection({ reference: v });
                  applyChange({ reference: v });
                }}
              />
            </span>
            <span className="ico">
              <IcoModel />
            </span>
          </div>

          <div className="station">
            <span className="stnum">03</span>
            <span className="stkey">{t("output")}</span>
            <span className="co">:</span>
            <span className="v">
              <Dropdown
                value={selOutput}
                onOpen={refreshDevices}
                options={(devices?.outputs ?? []).map((d) => ({
                  value: d.stable_id,
                  label: d.name,
                }))}
                onChange={(v) => {
                  updateSelection({ selOutput: v });
                  applyChange({ output: v });
                }}
              />
            </span>
            <span className="sp" />
            {doctor && !doctor.virtual_output_detected ? (
              <span className="meta" style={{ color: "var(--warn)" }}>
                <span className="mk">!!!</span> {t("installCable")}:{" "}
                <b>{doctor.recommended_driver}</b>
              </span>
            ) : (
              <span className="meta">
                <span className="mk">&gt;&gt;&gt;</span>{" "}
                {t("inAppPickMic").replace("{name}", cableName)}
              </span>
            )}
            <span className="ico">
              <IcoOutput />
            </span>
          </div>

          <div className="station">
            <span className="stnum">04</span>
            <span className="stkey">{t("noise")}</span>
            <span className="co">:</span>
            <div className="segg" id="ns">
              {(
                [
                  ["webrtc", "WEBRTC"],
                  ["rnnoise", "RNNOISE"],
                  ["off", "OFF"],
                ] as const
              ).map(([mode, label]) => {
                const active = noiseMode === mode;
                const compatible = compatibleNoiseModes.includes(mode);
                return (
                  <button
                    type="button"
                    key={mode}
                    className={`b ${mode === "off" ? "" : "long-label"} ${
                      active ? "active" : ""
                    }`}
                    disabled={busy || active || !compatible}
                    onClick={() => selectNoiseMode(mode)}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
            <span className="sp" />
            <span className="meta">{noiseHint}</span>
            <span className="ico">
              <IcoNoise />
            </span>
          </div>
        </section>

        {/* ---- D 仪器区 ---- */}
        <section className="zone zd">
          {/* 局部隔离墙:遥测面板是高频刷新、最易受后端异常数据影响的地方。
              包一层边界 → 即便它渲染出错,也只降级本面板,主控制/引擎照常可用。 */}
          <ErrorBoundary
            label="signal-panel"
            fallback={<div className="sig" style={{ opacity: 0.35 }} />}
          >
            <RuntimeSignalPanel
              telRef={telRef}
              powerOn={powerOn}
              statusKind={statusKind}
            />
          </ErrorBoundary>
        </section>
        </div>
        )}
        {view === "engine" && (
          <EnginePage
            processors={processors}
            noiseSuppression={noiseSuppression}
            platform={platformView}
            kind={kind}
            doctor={nvafxView}
            dev={dev}
            onSelect={changeKind}
            onPickModel={pickLocalvqeModel}
            localvqeModel={
              (kind === "localvqe"
                ? (params.model as string | undefined)
                : (paramsByKind.current["localvqe"]?.model as
                    | string
                    | undefined)) ?? null
            }
            onRecheck={recheckNvafx}
            onSetup={() => gotoView("rtxsetup")}
          />
        )}
        {view === "rtxsetup" && (
          <RtxSetupPage
            doctor={nvafxView}
            busy={nvafxBusy}
            pct={nvafxPct}
            stage={nvafxStage}
            recv={nvafxRecv}
            dev={dev}
            devState={devRtxState}
            onDevState={chooseDevRtxState}
            onRecheck={recheckNvafx}
            onInstall={installNvafx}
            onDownloadInstall={downloadInstallNvafx}
            onUse={() => {
              changeKind("nvidia_afx_aec");
              gotoView("overview");
            }}
          />
        )}
        <div className="persistent-view" hidden={view !== "advanced"}>
          <AdvancedPage
            processors={processors}
            noiseSuppression={noiseSuppression}
            kind={kind}
            noiseMode={noiseMode}
            noiseParams={noiseParamsByMode[noiseMode] ?? {}}
            pipeline={pipeline}
            params={params}
            onPipeline={changePipeline}
            onParam={setParam}
            onNoiseParam={setNoiseParam}
            platform={platformView}
            mic={selInput}
            reference={reference}
            output={selOutput}
            running={powerOn}
            onSetRun={setRunForProbe}
            visible={view === "advanced"}
            trayPrefs={trayPrefs}
            onTrayPrefs={(patch) =>
              updateTrayPrefs((cur) => ({ ...cur, ...patch }))
            }
            autoStartEnabled={displayAutostartEnabled(autostartPreference)}
            autoStartBusy={
              !(dev && platformView === "windows" && platform !== "windows") &&
              (autostartPreference.enabled == null ||
                autostartPreference.pending != null)
            }
            onAutoStart={changeAutostart}
          />
        </div>
        {view === "diagnostics" && (
          <RuntimeDiagnosticsPage
            rec={rec}
            seconds={diagSeconds}
            diagDir={diagDir}
            running={powerOn}
            doctor={doctorView}
            platform={platformView}
            onMicSetup={() => gotoView("micsetup")}
            onRequestSystemAudio={probeSystemAudio}
            onRecheck={recheckAudio}
            onRec={setRecording}
            onSeconds={setRecSeconds}
          />
        )}
        {view === "micsetup" && (
          <MicSetupPage
            doctor={doctorView}
            platform={platformView}
            dev={dev}
            devState={devMicState}
            onDevState={chooseDevMicState}
            onRecheck={recheckAudio}
          />
        )}
      </main>

      {/* ---- footer ---- */}
      <footer className="fbar">
        {view === "overview" ? (
          <>
            <button
              type="button"
              className="link"
              onClick={() => gotoView("engine")}
            >
              {t("engine")} <span className="mk">&gt;&gt;&gt;</span>
            </button>
            <button
              type="button"
              className="link"
              onClick={() => gotoView("advanced")}
            >
              {t("advanced")} <span className="mk">&gt;&gt;&gt;</span>
            </button>
            <button
              type="button"
              className="link"
              onClick={() => gotoView("diagnostics")}
            >
              {t("diagnostics")} <span className="mk">&gt;&gt;&gt;</span>
            </button>
          </>
        ) : (
          <button
            type="button"
            className="link"
            onClick={() =>
              gotoView(
                view === "rtxsetup"
                  ? "engine"
                  : view === "micsetup"
                    ? "diagnostics"
                    : "overview",
              )
            }
          >
            <span className="mk">&lt;&lt;&lt;</span>{" "}
            {view === "rtxsetup"
              ? t("engine")
              : view === "micsetup"
                ? t("diagnostics")
                : t("backToOverview")}
          </button>
        )}
        <span className="sp" />
        <span className="fright">
          <VolumeWheel
            volume={pipeline.output_level ?? 50}
            onChange={changeOutVolume}
            invertWheel={isMac}
          />
          {err ? (
            <Hint text={`${err} · 点击关闭`} pos="top" attach>
              <button
                type="button"
                className="stamp err plainbtn"
                onClick={() => noteError(null)}
              >
                {err.length > 44 ? err.slice(0, 44) + "…" : err}{" "}
                <span className="mk">✕</span>
              </button>
            </Hint>
          ) : (
            <>
              <span className="fdot">·</span>
              <span className="stamp">{stamp}</span>
              <span className="fdot">·</span>
              {/* v3:UPTIME 走表(动效只证明系统活着) */}
              <UptimeStamp powerOn={powerOn} />
            </>
          )}
        </span>
        <RuntimeFooterBars telRef={telRef} powerOn={powerOn} />
      </footer>

      {/* v10 动态底噪(WebGL shader,见 TvNoise 注释);OFF 时随 sysoff 渐隐停走 */}
      <TvNoise active={uiOn} />
      {/* v6 VHS 亮带(transform 合成器动画);OFF 时随 sysoff 渐隐暂停 */}
      <div className="vhs" aria-hidden="true">
        <i className="band" />
        <i className="line" />
      </div>
    </div>
  );
}

// 动态底噪 v4:WebGL 逐项复刻 feTurbulence(type=turbulence, baseFrequency=1.15,
// numOctaves=1),每帧一次 draw call,CPU 归零(原版 SMIL 滤镜 = 320% CPU)。
// 复刻要点(三轮反馈的最终结论):
//   · value noise(晶格 + smoothstep 插值)= 平滑连续场 —— 质感的关键;
//     逐像素独立 hash 是椒盐白噪,又硬又脏;
//   · |2n-1| 折叠 = turbulence 的 |noise|,分布偏 0,白底上大多近乎透明;
//   · px uniform = baseFrequency/dpr,晶格按 CSS 像素网格(与 SVG 滤镜一致)。
const NOISE_FS = `precision highp float;
uniform float t;
uniform float px;
float h(vec2 p) {
  return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123);
}
float vn(vec2 p, float s) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  return mix(
    mix(h(i + s), h(i + s + vec2(1.0, 0.0)), u.x),
    mix(h(i + s + vec2(0.0, 1.0)), h(i + s + vec2(1.0, 1.0)), u.x),
    u.y
  );
}
float tb(vec2 p, float s) {
  return abs(vn(p, s) * 2.0 - 1.0);
}
void main() {
  vec2 p = gl_FragCoord.xy * px;
  vec3 rgb = vec3(tb(p, t), tb(p, t + 17.0), tb(p, t + 41.0));
  float a = tb(p, t + 89.0);
  // alpha 恒等折叠:multiply 下 (rgb, a) 与 (mix(1,rgb,a), 1) 逐像素等价。
  // 输出全不透明,绕开 WKWebView 对 premultipliedAlpha:false 画布的
  // unpremultiply 溢出(a→0 像素爆成高饱和彩点 = 椒盐,用户三轮反馈根源)。
  gl_FragColor = vec4(mix(vec3(1.0), rgb, a), 1.0);
}`;
const NOISE_VS = `attribute vec2 a;
void main() { gl_Position = vec4(a, 0.0, 1.0); }`;

function TvNoise({ active }: { active: boolean }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const activeRef = useRef(active);
  activeRef.current = active;
  const scheduleRef = useRef<() => void>(() => {});
  useEffect(() => {
    const canvas = ref.current;
    const wrap = wrapRef.current;
    if (!canvas) return;
    const gl = canvas.getContext("webgl", {
      alpha: false, // 输出全不透明(alpha 已折进颜色),跨引擎合成零歧义
      antialias: false,
      depth: false,
      stencil: false,
      powerPreference: "low-power",
      // 不设 preserveDrawingBuffer:不读回像素,省驱动保留后缓冲的开销,
      // 也减轻 GPU 压力下 WebView2/WKWebView 主动丢弃 context 的概率。
    });
    if (!gl) return; // 无 WebGL:静默无噪点(氛围件,不值得再养 fallback)

    let tLoc: WebGLUniformLocation | null = null;
    let pxLoc: WebGLUniformLocation | null = null;
    let ready = false;
    let forceFit = false;

    // GL 资源建立:首次挂载 + context 恢复后都走这里重建 program/buffer。
    const initGL = (): boolean => {
      const compile = (type: number, src: string) => {
        const s = gl.createShader(type)!;
        gl.shaderSource(s, src);
        gl.compileShader(s);
        return s;
      };
      const prog = gl.createProgram()!;
      gl.attachShader(prog, compile(gl.VERTEX_SHADER, NOISE_VS));
      gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, NOISE_FS));
      gl.linkProgram(prog);
      if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return false;
      gl.useProgram(prog);
      // 全屏三角形
      const buf = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, buf);
      gl.bufferData(
        gl.ARRAY_BUFFER,
        new Float32Array([-1, -1, 3, -1, -1, 3]),
        gl.STATIC_DRAW,
      );
      const loc = gl.getAttribLocation(prog, "a");
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
      tLoc = gl.getUniformLocation(prog, "t");
      pxLoc = gl.getUniformLocation(prog, "px");
      forceFit = true; // 恢复后 viewport 需重设,即便画布尺寸未变
      return true;
    };
    ready = initGL();

    // 画布按设备物理像素渲染(retina 不糊),噪声晶格按 CSS 像素网格
    // (px = baseFrequency / dpr)—— 两者同 feTurbulence 的栅格化行为。
    const fit = () => {
      const dpr = window.devicePixelRatio || 1;
      const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
      const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
      if (forceFit || canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
        gl.viewport(0, 0, w, h);
        forceFit = false;
      }
      // 1.35:较设计稿 baseFrequency 1.15 晶格更密 → 颗粒更细(用户定档)
      gl.uniform1f(pxLoc, 1.35 / dpr);
    };
    let raf = 0;
    let seed = 2;
    const draw = () => {
      if (!ready) return;
      fit();
      gl.uniform1f(tLoc, seed);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    const frame = () => {
      raf = 0;
      // t 保持小数值域(hash 的 sin 精度),循环推进
      seed = (seed + 0.618) % 61.0;
      draw();
      // OFF(穿透/停机)/ 后台隐藏时停走:动效只属于活着的系统。
      // 注意:不再读 prefers-reduced-motion —— 氛围噪点恒动,不随系统减弱动效停帧。
      if (!document.hidden && activeRef.current)
        raf = requestAnimationFrame(frame);
    };
    const schedule = () => {
      if (ready && !raf) raf = requestAnimationFrame(frame);
    };
    scheduleRef.current = schedule;
    const onVisibility = () => {
      if (document.hidden) {
        if (raf) cancelAnimationFrame(raf);
        raf = 0;
      } else schedule();
    };
    // WebView2/WKWebView 在 GPU 压力 / 驱动 TDR / 休眠唤醒时会主动丢弃 context。
    // 不处理则画布内容变未定义(常为黑),经 multiply 把整窗 UI 乘成黑屏且不自愈。
    const onLost = (e: Event) => {
      e.preventDefault(); // 必须:否则浏览器永不派发 webglcontextrestored
      ready = false;
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
      // 立即隐藏噪声层:宁可几秒没噪点,也不让黑画布 multiply 乘黑整窗 UI
      if (wrap) wrap.classList.add("ctxlost");
    };
    const onRestored = () => {
      ready = initGL();
      if (wrap) wrap.classList.remove("ctxlost");
      schedule();
    };
    canvas.addEventListener("webglcontextlost", onLost, false);
    canvas.addEventListener("webglcontextrestored", onRestored, false);
    const ro = new ResizeObserver(() => {
      draw(); // resize 立即补一帧,避免拉伸残影
    });
    ro.observe(canvas);
    document.addEventListener("visibilitychange", onVisibility);
    schedule();
    return () => {
      // 不主动 loseContext:StrictMode 双挂载下我们若主动丢弃,同一 canvas 的
      // context 无法复活(getContext 返回已死实例)。浏览器主动丢失走上面的
      // lost/restored 监听恢复;组件与 App 同生命周期,context 随窗口销毁即可。
      if (raf) cancelAnimationFrame(raf);
      ro.disconnect();
      document.removeEventListener("visibilitychange", onVisibility);
      canvas.removeEventListener("webglcontextlost", onLost, false);
      canvas.removeEventListener("webglcontextrestored", onRestored, false);
      scheduleRef.current = () => {};
    };
  }, []);
  useEffect(() => {
    // 重新上电 → 恢复走噪
    if (active) scheduleRef.current();
  }, [active]);
  return (
    <div className="tvnoise" aria-hidden="true" ref={wrapRef}>
      <canvas ref={ref} />
    </div>
  );
}

export default function App() {
  return <AppShell />;
}

// UPTIME 走表:开机从零计,关机冻结最后读数(v3 原则 #5)。
// ref 直写 DOM:每秒走字不进 React 渲染;vdom 文本恒定,父级重渲不会覆写。
function UptimeStamp({ powerOn }: { powerOn: boolean }) {
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!powerOn) return; // 冻结显示,保留最后读数
    const start = performance.now();
    if (ref.current) ref.current.textContent = "UP 00:00:00";
    const iv = window.setInterval(() => {
      const s = Math.floor((performance.now() - start) / 1000);
      const hh = String(Math.floor(s / 3600)).padStart(2, "0");
      const mm = String(Math.floor(s / 60) % 60).padStart(2, "0");
      const ss = String(s % 60).padStart(2, "0");
      if (ref.current) ref.current.textContent = `UP ${hh}:${mm}:${ss}`;
    }, 1000);
    return () => window.clearInterval(iv);
  }, [powerOn]);
  return (
    <span className="stamp" ref={ref}>
      UP 00:00:00
    </span>
  );
}
