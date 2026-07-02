import { useCallback, useEffect, useReducer, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  buildConfigToml,
  defaultDiagDir,
  doctorAudio,
  getPlatform,
  listDevices,
  listProcessors,
  nvafxDoctor,
  nvafxDownloadInstall,
  nvafxInstall,
  onRunEvent,
  onRunExit,
  onRunLog,
  openPath,
  requestSystemAudio,
  setAec3Agc,
  setAec3Ns,
  setInitialDelayMs,
  setLocalvqeNoiseGate,
  setNearDelayMs,
  setOutputLevel,
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
import { RuntimeStatusStrip } from "./components/RuntimeStatusStrip";
import { AdvancedPage } from "./pages/AdvancedPage";
import { EnginePage } from "./pages/EnginePage";
import { RtxSetupPage } from "./pages/RtxSetupPage";
import { MicSetupPage } from "./pages/MicSetupPage";
import { simNvafxDoctor, type RtxState } from "./nvafx";
import { simMicDoctor, type MicState } from "./mic";
import {
  publishRuntimeStatus,
  resetRuntimeHealth,
  setDiagnosticsSessionDir,
} from "./runtimeTelemetry";

const appWindow = getCurrentWindow();
const REQUIRED_RUN_CONTROLS = [
  "start_diagnostics",
  "stop_diagnostics",
  "set_output_level",
  "set_near_delay_ms",
  "set_initial_delay_ms",
  "set_aec3_ns",
  "set_aec3_agc",
];

const DEVICE_SELECTION_KEY = "echoless.deviceSelection.v1";

// 设备选择值统一用 stable_id(跨重启稳定;mic/output 配置直接吃它)。
// 选默认输出:优先虚拟声卡(VB-CABLE / BlackHole),否则系统默认。
function pickDefaultOutput(outs: AudioDevice[]): string {
  const virt = outs.find((d) => /cable|blackhole|vb-?audio/i.test(d.name));
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
  return "none";
}

const MODELS: { kind: string; label: string }[] = [
  { kind: "sonora_aec3", label: "AEC3" },
  { kind: "localvqe", label: "LOCALVQE" },
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

function isNearDelayOnlyPatch(patch: Partial<PipelineCfg>): boolean {
  const keys = Object.keys(patch);
  return keys.length === 1 && keys[0] === "near_delay_ms";
}

function hotInitialDelayValue(value: unknown): number | null {
  if (value == null || value === "") return 0;
  const delayMs = Number(value);
  if (!Number.isFinite(delayMs)) return null;
  return Math.round(delayMs);
}

function hotLocalvqeNoiseGateValue(next: Record<string, unknown>): {
  enabled: boolean;
  thresholdDbfs: number;
} | null {
  const threshold =
    next.noise_gate_threshold_dbfs == null || next.noise_gate_threshold_dbfs === ""
      ? -45
      : Number(next.noise_gate_threshold_dbfs);
  if (!Number.isFinite(threshold)) return null;
  return { enabled: Boolean(next.noise_gate), thresholdDbfs: threshold };
}

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
  powerOn: boolean;
  busy: boolean;
  err: string | null;
  view: View;
  doctor: DoctorAudio | null;
  nvafx: NvafxDoctor | null;
  nvafxBusy: boolean;
  dev: boolean;
  devRtxState: RtxState;
  devMicState: MicState;
  devWin: boolean;
  io: IoResamplingState;
  rec: boolean;
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
  pipeline: PipelineCfg;
  params: Record<string, unknown>;
};

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
  powerOn: false,
  busy: false,
  err: null,
  view: "overview",
  doctor: null,
  nvafx: null,
  nvafxBusy: false,
  dev: false,
  devRtxState: "runtime_not_installed",
  devMicState: "missing",
  devWin: false,
  io: null,
  rec: false,
  diagSeconds: null,
  diagDir: "",
};

