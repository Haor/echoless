import { useEffect, useRef, useState } from "react";
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
  openUrl,
  requestSystemAudio,
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
import { FooterBars, Scope, type Telemetry } from "./components/Scope";
import { Dropdown } from "./components/Dropdown";
import { ScrambleText } from "./components/ScrambleText";
import { SlideSwitch } from "./components/SlideSwitch";
import { VolumeWheel } from "./components/VolumeWheel";
import { AdvancedPage } from "./pages/AdvancedPage";
import { DiagnosticsPage } from "./pages/DiagnosticsPage";
import { EnginePage } from "./pages/EnginePage";
import { RtxSetupPage } from "./pages/RtxSetupPage";
import { MicSetupPage } from "./pages/MicSetupPage";
import { simNvafxDoctor, type RtxState } from "./nvafx";
import { simMicDoctor, type MicState } from "./mic";

const appWindow = getCurrentWindow();

// 系统设置 › 隐私与安全性(系统音频录制权限在此开启;具体面板随 macOS 版本)。
const SYS_AUDIO_PRIVACY_URL =
  "x-apple.systempreferences:com.apple.preference.security?Privacy";

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

interface Live {
  mic: number | null;
  ref: number | null;
  out: number | null;
  lat: number | null;
  healthy: boolean;
}

export interface Health {
  input_drops: number;
  ref_underruns: number;
  output_underruns: number;
  stale_drops: number;
  runtime_errors: number;
  diverged: boolean;
  session_dir: string | null;
  backend_error: string | null;
  // 诊断录制实时态(后端 status 提供)。
  recording: boolean;
  rec_elapsed_s: number;
  rec_drops: number;
}
const ZERO_HEALTH: Health = {
  input_drops: 0,
  ref_underruns: 0,
  output_underruns: 0,
  stale_drops: 0,
  runtime_errors: 0,
  diverged: false,
  session_dir: null,
  backend_error: null,
  recording: false,
  rec_elapsed_s: 0,
  rec_drops: 0,
};

