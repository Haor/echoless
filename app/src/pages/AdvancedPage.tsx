import { useEffect, useReducer, useRef } from "react";
import type { ParamSpec, Platform, Processor } from "../types";
import type { TrayPrefsState } from "../App";
import {
  onProbeProgress,
  probeDelay,
  type NearDelayProbeResult,
  type PipelineCfg,
} from "../api";
import { useI18n, type Lang } from "../i18n";
import { Hint } from "../components/Hint";
import { Field, SegButtons } from "../components/Controls";

interface Props {
  processors: Processor[];
  kind: string;
  pipeline: PipelineCfg;
  params: Record<string, unknown>;
  onPipeline: (patch: Partial<PipelineCfg>) => void;
  onParam: (key: string, val: unknown) => void;
  platform: Platform;
  // 延迟侦测用:当前设备 selector(透传给 probe-delay)+ 是否在跑 + 停/起引擎回调。
  mic: string;
  reference: string;
  output: string;
  running: boolean;
  onSetRun: (on: boolean) => Promise<void>;
  // Windows 托盘偏好(SESSION 段,仅 windows 平台渲染)。
  trayPrefs: TrayPrefsState;
  onTrayPrefs: (patch: Partial<TrayPrefsState>) => void;
}

// 蜂鸣节奏(对齐 CLI:startup 4s + pre-roll 0.5s,每声 70ms / 间隔 650ms ≈ 720ms,共 12 声)。
// 段按钮定宽 74px(v9.2 对齐轴),枚举文案超宽会溢出 → 长值用缩写显示(提交值不变)。
const SELECT_LABELS: Record<string, string> = {
  moderate: "mid",
  veryhigh: "max",
};

// C3/C5 减参:专家级/与引擎页重复的字段不在高级页暴露 ——
//   LocalVQE:model(引擎页模型清单管理)、library/backend/device(auto 即可);
//   NVAFX:model_path/use_default_gpu/disable_cuda_graph(专家字段,走配置文件)。
const HIDDEN_PARAMS: Record<string, Set<string>> = {
  localvqe: new Set(["model", "library", "backend", "device"]),
  nvidia_afx_aec: new Set([
    "model_path",
    "use_default_gpu",
    "disable_cuda_graph",
  ]),
};

const PROBE_BEEPS = 12;
const PROBE_FIRST_MS = 4500;
const PROBE_STEP_MS = 720;
// afplay / cpal 输出流从 spawn 到出声的经验常量(进度灯对齐用,无需精确)。
const PROBE_PLAYER_OPEN_MS = 150;
// 信号判定阈值:dBFS 低于此视为没收到。
const PROBE_SIG_DBFS = -55;
// mac 上 near_delay 被侦测设非零时,顺带给 AEC3 一个 8ms 初始延迟 hint,
// 减少初始 echo-path 搜索(实测有效)。仅 AEC3、仅 macOS、仅 recommended>0 时写。
const PROBE_INIT_DELAY_MS = 8;