const INITIAL_ENGINE_STATE: EngineState = {
  kind: "sonora_aec3",
  pipeline: INITIAL_PIPELINE,
  params: {},
};

function initSelection(): SelectionState {
  const saved = readSavedDeviceSelection();
  return {
    selInput: saved.input ?? "default",
    selOutput: saved.output ?? "default",
    reference: saved.reference ?? "system",
  };
}

function useAppController() {
  const [appState, updateApp] = useReducer(
    patchReducer<AppState>,
    INITIAL_APP_STATE,
  );
  const [selection, updateSelection] = useReducer(
    patchReducer<SelectionState>,
    undefined,
    initSelection,
  );
  const [engineState, updateEngine] = useReducer(
    patchReducer<EngineState>,
    INITIAL_ENGINE_STATE,
  );
  const {
    platform,
    devices,
    processors,
    powerOn,
    busy,
    err,
    view,
    doctor,
    nvafx,
    nvafxBusy,
    dev,
    devRtxState,
    devMicState,
    devWin,
    io,
    rec,
    diagSeconds,
    diagDir,
  } = appState;
  const { selInput, selOutput, reference } = selection;
  const { kind, pipeline, params } = engineState;

  const telRef = useRef<Telemetry>({ mic: -120, ref: -120, out: -120, on: false });
  // 当前 run 实际生效的参考源(由 started 给出),供 status 判断是否 Process Tap。
  const refSourceRef = useRef<string | null>(null);
  const cliVersionRef = useRef<string | null>(null);
  const runControlsRef = useRef<Set<string> | null>(null);
  // 子进程最近一条 stderr 日志(用于在非预期退出时报错)。
  const lastLogRef = useRef<string>("");
  const powerOnRef = useRef(powerOn);
  powerOnRef.current = powerOn;
  const pipelineRef = useRef(pipeline);
  pipelineRef.current = pipeline;
  const paramsRef = useRef(params);
  paramsRef.current = params;
  // 记住每个引擎的参数(如 LocalVQE 选的模型),切换引擎再切回来不丢。
  const paramsByKind = useRef<Record<string, Record<string, unknown>>>({});
  const recRef = useRef(rec);
  recRef.current = rec;
  const diagSecondsRef = useRef(diagSeconds);
  diagSecondsRef.current = diagSeconds;
  const diagDirRef = useRef(diagDir);
  diagDirRef.current = diagDir;
  const { t } = useI18n();

  const noteError = useCallback((err: string | null) => {
    updateApp({ err });
  }, []);

  const gotoView = useCallback((view: View) => {
    updateApp({ view });
  }, []);

  const chooseDevRtxState = useCallback((devRtxState: RtxState) => {
    updateApp({ devRtxState });
  }, []);

  const chooseDevMicState = useCallback((devMicState: MicState) => {
    updateApp({ devMicState });
  }, []);

  const kindRef = useRef(kind);
  kindRef.current = kind;

  const hasRunControl = useCallback((cmd: string): boolean => {
    return runControlsRef.current?.has(cmd) ?? false;
  }, []);

  const reportMissingRunControl = useCallback((cmd: string) => {
    noteError(
      `CLI ${cliVersionRef.current ?? "unknown"} does not support runtime control "${cmd}". Rebuild or replace the bundled echoless CLI.`,
    );
  }, [noteError]);

  // 录制就地起停命令(运行中改录制态用 stdin,不重启 run)。
  const startDiag = useCallback(() => {
    if (!hasRunControl("start_diagnostics")) {
      reportMissingRunControl("start_diagnostics");
      return;
    }
    if (diagDirRef.current) {
      startDiagnostics(diagDirRef.current, diagSecondsRef.current).catch((e) =>
        noteError(String(e)),
      );
    }
  }, [hasRunControl, noteError, reportMissingRunControl]);

  const refreshDevices = useCallback(() => {
    listDevices()
      .then((d) => {
        updateApp({ devices: d });
        updateSelection((cur) => ({
          selInput: deviceSelectionStillExists(d.inputs, cur.selInput)
            ? cur.selInput
            : pickDefaultInput(d.inputs),
          selOutput: deviceSelectionStillExists(d.outputs, cur.selOutput)
            ? cur.selOutput
            : pickDefaultOutput(d.outputs),
          // 默认 reference:system 可用就用 system,否则退到 none;用户改过则保留。
          reference: pickReference(d, cur.reference),
        }));
      })
      .catch((e) => noteError(String(e)));
  }, [noteError]);

  // 平台 + 设备/处理器枚举 + 事件订阅
  useEffect(() => {
    // 清理可能残留的 sidecar(前端 reload 后 Rust 子进程可能还活着 → 状态脱同步)。
    stopRun().catch(() => {});
    getPlatform()
      .then((platform) => updateApp({ platform }))
      .catch(() => {});
    refreshDevices();
    listProcessors()
      .then((m) => {
        updateApp({ processors: m.processors });
        const proc = m.processors.find((p) => p.kind === kindRef.current);
        updateEngine((cur) =>
          Object.keys(cur.params).length === 0
            ? {
                ...cur,
                params:
                  paramsByKind.current[kindRef.current] ?? defaultParams(proc),
              }
            : cur,
        );
      })
      .catch((e) => noteError(String(e)));
    doctorAudio()
      .then((doctor) => updateApp({ doctor }))
      .catch(() => {});
    nvafxDoctor()
      .then((nvafx) => updateApp({ nvafx }))
      .catch(() => {});
    defaultDiagDir()
      .then((diagDir) => updateApp({ diagDir }))
      .catch(() => {});

    const uns: UnlistenFn[] = [];
    (async () => {
      uns.push(
        await onRunEvent((ev) => {
          if (ev.type === "started") {
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
            if (recRef.current && diagDirRef.current) startDiag();
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
            noteError(`${ev.cmd}: ${ev.message}`);
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
          if (ev.type === "aec3_ns_changed" || ev.type === "aec3_agc_changed") {
            return;
          }
          if (ev.type === "localvqe_noise_gate_changed") {
            return;
          }
          // 诊断录制收尾:writer 已 finalize 文件。仅「录满 max_seconds」时
          // 自动关开关 + 打开会话目录;stopped / run_exit / error 不弹目录。
          if (ev.type === "diagnostics_done") {
            if (ev.reason === "max_seconds") {
              recRef.current = false;
              updateApp({ rec: false });
              if (ev.session_dir) openPath(ev.session_dir).catch(() => {});
            }
            return;
          }
          // status
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
          const tel = telRef.current;
          tel.mic = s.mic_dbfs;
          tel.ref = s.ref_dbfs;
          tel.out = s.out_dbfs;
          tel.on = true;
          tel.micWave = s.mic_wave;
          tel.refWave = s.ref_wave;
          tel.outWave = s.out_wave;
          publishRuntimeStatus(s);
        }),
      );
      uns.push(
        await onRunExit((ev) => {
          telRef.current.on = false;
          updateApp({ io: null });
          refSourceRef.current = null;
          cliVersionRef.current = null;
          runControlsRef.current = null;
          // 后端按子进程标记:intentional=主动停/重启 → 正常,不报错。
          if (ev.intentional) return;
          // 非预期退出(子进程自己挂了,如设备不支持采样率)→ 如实反映失败 + 报错。
          // 稍等让 stderr 末行(真正的错误原因)到达,再显示。
          if (powerOnRef.current) {
            window.setTimeout(() => {
              if (!powerOnRef.current) return;
              updateApp({
                powerOn: false,
                err: lastLogRef.current || "运行已停止:子进程意外退出",
              });
            }, 150);
          }
        }),
      );
      uns.push(
        await onRunLog((line) => {
          if (line.trim()) lastLogRef.current = line;
        }),
      );
    })();
    return () => uns.forEach((u) => u());
  }, [hasRunControl, noteError, refreshDevices, startDiag]);

  // Esc 始终有意义:在次级页按 Esc 返回 Overview。
  useEffect(() => {
    if (view === "overview") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") gotoView("overview");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [gotoView, view]);

  useEffect(() => {
    saveDeviceSelection({
      input: selInput,
      output: selOutput,
      reference,
    });
  }, [selInput, selOutput, reference]);

  // 桌面 app:禁用 Tab 键焦点移动(避免按钮出现键盘选中框)。
  useEffect(() => {
    const onTab = (e: KeyboardEvent) => {
      if (e.key === "Tab") e.preventDefault();
    };
    window.addEventListener("keydown", onTab);
    return () => window.removeEventListener("keydown", onTab);
  }, []);

  // 开发态快捷键:按 ~ 切换(在输入框里则正常输入,不触发)。
  useEffect(() => {
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

  // 开发态下按 `(同一物理键不按 Shift)在 Windows / macOS 模拟平台间切换。
  useEffect(() => {
    const onBacktick = (e: KeyboardEvent) => {
      if (e.key !== "`") return;
      const el = document.activeElement;
      if (el && /^(INPUT|TEXTAREA)$/.test(el.tagName)) return;
      if (!dev) return;
      e.preventDefault();
      updateApp((state) => ({ ...state, devWin: !state.devWin }));
    };
    window.addEventListener("keydown", onBacktick);
    return () => window.removeEventListener("keydown", onBacktick);
  }, [dev]);

  function recheckNvafx(runtimeDir?: string) {
    if (dev) return; // dev 模拟:状态由 dev 切换条控制
    nvafxDoctor(runtimeDir)
      .then((nvafx) => updateApp({ nvafx }))
      .catch(() => {});
  }

  // 重跑虚拟声卡检测(MIC SETUP 向导的 recheck)。
  function recheckAudio() {
    doctorAudio()
      .then((doctor) => updateApp({ doctor }))
      .catch(() => {});
  }

  // 用户主动请求系统音频录制权限:触发一次 Process Tap probe(macOS 弹窗),回传更新 doctor。
  const probeSystemAudio = useCallback(() => {
    noteError(null);
    requestSystemAudio()
      .then((doctor) => updateApp({ doctor }))
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
    const runtimeDir = (paramsRef.current.runtime_dir as string) || undefined;
    updateApp({ nvafxBusy: true, err: null });
    nvafxInstall({ commonZip, modelZip, runtimeDir })
      .then((nvafx) => updateApp({ nvafx }))
      .catch((e) => noteError(String(e)))
      .finally(() => updateApp({ nvafxBusy: false }));
  }

  // 从公共 GitHub release 下载并安装(按 GPU 架构自动选模型)。dev 下模拟。
  function downloadInstallNvafx() {
    if (dev) {
      updateApp({ nvafxBusy: true });
      window.setTimeout(() => {
        updateApp({ devRtxState: "ready", nvafxBusy: false });
      }, 1200);
      return;
    }
    const runtimeDir = (paramsRef.current.runtime_dir as string) || undefined;
    updateApp({ nvafxBusy: true, err: null });
    nvafxDownloadInstall({ runtimeDir })
      .then((nvafx) => updateApp({ nvafx }))
      .catch((e) => noteError(String(e)))
      .finally(() => updateApp({ nvafxBusy: false }));
  }

  // 引擎就绪判定:AEC3 永远就绪;LocalVQE 需模型;NVAFX 需平台支持 + doctor 通过。
  // 开发态(dev)临时解开 NVAFX 的平台/doctor 门槛,用于走通前端流程。
  function engineReady(k: string): boolean {
    const proc = processors.find((p) => p.kind === k);
    if (proc && !proc.platforms.includes(platform) && !dev) return false;
    // LocalVQE 是否就绪要看它自己持久化的模型(可能当前激活的是别的引擎),
    // 否则在 AEC3 激活时点 LocalVQE 会因 params.model 为空而误判未就绪、每次跳引擎页。
    if (k === "localvqe")
      return Boolean(
        k === kind ? params.model : paramsByKind.current["localvqe"]?.model,
      );
    if (k === "nvidia_afx_aec") return dev || Boolean(nvafx?.ok);
    return true;
  }

  type Override = Partial<{
    mic: string;
    output: string;
    reference: string;
    kind: string;
    pipeline: PipelineCfg;
    params: Record<string, unknown>;
  }>;

  function currentToml(over?: Override) {
    return buildConfigToml({
      mic: over?.mic ?? selInput,
      output: over?.output ?? selOutput,
      reference: over?.reference ?? reference,
      kind: over?.kind ?? kind,
      pipeline: over?.pipeline ?? pipelineRef.current,
      params: over?.params ?? paramsRef.current,
      // 录制改由 stdin 就地控制(start/stop_diagnostics),不再写进 toml。
      diagnostics: null,
    });
  }

  async function start() {
    updateApp({ busy: true, err: null });
    resetRuntimeHealth();
    lastLogRef.current = ""; // 清掉上次的 stderr,避免旧错误误报
    try {
      const toml = currentToml();
      const v = await validateConfig(toml);
      if (!v.ok) {
        updateApp({
          err: v.errors.map((e) => `${e.path}: ${e.message}`).join("; "),
          busy: false,
        });
        return;
      }
      telRef.current.on = true;
      await startRun(toml, 80);
      updateApp({ powerOn: true });
    } catch (e) {
      noteError(String(e));
      telRef.current.on = false;
    } finally {
      updateApp({ busy: false });
    }
  }

  async function stop() {
    updateApp({ busy: true });
    try {
      await stopRun();
    } catch (e) {
      noteError(String(e));
    } finally {
      telRef.current.on = false;
      updateApp({ powerOn: false, busy: false });
    }
  }

  // 延迟侦测专用:probe 需独占麦克风/输出 → AdvancedPage 在探测前后调这个停/起引擎。
  // 恢复时走 start(),会用上探测刚写入的 near_delay/initial_delay(refs 已同步)。
  async function setRunForProbe(on: boolean) {
    if (on) await start();
    else await stop();
  }

  async function togglePower() {
    if (busy) return;
    if (powerOn) {
      await stop();
      return;
    }
    // 引擎未就绪(无模型 / doctor 未过)→ 先去 Engine 配置,避免启动即失败。
    if (!engineReady(kind)) {
      gotoView("engine");
      return;
    }
    // mac 系统音频参考需 48k;采样率不符 → 去 Advanced 改,避免启动即被后端拒。
    if (sysRefRateConflict) {
      gotoView("advanced");
      return;
    }
    await start();
  }

  // Applies changes that still require rebuilding the sidecar runtime.
  // Hot controls bypass this path to avoid an audio dropout.
  async function applyChange(next: Override) {
    if (!powerOnRef.current) return;
    updateApp({ busy: true });
    try {
      await stopRun();
      const toml = currentToml(next);
      const v = await validateConfig(toml);
      if (!v.ok) {
        updateApp({
          err: v.errors.map((e) => `${e.path}: ${e.message}`).join("; "),
          powerOn: false,
        });
        telRef.current.on = false;
        return;
      }
      telRef.current.on = true;
      await startRun(toml, 80);
      noteError(null);
    } catch (e) {
      noteError(String(e));
      telRef.current.on = false;
      updateApp({ powerOn: false });
    } finally {
      updateApp({ busy: false });
    }
  }

  // 切 backend:优先恢复该引擎上次的参数(保住 LocalVQE 选过的模型),否则用 manifest 默认。
  function changeKind(k: string) {
    paramsByKind.current[kind] = paramsRef.current; // 存下当前引擎的参数
    const np =
      paramsByKind.current[k] ??
      defaultParams(processors.find((p) => p.kind === k));
    updateEngine({ kind: k, params: np });
    applyChange({ kind: k, params: np });
  }
  // 改单个 chain 参数(NOISE / Advanced)。
  function setParam(key: string, val: unknown) {
    const np = { ...paramsRef.current, [key]: val };
    paramsRef.current = np; // 同步更新 ref:探测后自动恢复引擎时能立刻读到新 initial_delay_ms
    paramsByKind.current[kind] = np;
    updateEngine({ params: np });
    if (kind === "sonora_aec3" && key === "initial_delay_ms") {
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
    if (kind === "sonora_aec3" && (key === "ns" || key === "ns_level")) {
      if (powerOnRef.current) {
        if (!hasRunControl("set_aec3_ns")) {
          reportMissingRunControl("set_aec3_ns");
          return;
        }
        setAec3Ns(Boolean(np.ns), String(np.ns_level ?? "low")).catch((e) =>
          noteError(String(e)),
        );
      }
      return;
    }
    if (kind === "sonora_aec3" && key === "agc") {
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
    applyChange({ params: np });
  }
  // 选 LocalVQE 模型(清单常驻):原子地切到 localvqe 引擎并设 model,避免把 model 写到当前引擎上。
  function pickLocalvqeModel(path: string) {
    const base =
      paramsByKind.current["localvqe"] ??
      defaultParams(processors.find((p) => p.kind === "localvqe"));
    const np = { ...base, model: path };
    paramsByKind.current[kind] = paramsRef.current; // 存下当前引擎
    paramsByKind.current["localvqe"] = np;
    updateEngine({ kind: "localvqe", params: np });
    applyChange({ kind: "localvqe", params: np });
  }
  function hotNearDelayValue(next: PipelineCfg): number {
    return next.near_delay_ms ?? (platform === "macos" ? 25 : 0);
  }
  // 改管线项。near_delay_ms 可运行中热控;采样率/帧长/参考声道仍需重启。
  function changePipeline(patch: Partial<PipelineCfg>) {
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
    applyChange({ pipeline: npl });
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
  }, [hasRunControl, noteError, reportMissingRunControl, startDiag]);
  // 时长 / 目录:仅更新状态。录制中改动 → 重发 start_diagnostics 让新参数立即生效
  // (后端先收尾旧 session 再开新的)。
  const setRecSeconds = useCallback((v: number | null) => {
    updateApp({ diagSeconds: v });
    diagSecondsRef.current = v;
    if (powerOnRef.current && recRef.current) startDiag();
  }, [startDiag]);
  const setRecDir = useCallback((v: string) => {
    updateApp({ diagDir: v });
    diagDirRef.current = v;
    if (powerOnRef.current && recRef.current) startDiag();
  }, [startDiag]);

  // dev 模拟 Windows 时,系统 render loopback 原生可用 → 注入一个 system 参考源,
  // 让 win 预览忠实(真实 win 上后端本就返回 system available;mac 才退 none)。
  const refSources =
    dev && devWin
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
  // dev win 下默认就选 system(否则沿用真实选择)。
  const referenceView = dev && devWin ? "system" : reference;
  // reference 概念 = 系统正在播放的声音(输出内容)。只保留有意义的参考源:
  //   system(Process Tap / loopback)、none、output 设备回环、以及承载系统声的虚拟声卡输入
  //   (BlackHole / VB-CABLE)。隐藏物理麦克风等(选它们当参考无意义)。
  const VIRTUAL_REF = /blackhole|vb-?cable|vb-?audio|cable|loopback|stereo\s*mix|soundflower/i;
  const availRefs = refSources.filter(
    (r) =>
      r.available &&
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

  // dev 下可把平台模拟成 Windows(按 `),让 mac 也能预览 win 全流程(标题栏/引擎/虚拟麦)。
  const platformView: Platform = dev && devWin ? "windows" : platform;
  const isMac = platformView === "macos";
  const refSel = dev ? referenceView : reference;
  const ns = Boolean(params.ns);
  // 降噪是 AEC3 管线独有(其它 backend 无 ns 参数)→ 不支持时置灰。
  const nsSupported = Boolean(
    processors.find((p) => p.kind === kind)?.params?.ns,
  );
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
  // mac Process Tap reference 要求全局采样率必须 48k(与引擎无关 —— LocalVQE 的 16k
  // 由 ProcessorChain 内部适配)。不符 → 阻止运行,引导去 Advanced 改采样率。
  const sysRefRateConflict = usingSysRef && pipeline.sample_rate !== 48000;

  return (
    <div className={`window ${isMac ? "mac" : "win"}`}>
      {/* ---- titlebar ---- */}
      <header className="tbar" data-tauri-drag-region>
        <AppIcon />
        <span className="screen">
          <ScrambleText text={viewTitle} />
        </span>
        <span className="hatch" />
        {dev && (
          <span className="devtag">
            DEV · {platformView === "windows" ? "WIN" : "MAC"}
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
        <>
        <div className="kick">
          <span className="d">
            <i />
            <i />
            <i />
          </span>{" "}
          {t("kicker")}
        </div>
        <div className="hero">
          <div className="word">ECHOLESS</div>
          <SlideSwitch on={powerOn} onToggle={togglePower} disabled={busy} />
        </div>
        <RuntimeStatusStrip
          powerOn={powerOn}
          activeReady={activeReady}
          refSel={refSel}
          dev={dev}
          sysRefRateConflict={sysRefRateConflict}
          sysAudioDenied={sysAudioDenied}
          sysAudioUndet={sysAudioUndet}
          onEngineSetup={() => gotoView("engine")}
          onAdvanced={() => gotoView("advanced")}
          onProbeSystemAudio={probeSystemAudio}
        />

        <hr className="hair" />

        {/* ---- controls ---- */}
        <div className="rows">
          <div className="row">
            <span className="bul">•</span>
            <span className="k">{t("input")}</span>
            <span className="co">:</span>
            <span className="v">
              <Dropdown
                value={selInput}
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

          <div className="row">
            <span className="bul">•</span>
            <span className="k">{t("model")}</span>
            <span className="co">:</span>
            <div className="segg" id="models">
              {MODELS.map((m) => {
                const proc = processors.find((p) => p.kind === m.kind);
                const supported =
                  !proc || proc.platforms.includes(platform) || dev;
                const exp = proc?.experimental;
                const rdy = engineReady(m.kind);
                return (
                  <button
                    type="button"
                    key={m.kind}
                    className={`b ${kind === m.kind ? "active" : ""} ${
                      exp ? "exp" : ""
                    } ${supported && !rdy ? "unready" : ""}`}
                    disabled={!supported}
                    onClick={() => {
                      // 未就绪(LocalVQE 无模型 / NVAFX doctor 未过):跳 Engine 配置,不生成非法配置。
                      if (!rdy) {
                        updateEngine({ kind: m.kind });
                        gotoView("engine");
                      } else {
                        changeKind(m.kind);
                      }
                    }}
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

          <div className="row">
            <span className="bul">•</span>
            <span className="k">{t("output")}</span>
            <span className="co">:</span>
            <span className="v">
              <Dropdown
                value={selOutput}
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
                <span className="mk">&gt;&gt;&gt;</span> in app pick{" "}
                <b>{cableName}</b> as mic
              </span>
            )}
            <span className="ico">
              <IcoOutput />
            </span>
          </div>

          <div className="row">
            <span className="bul">•</span>
            <span className="k">{t("noise")}</span>
            <span className="co">:</span>
            <div className={`segg ${nsSupported ? "" : "dim"}`} id="ns">
              <button
                type="button"
                className={`b ${ns ? "active" : ""}`}
                onClick={() => setParam("ns", true)}
              >
                ON
              </button>
              <button
                type="button"
                className={`b ${!ns ? "active" : ""}`}
                onClick={() => setParam("ns", false)}
              >
                OFF
              </button>
            </div>
            <span className="sp" />
            <span className="meta">
              {nsSupported ? t("reduceNoise") : "AEC3 only"}
            </span>
            <span className="ico">
              <IcoNoise />
            </span>
          </div>
        </div>

        <hr className="hair" />

        <RuntimeSignalPanel telRef={telRef} powerOn={powerOn} />
        </>
        )}
        {view === "engine" && (
          <EnginePage
            processors={processors}
            platform={platformView}
            kind={kind}
            params={params}
            doctor={nvafxView}
            dev={dev}
            onSelect={changeKind}
            onParam={setParam}
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
            dev={dev}
            devState={devRtxState}
            onDevState={chooseDevRtxState}
            onBack={() => gotoView("engine")}
            onRecheck={recheckNvafx}
            onInstall={installNvafx}
            onDownloadInstall={downloadInstallNvafx}
            onUse={() => {
              changeKind("nvidia_afx_aec");
              gotoView("overview");
            }}
          />
        )}
        {view === "advanced" && (
          <AdvancedPage
            processors={processors}
            kind={kind}
            pipeline={pipeline}
            params={params}
            onPipeline={changePipeline}
            onParam={setParam}
            platform={platformView}
            mic={selInput}
            reference={reference}
            output={selOutput}
            running={powerOn}
            onSetRun={setRunForProbe}
          />
        )}
        {view === "diagnostics" && (
          <RuntimeDiagnosticsPage
            rec={rec}
            seconds={diagSeconds}
            diagDir={diagDir}
            running={powerOn}
            doctor={doctorView}
            onMicSetup={() => gotoView("micsetup")}
            onRec={setRecording}
            onSeconds={setRecSeconds}
            onDir={setRecDir}
          />
        )}
        {view === "micsetup" && (
          <MicSetupPage
            doctor={doctorView}
            platform={platformView}
            dev={dev}
            devState={devMicState}
            onDevState={chooseDevMicState}
            onBack={() => gotoView("diagnostics")}
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
              style={linkStyle}
              onClick={() => gotoView("engine")}
            >
              {t("engine")} <span className="mk">&gt;&gt;&gt;</span>
            </button>
            <button
              type="button"
              className="link"
              style={linkStyle}
              onClick={() => gotoView("advanced")}
            >
              {t("advanced")} <span className="mk">&gt;&gt;&gt;</span>
            </button>
            <button
              type="button"
              className="link"
              style={linkStyle}
              onClick={() => gotoView("diagnostics")}
            >
              {t("diagnostics")} <span className="mk">&gt;&gt;&gt;</span>
            </button>
          </>
        ) : (
          <button
            type="button"
            className="link"
            style={linkStyle}
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
          />
          {err ? (
            <button
              type="button"
              className="stamp err plainbtn"
              style={{ color: "var(--warn)", cursor: "pointer" }}
              title={`${err} · 点击关闭`}
              onClick={() => noteError(null)}
            >
              {err.length > 44 ? err.slice(0, 44) + "…" : err}{" "}
              <span className="mk">✕</span>
            </button>
          ) : (
            <>
              <span className="fdot">·</span>
              <span className="stamp">{stamp}</span>
            </>
          )}
        </span>
        <RuntimeFooterBars telRef={telRef} powerOn={powerOn} />
      </footer>
    </div>
  );
}

export default function App() {
  return useAppController();
}

const linkStyle: React.CSSProperties = {
  color: "var(--t-soft)",
  textDecoration: "none",
  display: "flex",
  alignItems: "center",
  gap: 7,
  background: "transparent",
  border: "none",
  font: "inherit",
  letterSpacing: "inherit",
  textTransform: "inherit",
};