export default function App() {
  const [platform, setPlatform] = useState<Platform>("macos");
  const [devices, setDevices] = useState<DeviceList | null>(null);
  const [processors, setProcessors] = useState<Processor[]>([]);
  const [selInput, setSelInput] = useState("default");
  const [selOutput, setSelOutput] = useState("default");
  const [kind, setKind] = useState("sonora_aec3");
  const [pipeline, setPipeline] = useState<PipelineCfg>({
    sample_rate: 48000,
    frame_ms: 10,
    reference_channels: "mono",
  });
  const [params, setParams] = useState<Record<string, unknown>>({});
  const [running, setRunning] = useState(false); // 进程是否存活(含 restart 抖动)
  const [powerOn, setPowerOn] = useState(false); // 用户开关意图(UI 显示/动画只看这个)
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [view, setView] = useState<
    | "overview"
    | "engine"
    | "advanced"
    | "diagnostics"
    | "rtxsetup"
    | "micsetup"
  >("overview");
  const [doctor, setDoctor] = useState<DoctorAudio | null>(null);
  const [nvafx, setNvafx] = useState<NvafxDoctor | null>(null);
  const [nvafxBusy, setNvafxBusy] = useState(false); // RTX runtime 安装中
  // reference:可用源由 devices.reference_sources 提供;mac system 无 loopback → 默认退 none。
  const [reference, setReference] = useState("system");
  const [live, setLive] = useState<Live>({
    mic: null,
    ref: null,
    out: null,
    lat: null,
    healthy: true,
  });
  const [health, setHealth] = useState<Health>(ZERO_HEALTH);
  // 开发态:页面内按 ~ 切换,临时解开 NVAFX 平台/doctor 门槛,用于走通前端流程。
  const [dev, setDev] = useState(false);
  // 开发态下用模拟 doctor 走 RTX 安装流程(mac 上也能逐屏过)。
  const [devRtxState, setDevRtxState] = useState<RtxState>("runtime_not_installed");
  // 开发态下用模拟 doctor 走虚拟麦路由流程(mac 上也能逐屏过)。
  const [devMicState, setDevMicState] = useState<MicState>("missing");
  // 开发态下按 ` 把整机平台模拟成 Windows,用于在 mac 上预览 win 全流程。
  const [devWin, setDevWin] = useState(false);
  // 设备 I/O 重采样态(started 事件给出;设备原生率 ≠ 管线率时后端会重采样)。
  const [io, setIo] = useState<{
    mic: boolean;
    micRate: number | null;
  } | null>(null);
  // 诊断录制
  const [rec, setRec] = useState(false);
  const [diagSeconds, setDiagSeconds] = useState<number | null>(null);
  const [diagDir, setDiagDir] = useState("");

  const telRef = useRef<Telemetry>({ mic: -120, ref: -120, out: -120, on: false });
  // 当前 run 实际生效的参考源(由 started 给出),供 status 判断是否 Process Tap。
  const refSourceRef = useRef<string | null>(null);
  // 子进程最近一条 stderr 日志(用于在非预期退出时报错)。
  const lastLogRef = useRef<string>("");
  const runningRef = useRef(running);
  runningRef.current = running;
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

  // 录制就地起停命令(运行中改录制态用 stdin,不重启 run)。
  function startDiag() {
    if (diagDirRef.current) {
      startDiagnostics(diagDirRef.current, diagSecondsRef.current).catch((e) =>
        setErr(String(e)),
      );
    }
  }

  // 平台 + 设备/处理器枚举 + 事件订阅
  useEffect(() => {
    // 清理可能残留的 sidecar(前端 reload 后 Rust 子进程可能还活着 → 状态脱同步)。
    stopRun().catch(() => {});
    getPlatform().then(setPlatform).catch(() => {});
    refreshDevices();
    listProcessors()
      .then((m) => setProcessors(m.processors))
      .catch((e) => setErr(String(e)));
    doctorAudio().then(setDoctor).catch(() => {});
    nvafxDoctor().then(setNvafx).catch(() => {});
    defaultDiagDir().then(setDiagDir).catch(() => {});

    const uns: UnlistenFn[] = [];
    (async () => {
      uns.push(
        await onRunEvent((ev) => {
          if (ev.type === "started") {
            telRef.current.on = true;
            setRunning(true);
            refSourceRef.current = ev.reference_source ?? null;
            setIo({
              mic: Boolean(ev.io_resampling?.mic),
              micRate: ev.mic_device_sample_rate ?? null,
            });
            // run 已起;若录制开关为开,就地下发 start_diagnostics(power-on-with-rec /
            // 改设置重启 后的统一入口)。session 目录随后由 diagnostics_started 给出。
            if (recRef.current && diagDirRef.current) startDiag();
            return;
          }
          // 录制已就地启动:拿到 session 目录。
          if (ev.type === "diagnostics_started") {
            setHealth((h) => ({ ...h, session_dir: ev.session_dir }));
            return;
          }
          if (ev.type === "diagnostics_stopping") {
            return; // 等 diagnostics_done 收尾
          }
          if (ev.type === "control_error") {
            setErr(`${ev.cmd}: ${ev.message}`);
            return;
          }
          // 实时音量变更回执:值由前端驱动,无需处理(否则会被当成 status 读到一堆 undefined,
          // 让 MIC/REF/OUT 表瞬间跳成「—」)。
          if (ev.type === "output_level_changed") {
            return;
          }
          // 诊断录制收尾:writer 已 finalize 文件。仅「录满 max_seconds」时
          // 自动关开关 + 打开会话目录;stopped / run_exit / error 不弹目录。
          if (ev.type === "diagnostics_done") {
            if (ev.reason === "max_seconds") {
              recRef.current = false;
              setRec(false);
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
            setDoctor((d) =>
              d && d.system_audio_permission !== "granted"
                ? { ...d, system_audio_permission: "granted" }
                : d,
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
          setLive({
            mic: s.mic_dbfs,
            ref: s.ref_dbfs,
            out: s.out_dbfs,
            lat: s.estimated_user_latency_ms,
            healthy:
              !s.diverged && s.runtime_errors === 0 && !s.last_backend_error,
          });
          setHealth((h) => ({
            input_drops: s.input_drops ?? 0,
            ref_underruns: s.ref_underruns ?? 0,
            output_underruns: s.output_underruns ?? 0,
            stale_drops: s.stale_drops ?? 0,
            runtime_errors: s.runtime_errors ?? 0,
            diverged: Boolean(s.diverged),
            // status 的 session_dir 缺省时保留 started 给的值。
            session_dir: s.diagnostics_session_dir ?? h.session_dir,
            backend_error: s.last_backend_error ?? null,
            recording: Boolean(s.recording),
            rec_elapsed_s: s.diagnostics_elapsed_s ?? 0,
            rec_drops: s.diagnostics_drops ?? 0,
          }));
        }),
      );
      uns.push(
        await onRunExit((ev) => {
          telRef.current.on = false;
          setRunning(false);
          setIo(null);
          refSourceRef.current = null;
          // 后端按子进程标记:intentional=主动停/重启 → 正常,不报错。
          if (ev.intentional) return;
          // 非预期退出(子进程自己挂了,如设备不支持采样率)→ 如实反映失败 + 报错。
          // 稍等让 stderr 末行(真正的错误原因)到达,再显示。
          if (powerOnRef.current) {
            window.setTimeout(() => {
              if (!powerOnRef.current) return;
              setPowerOn(false);
              setErr(lastLogRef.current || "运行已停止:子进程意外退出");
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
  }, []);

  // Esc 始终有意义:在次级页按 Esc 返回 Overview。
  useEffect(() => {
    if (view === "overview") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setView("overview");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view]);

  // backend 切换 / manifest 加载 → 优先恢复该引擎上次的参数(保住 LocalVQE 选的模型),
  // 否则用 manifest 默认值。
  useEffect(() => {
    setParams(
      paramsByKind.current[kind] ??
        defaultParams(processors.find((p) => p.kind === kind)),
    );
  }, [processors, kind]);

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
      setDev((d) => !d);
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
      setDevWin((w) => !w);
    };
    window.addEventListener("keydown", onBacktick);
    return () => window.removeEventListener("keydown", onBacktick);
  }, [dev]);

  function refreshDevices() {
    listDevices()
      .then((d) => {
        setDevices(d);
        setSelInput((cur) => (cur === "default" ? pickDefaultInput(d.inputs) : cur));
        setSelOutput((cur) =>
          cur === "default" ? pickDefaultOutput(d.outputs) : cur,
        );
        // 默认 reference:system 可用就用 system,否则退到 none;用户改过则保留。
        const sys = d.reference_sources.find((r) => r.id === "system");
        setReference((cur) =>
          cur !== "system" ? cur : sys && !sys.available ? "none" : "system",
        );
      })
      .catch((e) => setErr(String(e)));
  }

  function recheckNvafx(runtimeDir?: string) {
    if (dev) return; // dev 模拟:状态由 dev 切换条控制
    nvafxDoctor(runtimeDir).then(setNvafx).catch(() => {});
  }

  // 重跑虚拟声卡检测(MIC SETUP 向导的 recheck)。
  function recheckAudio() {
    doctorAudio().then(setDoctor).catch(() => {});
  }

  // 用户主动请求系统音频录制权限:触发一次 Process Tap probe(macOS 弹窗),回传更新 doctor。
  function probeSystemAudio() {
    setErr(null);
    requestSystemAudio()
      .then(setDoctor)
      .catch((e) => setErr(String(e)));
  }

  // RTX runtime 安装:解压 common + 架构 model,回传安装后 doctor 报告。
  // dev 模拟:不调后端,延迟后置 ready,以便走通"安装中 → 就绪"。
  function installNvafx(commonZip: string, modelZip: string) {
    if (dev) {
      setNvafxBusy(true);
      window.setTimeout(() => {
        setDevRtxState("ready");
        setNvafxBusy(false);
      }, 900);
      return;
    }
    const runtimeDir = (paramsRef.current.runtime_dir as string) || undefined;
    setNvafxBusy(true);
    setErr(null);
    nvafxInstall({ commonZip, modelZip, runtimeDir })
      .then(setNvafx)
      .catch((e) => setErr(String(e)))
      .finally(() => setNvafxBusy(false));
  }

  // 从公共 GitHub release 下载并安装(按 GPU 架构自动选模型)。dev 下模拟。
  function downloadInstallNvafx() {
    if (dev) {
      setNvafxBusy(true);
      window.setTimeout(() => {
        setDevRtxState("ready");
        setNvafxBusy(false);
      }, 1200);
      return;
    }
    const runtimeDir = (paramsRef.current.runtime_dir as string) || undefined;
    setNvafxBusy(true);
    setErr(null);
    nvafxDownloadInstall({ runtimeDir })
      .then(setNvafx)
      .catch((e) => setErr(String(e)))
      .finally(() => setNvafxBusy(false));
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
    setBusy(true);
    setErr(null);
    setHealth(ZERO_HEALTH);
    lastLogRef.current = ""; // 清掉上次的 stderr,避免旧错误误报
    try {
      const toml = currentToml();
      const v = await validateConfig(toml);
      if (!v.ok) {
        setErr(v.errors.map((e) => `${e.path}: ${e.message}`).join("; "));
        setBusy(false);
        return;
      }
      telRef.current.on = true;
      await startRun(toml, 80);
      setRunning(true);
      setPowerOn(true);
    } catch (e) {
      setErr(String(e));
      telRef.current.on = false;
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    setBusy(true);
    try {
      await stopRun();
    } catch (e) {
      setErr(String(e));
    } finally {
      telRef.current.on = false;
      setRunning(false);
      setPowerOn(false);
      setBusy(false);
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
      setView("engine");
      return;
    }
    // mac 系统音频参考需 48k;采样率不符 → 去 Advanced 改,避免启动即被后端拒。
    if (sysRefRateConflict) {
      setView("advanced");
      return;
    }
    await start();
  }

  // 运行中改配置 → 重启 runtime 应用新值(后端契约要求)。
  // 成功路径不动 powerOn,避免状态框/开关跟着 scramble(各管各的)。
  async function applyChange(next: Override) {
    if (!powerOnRef.current) return;
    setBusy(true);
    try {
      await stopRun();
      const toml = currentToml(next);
      const v = await validateConfig(toml);
      if (!v.ok) {
        setErr(v.errors.map((e) => `${e.path}: ${e.message}`).join("; "));
        telRef.current.on = false;
        setRunning(false);
        setPowerOn(false);
        return;
      }
      telRef.current.on = true;
      await startRun(toml, 80);
      setRunning(true);
      setErr(null);
    } catch (e) {
      setErr(String(e));
      telRef.current.on = false;
      setRunning(false);
      setPowerOn(false);
    } finally {
      setBusy(false);
    }
  }

  // 切 backend:优先恢复该引擎上次的参数(保住 LocalVQE 选过的模型),否则用 manifest 默认。
  function changeKind(k: string) {
    paramsByKind.current[kind] = paramsRef.current; // 存下当前引擎的参数
    const np =
      paramsByKind.current[k] ??
      defaultParams(processors.find((p) => p.kind === k));
    setKind(k);
    setParams(np);
    applyChange({ kind: k, params: np });
  }
  // 改单个 chain 参数(NOISE / Advanced)。
  function setParam(key: string, val: unknown) {
    const np = { ...paramsRef.current, [key]: val };
    paramsRef.current = np; // 同步更新 ref:探测后自动恢复引擎时能立刻读到新 initial_delay_ms
    paramsByKind.current[kind] = np;
    setParams(np);
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
    setKind("localvqe");
    setParams(np);
    applyChange({ kind: "localvqe", params: np });
  }
  // 改管线项(Advanced:sample_rate / frame_ms / reference_channels)。
  function changePipeline(patch: Partial<PipelineCfg>) {
    const npl = { ...pipelineRef.current, ...patch };
    pipelineRef.current = npl; // 同步更新 ref:探测后自动恢复引擎时能立刻读到新 near_delay
    setPipeline(npl);
    applyChange({ pipeline: npl });
  }
  // 输出音量(滚轮 0-100):落进 pipeline(下次 start 用);运行中走 stdin 实时控制,
  // 逐 buffer 生效、零掉音(不 applyChange —— 那会 stop+start 抖音频)。
  function changeOutVolume(v: number) {
    const npl = { ...pipelineRef.current, output_level: v };
    pipelineRef.current = npl;
    setPipeline(npl);
    if (powerOnRef.current) {
      setOutputLevel(v).catch((e) => setErr(String(e)));
    }
  }
  // 诊断录制开关:运行中 → 经 stdin 就地起停(不重启 run);未运行 → 仅置位,
  // 等 run 启动后由 started 处理。
  function setRecording(on: boolean) {
    setRec(on);
    recRef.current = on;
    if (!powerOnRef.current) return;
    if (on) startDiag();
    else stopDiagnostics().catch((e) => setErr(String(e)));
  }
  // 时长 / 目录:仅更新状态。录制中改动 → 重发 start_diagnostics 让新参数立即生效
  // (后端先收尾旧 session 再开新的)。
  function setRecSeconds(v: number | null) {
    setDiagSeconds(v);
    diagSecondsRef.current = v;
    if (powerOnRef.current && recRef.current) startDiag();
  }
  function setRecDir(v: string) {
    setDiagDir(v);
    diagDirRef.current = v;
    if (powerOnRef.current && recRef.current) startDiag();
  }

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
  const off = !powerOn;
  const stopped = off;
  const unstable = powerOn && !live.healthy;
  // 诚实状态:没有有效参考(reference=none 或 ref 信号静默)时,AEC 无消回声依据
  // → 不显示绿色 "REMOVING ECHO",而是琥珀 "NO REFERENCE"。
  // dev 预览态:跟随下拉展示值(referenceView),且不因 REF 静默判无参考
  // (预览跑在真实 mac 上无对应音频);真实运行行为不变。
  const refSel = dev ? referenceView : reference;
  const hasReference =
    refSel !== "none" && (dev || !(live.ref !== null && live.ref <= -100));
  const noRef = powerOn && !unstable && !hasReference;
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

  const statusText = stopped
    ? t("echoStopped")
    : unstable
      ? t("unstable")
      : noRef
        ? t("noReference")
        : t("removingEcho");
  // 四态语义色:停止=灰 / 不稳定=黄(告警) / 无参考=蓝(待机,非告警) / 工作中=绿。
  const boxClass = stopped
    ? "box stopped"
    : unstable
      ? "box warn"
      : noRef
        ? "box idle"
        : "box";
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

  const dash = (v: number | null, d = 1) =>
    v === null ? "—" : v.toFixed(d);

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
            <button className="cbtn" onClick={() => appWindow.minimize()}>
              <CapMin />
            </button>
            <button className="cbtn" onClick={() => appWindow.toggleMaximize()}>
              <CapMax />
            </button>
            <button className="cbtn close" onClick={() => appWindow.close()}>
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
        <div className="status">
          <span className={boxClass}>
            {/* 运行=圆点 ●,停止=方块 ■ */}
            <span
              className={`sq ${powerOn ? "dot" : ""} ${noRef ? "tri" : ""}`}
            />{" "}
            <ScrambleText text={statusText} />
          </span>
          <span className="m">
            {t("latency")} <b>{dash(live.lat, 0)}</b> {t("ms")}
          </span>
          {!activeReady ? (
            <span
              className="m setup"
              onClick={() => setView("engine")}
              style={{ color: "var(--warn)", cursor: "default" }}
            >
              {t("engSetupHint")} <span className="mk">&raquo;</span>
            </span>
          ) : sysRefRateConflict ? (
            <span
              className="m setup"
              onClick={() => setView("advanced")}
              style={{ color: "var(--warn)" }}
            >
              {t("sysRefRate")} <span className="mk">&raquo;</span>
            </span>
          ) : sysAudioDenied ? (
            <span
              className="m setup"
              onClick={() => openUrl(SYS_AUDIO_PRIVACY_URL)}
              style={{ color: "var(--warn)" }}
            >
              {t("sysAudioGrant")} <span className="mk">&raquo;</span>
            </span>
          ) : sysAudioUndet ? (
            <span className="m setup" onClick={probeSystemAudio}>
              {t("sysAudioRequest")} <span className="mk">&raquo;</span>
            </span>
          ) : (
            <span className="m">{unstable ? t("checkSetup") : t("stable")}</span>
          )}
        </div>

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
                  setSelInput(v);
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
                    key={m.kind}
                    className={`b ${kind === m.kind ? "active" : ""} ${
                      exp ? "exp" : ""
                    } ${supported && !rdy ? "unready" : ""}`}
                    disabled={!supported}
                    onClick={() => {
                      // 未就绪(LocalVQE 无模型 / NVAFX doctor 未过):跳 Engine 配置,不生成非法配置。
                      if (!rdy) {
                        setKind(m.kind);
                        setView("engine");
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
                  setReference(v);
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
                  setSelOutput(v);
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
                className={`b ${ns ? "active" : ""}`}
                onClick={() => setParam("ns", true)}
              >
                ON
              </button>
              <button
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

        {/* ---- signal:三路示波 ---- */}
        <div className="sig">
          <div className="h">
            <span className="t">// {t("signal")}</span>
            <span className="v">{t("sigFlow")}</span>
          </div>
          <div className="scope">
            <div className="near">
              <div className="trace">
                <span className="lb">MIC</span>
                <Scope traceKey="mic" telRef={telRef} phase={0} />
                <span className="db">
                  {dash(live.mic)} <i>dBFS</i>
                </span>
              </div>
              <div className="trace">
                <span className="lb">REF</span>
                <Scope traceKey="ref" telRef={telRef} phase={2.1} />
                <span className="db">
                  {dash(live.ref)} <i>dBFS</i>
                </span>
              </div>
            </div>
            <div className="gap">&raquo;</div>
            <div className="far">
              <div className="trace">
                <span className="lb">OUT</span>
                <Scope traceKey="out" telRef={telRef} phase={4.2} />
                <span className="db">
                  {dash(live.out)} <i>dBFS</i>
                </span>
              </div>
            </div>
          </div>
        </div>
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
            onSetup={() => setView("rtxsetup")}
          />
        )}
        {view === "rtxsetup" && (
          <RtxSetupPage
            doctor={nvafxView}
            busy={nvafxBusy}
            dev={dev}
            devState={devRtxState}
            onDevState={setDevRtxState}
            onBack={() => setView("engine")}
            onRecheck={recheckNvafx}
            onInstall={installNvafx}
            onDownloadInstall={downloadInstallNvafx}
            onUse={() => {
              changeKind("nvidia_afx_aec");
              setView("overview");
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
          <DiagnosticsPage
            rec={rec}
            seconds={diagSeconds}
            diagDir={diagDir}
            running={powerOn}
            health={health}
            doctor={doctorView}
            onMicSetup={() => setView("micsetup")}
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
            onDevState={setDevMicState}
            onBack={() => setView("diagnostics")}
            onRecheck={recheckAudio}
          />
        )}
      </main>

      {/* ---- footer ---- */}
      <footer className="fbar">
        {view === "overview" ? (
          <>
            <button
              className="link"
              style={linkStyle}
              onClick={() => setView("engine")}
            >
              {t("engine")} <span className="mk">&gt;&gt;&gt;</span>
            </button>
            <button
              className="link"
              style={linkStyle}
              onClick={() => setView("advanced")}
            >
              {t("advanced")} <span className="mk">&gt;&gt;&gt;</span>
            </button>
            <button
              className="link"
              style={linkStyle}
              onClick={() => setView("diagnostics")}
            >
              {t("diagnostics")} <span className="mk">&gt;&gt;&gt;</span>
            </button>
          </>
        ) : (
          <button
            className="link"
            style={linkStyle}
            onClick={() =>
              setView(
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
            <span
              className="stamp err"
              style={{ color: "var(--warn)", cursor: "pointer" }}
              title={`${err} · 点击关闭`}
              onClick={() => setErr(null)}
            >
              {err.length > 44 ? err.slice(0, 44) + "…" : err}{" "}
              <span className="mk">✕</span>
            </span>
          ) : (
            <>
              <span className="fdot">·</span>
              <span className="stamp">{stamp}</span>
            </>
          )}
        </span>
        <FooterBars telRef={telRef} />
      </footer>
    </div>
  );
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