// 参数说明(悬浮 label 时提示)。缺省键无提示。
const DESC: Record<string, { en: string; zh: string }> = {
  sample_rate: {
    en: "Audio processing sample rate. 48000 = full band, 16000 = narrowband/lighter. Restarts the engine.",
    zh: "音频处理采样率。48000 全带宽,16000 窄带更省资源。改动会重启引擎。",
  },
  frame_ms: {
    en: "Audio block size per processing step; sets the processing latency. Restarts the engine.",
    zh: "单次处理的音频块时长,决定处理延迟。改动会重启引擎。",
  },
  reference_channels: {
    en: "Whether the system-audio reference enters the AEC as mono or stereo. Mono is the stable baseline.",
    zh: "系统声音(参考)送入 AEC 时按 mono 还是 stereo。Mono 为稳定基线。",
  },
  ns_level: {
    en: "Noise-suppression strength. Higher suppresses more background noise.",
    zh: "降噪强度。越高压掉的背景噪声越多。",
  },
  agc: {
    en: "Automatic Gain Control — auto-levels mic volume. Off by default (it can cause volume pumping).",
    zh: "自动增益控制(AGC):自动调平麦克风音量。默认关(会致音量泵动)。",
  },
  near_delay_ms: {
    en: "Delays the mic to align echoes that arrive before the reference; the value is the negative-direction search depth. macOS default 25ms (probe may override), Windows default 0. Applies live.",
    zh: "延后麦克风,对齐「比系统声音先到」的回声;数值即负方向搜索深度。macOS 默认 25ms(侦测可覆盖),Windows 默认 0。运行中生效。",
  },
  initial_delay_ms: {
    en: "Initial echo-delay value for AEC cold-start alignment; the engine self-estimates after. The probe fills the measured value.",
    zh: "AEC 启动时的回声延迟初值,仅用于冷启动对齐,之后引擎自估。侦测会填入实测值。",
  },
  tail_ms: {
    en: "Echo length the canceller models (tail). Default ~52ms.",
    zh: "AEC 建模的回声长度(拖尾)。默认约 52ms。",
  },
  delay_num_filters: {
    en: "Delay-estimation search range (parallel matched filters). Default 5 ≈ 608ms.",
    zh: "延迟估计的搜索范围(并行滤波器数)。默认 5 ≈ 608ms。",
  },
  linear_stable_echo_path: {
    en: "Assume a linear, stable echo path (closer to pure loopback). Off by default.",
    zh: "假设回声路径线性稳定(偏纯回环)。默认关。",
  },
  model: { en: "Path to the LocalVQE model file (.gguf). Required.", zh: "LocalVQE 模型文件(.gguf)路径。必填。" },
  library: { en: "LocalVQE runtime library path. Auto-detected if empty.", zh: "LocalVQE 运行库路径。留空自动。" },
  threads: {
    en: "CPU threads for model inference. Auto if empty.",
    zh: "模型推理的 CPU 线程数。留空自动。",
  },
  noise_gate: {
    en: "Mutes output below the threshold. Off by default.",
    zh: "输出低于阈值时静音。默认关。",
  },
  noise_gate_threshold_dbfs: {
    en: "Noise-gate threshold (dBFS). Default -45; higher is more aggressive.",
    zh: "噪声门阈值(dBFS)。默认 -45,越高越激进。",
  },
  intensity_ratio: {
    en: "RTX echo-removal strength (0–1). Default 1.0; lower is gentler.",
    zh: "RTX 回声消除强度(0–1)。默认 1.0,越低越保守。",
  },
  on_runtime_error: {
    en: "Fallback when the RTX backend errors: silence (no echo leak) or bypass (mic stays live, echo passes).",
    zh: "RTX 后端出错时:silence 静音不漏回声,bypass 直通保麦克风但漏回声。",
  },
};

function backendLabel(kind: string, proc?: Processor): string {
  if (kind === "nvidia_afx_aec") return "NVAFX";
  if (kind === "aec3") return "AEC3";
  return proc?.label ?? kind;
}

type ProbePhase = "" | "pausing" | "probing" | "restoring";

type ProbeState = {
  probing: boolean;
  phase: ProbePhase;
  lit: number;
  probe: NearDelayProbeResult | null;
  probeErr: string | null;
};

type ProbePatch = Partial<ProbeState> | ((state: ProbeState) => ProbeState);

const PROBE_INITIAL_STATE: ProbeState = {
  probing: false,
  phase: "",
  lit: 0,
  probe: null,
  probeErr: null,
};

function probeReducer(state: ProbeState, patch: ProbePatch): ProbeState {
  return typeof patch === "function" ? patch(state) : { ...state, ...patch };
}

function probeInitialDelay(
  r: NearDelayProbeResult,
  platform: Platform,
  kind: string,
): number | null {
  if (kind !== "aec3") return null;
  // mac:近端延迟已做负方向偏置对齐 → init 只需一个小的安全余量。
  if (platform === "macos") return PROBE_INIT_DELAY_MS;
  // win/其它:不设近端延迟 → init = 实测回声延迟(冷启动对齐起点),需稳定。
  const stable =
    r.warnings.length === 0 &&
    Math.abs(r.event_lag_stddev_ms) < 5 &&
    Math.abs(r.event_lag_drift_ms) < 10;
  const measured = Math.round(r.event_lag_mean_ms);
  return stable && measured >= 1 ? measured : null;
}

