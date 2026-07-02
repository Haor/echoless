import { useEffect, useReducer, useRef } from "react";
import type { ParamSpec, Platform, Processor } from "../types";
import { probeDelay, type NearDelayProbeResult, type PipelineCfg } from "../api";
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
}

// 蜂鸣节奏(对齐 CLI:startup 4s + pre-roll 0.5s,每声 70ms / 间隔 650ms ≈ 720ms,共 12 声)。
const PROBE_BEEPS = 12;
const PROBE_FIRST_MS = 4500;
const PROBE_STEP_MS = 720;
// 信号判定阈值:dBFS 低于此视为没收到。
const PROBE_SIG_DBFS = -55;
// mac 上 near_delay 被侦测设非零时,顺带给 AEC3 一个 8ms 初始延迟 hint,
// 减少初始 echo-path 搜索(实测有效)。仅 AEC3、仅 macOS、仅 recommended>0 时写。
const PROBE_INIT_DELAY_MS = 8;

// 参数说明(悬浮 label 时提示)。缺省键无提示。
const DESC: Record<string, { en: string; zh: string }> = {
  sample_rate: {
    en: "Pipeline sample rate (must divide by 100). Restarts runtime.",
    zh: "管线采样率(须能被 100 整除)。改动会重启运行时。",
  },
  frame_ms: { en: "Realtime frame size. Restarts runtime.", zh: "实时帧长。改动会重启运行时。" },
  reference_channels: {
    en: "Far-end reference channel mode (mono is the stable baseline).",
    zh: "远端参考声道模式(mono 为稳定基线)。",
  },
  ns_level: {
    en: "Only effective when NS is on; NS is off by default.",
    zh: "仅在降噪开启时有效;降噪默认关闭。",
  },
  agc: {
    en: "Off by default; avoids volume pumping (loud/quiet swings).",
    zh: "默认关闭,避免音量泵动(忽大忽小)。",
  },
  near_delay_ms: {
    en: "Top-level near/mic alignment delay. Empty = backend default (macOS 25, others 0). Applies live while running.",
    zh: "顶层近端对齐延迟。留空走后端默认(macOS 25 / 其它 0)。运行中可实时生效。",
  },
  initial_delay_ms: {
    en: "Initial stream delay hint; runtime still estimates dynamically. On macOS the probe writes it: 8ms when a near delay is applied, else the measured echo delay.",
    zh: "初始延迟提示;运行时仍会动态估计。macOS 上侦测会写入:设了近端延迟时写 8ms,否则写实测 echo 延迟。",
  },
  tail_ms: {
    en: "Echo tail length. Auto ≈ AEC3 default (~52ms).",
    zh: "回声拖尾长度。自动时走 AEC3 默认(约 52ms)。",
  },
  delay_num_filters: {
    en: "Delay search window size. Auto ≈ 5 (~608ms).",
    zh: "延迟搜索窗大小。自动约为 5(约 608ms)。",
  },
  linear_stable_echo_path: {
    en: "Assume a more linear/stable echo path (pure loopback). Off by default.",
    zh: "假设 echo path 更线性稳定(偏纯 loopback)。默认关闭。",
  },
  model: { en: "GGUF model path (required).", zh: "GGUF 模型路径(必填)。" },
  library: { en: "LocalVQE dynamic library path (auto if empty).", zh: "LocalVQE 动态库路径(留空自动)。" },
  threads: { en: "CPU threads (auto if empty).", zh: "CPU 线程数(留空自动)。" },
  noise_gate: { en: "LocalVQE noise gate.", zh: "LocalVQE 噪声门。" },
  noise_gate_threshold_dbfs: { en: "Noise gate threshold (dBFS).", zh: "噪声门阈值(dBFS)。" },
  intensity_ratio: { en: "RTX AEC strength.", zh: "RTX AEC 强度。" },
  runtime_dir: { en: "NVIDIA AFX runtime dir (auto if empty).", zh: "NVIDIA AFX runtime 目录(留空自动)。" },
  model_path: { en: "RTX AEC model path (auto if empty).", zh: "RTX AEC 模型路径(留空自动)。" },
  on_runtime_error: { en: "On backend runtime error: silence or bypass.", zh: "运行时出错时:静音或直通。" },
  use_default_gpu: { en: "Use the default GPU.", zh: "使用默认 GPU。" },
  disable_cuda_graph: { en: "Disable CUDA graph.", zh: "关闭 CUDA graph。" },
};