function ProbeSection({
  platform,
  kind,
  pipeline,
  onPipeline,
  onParam,
  mic,
  reference,
  output,
  running,
  onSetRun,
}: Pick<
  Props,
  | "platform"
  | "kind"
  | "pipeline"
  | "onPipeline"
  | "onParam"
  | "mic"
  | "reference"
  | "output"
  | "running"
  | "onSetRun"
>) {
  const { t, lang } = useI18n();
  const [state, updateProbe] = useReducer(probeReducer, PROBE_INITIAL_STATE);
  const { probing, phase, lit, probe, probeErr } = state;
  const timer = useRef<number | null>(null);
  const mounted = useRef(true);

  useEffect(() => {
    // setup 必须把 mounted 设回 true:StrictMode(dev)会 mount→cleanup→再 mount,
    // 首次 cleanup 已把 mounted 翻 false;若 setup 不重置,mounted 永久 false,
    // 之后 runProbe 里所有 updateProbeIfMounted 全 no-op —— 进度灯不亮、结果不填、
    // PROBING 永不清除(表现为「有声音但前端卡死」)。
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (timer.current != null) window.clearInterval(timer.current);
    };
  }, []);
  const updateProbeIfMounted = (patch: ProbePatch) => {
    if (mounted.current) updateProbe(patch);
  };

  async function runProbe() {
    if (probing) return;
    updateProbe({ probing: true, probe: null, probeErr: null, lit: 0 });
    // 引擎在跑 → probe 要独占设备:先自动停机,跑完在 finally 自动恢复。
    const wasRunning = running;
    try {
      if (wasRunning) {
        updateProbeIfMounted({ phase: "pausing" });
        await onSetRun(false);
        if (!mounted.current) return;
      }
      updateProbeIfMounted({ phase: "probing" });
      // 进度灯节奏:默认按墙钟估计(旧 CLI 无进度事件的回退);收到 CLI 的
      // beep_train_start 事件后改以真实开播时刻为基准 —— 蜂鸣要等子进程起好
      // 设备 + 4s 稳定期才响,纯墙钟估计会让灯超前声音(音画不同步)。
      let t0 = Date.now();
      let firstMs = PROBE_FIRST_MS;
      let stepMs = PROBE_STEP_MS;
      let beeps = PROBE_BEEPS;
      if (timer.current != null) window.clearInterval(timer.current);
      timer.current = window.setInterval(() => {
        const el = Date.now() - t0;
        const n = Math.max(
          0,
          Math.min(beeps, Math.floor((el - firstMs) / stepMs) + 1),
        );
        updateProbeIfMounted({ lit: n });
      }, 100);
      const unProgress = await onProbeProgress((p) => {
        if (!mounted.current) return;
        if (p.stage !== "beep_train_start") return;
        // 首响 = 事件时刻 + WAV 前导静音 + 播放器打开的经验常量。
        t0 = Date.now();
        firstMs = (p.pre_roll_ms ?? 500) + PROBE_PLAYER_OPEN_MS;
        stepMs = (p.beep_ms ?? 70) + (p.gap_ms ?? 650);
        beeps = p.beeps ?? PROBE_BEEPS;
        updateProbeIfMounted({ lit: 0 });
      });
      let r: NearDelayProbeResult;
      try {
        r = await probeDelay({ mic, reference, output });
      } finally {
        unProgress();
      }
      if (!mounted.current) return;
      updateProbeIfMounted({ probe: r });
      // mac:近端延迟做负方向偏置(后端已含安全余量);win/其它:正 lag 无需近端延迟,不动。
      if (platform === "macos") {
        onPipeline({ near_delay_ms: r.recommended_near_delay_ms });
      }
      // AEC3 初始延迟:mac 写 8ms 余量(near_delay 已对齐),win/其它写实测回声延迟(冷启动起点)。
      const init = probeInitialDelay(r, platform, kind);
      if (init != null) onParam("initial_delay_ms", init);
    } catch (e) {
      updateProbeIfMounted({ probeErr: String(e) });
    } finally {
      if (timer.current != null) {
        window.clearInterval(timer.current);
        timer.current = null;
      }
      if (!mounted.current) return;
      updateProbeIfMounted({ lit: PROBE_BEEPS });
      // 恢复引擎(用上刚写入的 near_delay/initial_delay);失败不阻塞 UI。
      if (wasRunning) {
        updateProbeIfMounted({ phase: "restoring" });
        try {
          await onSetRun(true);
        } catch {
          /* 恢复失败时用户可手动开机 */
        }
      }
      updateProbeIfMounted({ phase: "", probing: false });
    }
  }

  const probeStable =
    !!probe &&
    probe.warnings.length === 0 &&
    Math.abs(probe.event_lag_stddev_ms) < 5 &&
    Math.abs(probe.event_lag_drift_ms) < 10;
  const initWritten = probe ? probeInitialDelay(probe, platform, kind) : null;

  return (
    <>
      <div className="asec">{t("secProbe")}</div>
      <div className="aprobe">
        <div className="arow">
          <Hint text={DESC.near_delay_ms?.[lang]}>
            <span className="alabel">{t("nearDelay")}</span>
          </Hint>
          <span className="aval">
            <Field
              value={pipeline.near_delay_ms}
              numeric
              min={0}
              max={500}
              integer
              placeholder={t("auto")}
              onCommit={(v) =>
                onPipeline({
                  near_delay_ms: v == null ? undefined : Number(v),
                })
              }
            />
          </span>
        </div>
        <div className="apright">
          <div className="prow">
            <Hint text={t("probeRunHint")} pos="top">
              <button
                type="button"
                className="dopen pbtn"
                disabled={probing}
                onClick={runProbe}
              >
                {probing ? t("probing") : t("probeRun")}{" "}
                <span className="mk">{probing ? "•••" : "↻"}</span>
              </button>
            </Hint>
            <span className="pnote">
              {phase === "pausing"
                ? t("probePausing")
                : phase === "restoring"
                  ? t("probeRestoring")
                  : probing
                    ? t("probeQuiet")
                    : running
                      ? t("probeAutoPause")
                      : t("probeQuiet")}
            </span>
          </div>

          {(probing || lit > 0) && !probeErr && (
            <div className="pdots">
              {Array.from({ length: PROBE_BEEPS }, (_, i) => (
                <i key={i} className={`pdot ${i < lit ? "on" : ""}`} />
              ))}
            </div>
          )}

          {probe && !probeErr && (
            <div className="presult">
              <span className="pline">
                <b className={probe.ref_dbfs >= PROBE_SIG_DBFS ? "ok" : "miss"}>
                  {t("probeRef")}{" "}
                  {probe.ref_dbfs >= PROBE_SIG_DBFS ? t("probeOk") : t("probeNoSig")}
                </b>
                <span className="psep">·</span>
                <b className={probe.mic_dbfs >= PROBE_SIG_DBFS ? "ok" : "miss"}>
                  {t("probeMic")}{" "}
                  {probe.mic_dbfs >= PROBE_SIG_DBFS ? t("probeOk") : t("probeNoSig")}
                </b>
                <span className="psep">·</span>
                <span>
                  {t("probeEcho")}{" "}
                  <b>{`${probe.event_lag_mean_ms >= 0 ? "+" : ""}${probe.event_lag_mean_ms.toFixed(1)}ms`}</b>
                </span>
                <span className="psep">·</span>
                <span className={probeStable ? "ok" : "miss"}>
                  {probeStable ? t("probeStable") : t("probeUnstable")}
                </span>
              </span>
              <span className="pline sub">
                {/* 显示随平台分流,与填充逻辑一致:mac 填近端延迟,win/其它只填初始延迟。 */}
                {platform === "macos" ? (
                  <span className="ok">
                    {t("probeRec")} {probe.recommended_near_delay_ms}ms · {t("probeFilled")}
                    {initWritten != null && ` · ${t("probeInit")} ${initWritten}ms`}
                  </span>
                ) : (
                  <span>
                    {t("probeNoFix")}
                    {initWritten != null && (
                      <span className="ok"> · {t("probeInit")} {initWritten}ms</span>
                    )}
                  </span>
                )}
                {probe.warnings.length > 0 && (
                  <span className="miss"> · {probe.warnings.join("; ")}</span>
                )}
              </span>
            </div>
          )}

          {probeErr && <div className="perr">{probeErr}</div>}
        </div>
      </div>
    </>
  );
}

export function AdvancedPage({
  processors,
  kind,
  pipeline,
  params,
  onPipeline,
  onParam,
  platform,
  mic,
  reference,
  output,
  running,
  onSetRun,
  trayPrefs,
  onTrayPrefs,
}: Props) {
  const { t, lang, setLang } = useI18n();
  const proc = processors.find((p) => p.kind === kind);
  const desc = (k: string) => DESC[k]?.[lang];
  const pipelineDisabled = kind === "nvidia_afx_aec";

  const reqMet = (spec: ParamSpec) =>
    !spec.requires ||
    Object.entries(spec.requires).every(([rk, rv]) => params[rk] === rv);

  const hidden = HIDDEN_PARAMS[kind];

  // 隐藏未满足 requires 的参数(如 ns 关闭时的 ns_level),而非置灰。
  const backendParams = Object.entries(proc?.params ?? {}).filter(
    ([k, spec]) =>
      k !== "reference_channels" &&
      k !== "ns" &&
      !hidden?.has(k) &&
      reqMet(spec),
  );

  const control = (key: string, spec: ParamSpec) => {
    const val = params[key];
    if (spec.type === "bool") {
      return (
        <SegButtons
          value={val ? "on" : "off"}
          options={[
            { value: "on", label: "ON" },
            { value: "off", label: "OFF" },
          ]}
          onChange={(v) => onParam(key, v === "on")}
        />
      );
    }
    if (spec.type === "select") {
      return (
        <SegButtons
          value={String(val ?? spec.default ?? "")}
          options={(spec.values ?? []).map((v) => ({
            value: v,
            label: SELECT_LABELS[v] ?? v,
          }))}
          onChange={(v) => onParam(key, v)}
        />
      );
    }
    return (
      <Field
        value={val}
        numeric={spec.type === "number"}
        placeholder={spec.required ? "required" : t("auto")}
        onCommit={(v) => onParam(key, v)}
      />
    );
  };

  const arow = (key: string, label: string, spec: ParamSpec) => (
    <div className="arow" key={key}>
      <Hint text={desc(key)}>
        <span className="alabel">{label}</span>
      </Hint>
      <span className="aval">{control(key, spec)}</span>
    </div>
  );

  return (
    <div className="page">
      <div className="kick">
        <span className="d">
          <i />
          <i />
          <i />
        </span>{" "}
        {t("advNote")}
      </div>
      <hr className="hair" />

      <div className="asec">{t("secPipeline")}</div>
      <div className="acols">
        <div className="arow">
          <Hint text={desc("sample_rate")}>
            <span className="alabel">{t("sampleRate")}</span>
          </Hint>
          <span className="aval">
            <SegButtons
              value={String(pipeline.sample_rate)}
              disabled={pipelineDisabled}
              options={[16000, 48000].map((n) => ({
                value: String(n),
                label: String(n),
              }))}
              onChange={(v) => onPipeline({ sample_rate: Number(v) })}
            />
          </span>
        </div>
        <div className="arow">
          <Hint text={desc("frame_ms")}>
            <span className="alabel">{t("frameMs")}</span>
          </Hint>
          <span className="aval">
            <SegButtons
              value={String(pipeline.frame_ms)}
              disabled={pipelineDisabled}
              options={[10, 20].map((n) => ({
                value: String(n),
                label: `${n} MS`,
              }))}
              onChange={(v) => onPipeline({ frame_ms: Number(v) })}
            />
          </span>
        </div>
        <div className="arow">
          <Hint text={desc("reference_channels")}>
            <span className="alabel">{t("referenceChannels")}</span>
          </Hint>
          <span className="aval">
            <SegButtons
              value={pipeline.reference_channels}
              disabled={pipelineDisabled}
              options={[
                { value: "mono", label: "MONO" },
                { value: "stereo", label: "STEREO" },
              ]}
              onChange={(v) =>
                onPipeline({ reference_channels: v as "mono" | "stereo" })
              }
            />
          </span>
        </div>
      </div>

      <div className="asec">{backendLabel(kind, proc)}</div>
      <div className="acols">
        {backendParams.length === 0 && (
          <div className="pnote">no parameters</div>
        )}
        {backendParams.map(([key, spec]) => arow(key, key, spec))}
      </div>

      <ProbeSection
        platform={platform}
        kind={kind}
        pipeline={pipeline}
        onPipeline={onPipeline}
        onParam={onParam}
        mic={mic}
        reference={reference}
        output={output}
        running={running}
        onSetRun={onSetRun}
      />

      <div className="asec">{t("secSession")}</div>
      <div className="acols">
        <div className="arow">
          <span className="alabel">{t("language")}</span>
          <span className="aval">
            <SegButtons<Lang>
              value={lang}
              options={[
                { value: "en", label: "EN" },
                { value: "zh", label: "中文" },
              ]}
              onChange={setLang}
            />
          </span>
        </div>
        {/* P5 前端侧:托盘偏好(仅 Windows;Rust 端非 Windows 强制 false)。
            只留「关闭到托盘」一个开关 —— 最小化到托盘退役(用户定案 2026-07-05) */}
        {platform === "windows" && (
          <div className="arow">
            <Hint text={t("trayCloseHint")}>
              <span className="alabel">{t("trayClose")}</span>
            </Hint>
            <span className="aval">
              <SegButtons
                value={trayPrefs.closeToTray ? "on" : "off"}
                options={[
                  { value: "on", label: "ON" },
                  { value: "off", label: "OFF" },
                ]}
                onChange={(v) => onTrayPrefs({ closeToTray: v === "on" })}
              />
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