function backendLabel(kind: string, proc?: Processor): string {
  if (kind === "nvidia_afx_aec") return "NVAFX";
  if (kind === "sonora_aec3") return "AEC3";
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
  if (platform !== "macos" || kind !== "sonora_aec3") return null;
  if (r.recommended_near_delay_ms > 0) return PROBE_INIT_DELAY_MS;
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

  useEffect(
    () => () => {
      if (timer.current != null) window.clearInterval(timer.current);
    },
    [],
  );

  async function runProbe() {
    if (probing) return;
    updateProbe({ probing: true, probe: null, probeErr: null, lit: 0 });
    // 引擎在跑 → probe 要独占设备:先自动停机,跑完在 finally 自动恢复。
    const wasRunning = running;
    try {
      if (wasRunning) {
        updateProbe({ phase: "pausing" });
        await onSetRun(false);
      }
      updateProbe({ phase: "probing" });
      const t0 = Date.now();
      if (timer.current != null) window.clearInterval(timer.current);
      timer.current = window.setInterval(() => {
        const el = Date.now() - t0;
        const n = Math.max(
          0,
          Math.min(PROBE_BEEPS, Math.floor((el - PROBE_FIRST_MS) / PROBE_STEP_MS) + 1),
        );
        updateProbe({ lit: n });
      }, 100);
      const r = await probeDelay({ mic, reference, output });
      updateProbe({ probe: r });
      // 自动把实测推荐值填进 near_delay_ms(含 8ms AEC 安全余量,后端已算好)。
      onPipeline({ near_delay_ms: r.recommended_near_delay_ms });
      // mac + AEC3 → 顺带写 AEC3 initial_delay_ms(负 lag 写 8ms 安全值,正常正 lag 写实测延迟)。
      const init = probeInitialDelay(r, platform, kind);
      if (init != null) onParam("initial_delay_ms", init);
    } catch (e) {
      updateProbe({ probeErr: String(e) });
    } finally {
      if (timer.current != null) {
        window.clearInterval(timer.current);
        timer.current = null;
      }
      updateProbe({ lit: PROBE_BEEPS });
      // 恢复引擎(用上刚写入的 near_delay/initial_delay);失败不阻塞 UI。
      if (wasRunning) {
        updateProbe({ phase: "restoring" });
        try {
          await onSetRun(true);
        } catch {
          /* 恢复失败时用户可手动开机 */
        }
      }
      updateProbe({ phase: "", probing: false });
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
            <button
              type="button"
              className="dopen pbtn"
              disabled={probing}
              onClick={runProbe}
            >
              {probing ? t("probing") : t("probeRun")}{" "}
              <span className="mk">{probing ? "•••" : "↻"}</span>
            </button>
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
                {probe.recommended_near_delay_ms > 0 ? (
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
}: Props) {
  const { t, lang, setLang } = useI18n();
  const proc = processors.find((p) => p.kind === kind);
  const desc = (k: string) => DESC[k]?.[lang];

  const reqMet = (spec: ParamSpec) =>
    !spec.requires ||
    Object.entries(spec.requires).every(([rk, rv]) => params[rk] === rv);

  // 隐藏未满足 requires 的参数(如 ns 关闭时的 ns_level),而非置灰。
  const backendParams = Object.entries(proc?.params ?? {}).filter(
    ([k, spec]) => k !== "reference_channels" && k !== "ns" && reqMet(spec),
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
          options={(spec.values ?? []).map((v) => ({ value: v, label: v }))}
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
      </div>
    </div>
  );
}
