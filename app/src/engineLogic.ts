// 引擎参数的纯逻辑(审计 T-02:抽出以便单测,不含 Tauri/React 依赖)。
import type { PipelineCfg } from "./api";

// LocalVQE 官方描述:v1.4 = 纯 AEC(无降噪),v1.3 = AEC + 降噪。
// 首页 NOISE 开关在 LVQE 下的语义 = 在这两个版本间切换(文件名 "-aec-" 标记纯 AEC)。
export const LVQE_NS_ON_FILE = "localvqe-v1.3-4.8M-f32.gguf";
export const LVQE_NS_OFF_FILE = "localvqe-v1.4-aec-200K-f32.gguf";

/** 模型文件名带 "-aec-" = 纯 AEC(降噪关)。 */
export const lvqePureAec = (model: unknown): boolean =>
  String(model ?? "").includes("-aec-");

/** NOISE 开关目标文件:ON→v1.3(含降噪),OFF→v1.4(纯 AEC)。 */
export const lvqeNoiseTargetFile = (on: boolean): string =>
  on ? LVQE_NS_ON_FILE : LVQE_NS_OFF_FILE;

export function claimEngineKindChange(
  current: { current: string },
  next: string,
): boolean {
  if (current.current === next) return false;
  current.current = next;
  return true;
}

export type EngineKindSelectionOutcome = "noop" | "setup" | "apply";

export function routeEngineKindSelection(
  current: { current: string },
  next: string,
  ready: boolean,
  handlers: {
    setup: (next: string) => void;
    apply: (previous: string, next: string) => void;
  },
): EngineKindSelectionOutcome {
  if (current.current === next) return "noop";
  if (!ready) {
    handlers.setup(next);
    return "setup";
  }

  const previous = current.current;
  if (!claimEngineKindChange(current, next)) return "noop";
  handlers.apply(previous, next);
  return "apply";
}

export function canChangePipeline(
  kind: string,
  patch: Partial<PipelineCfg>,
): boolean {
  if (kind !== "nvidia_afx_aec") return true;
  return !(
    "sample_rate" in patch ||
    "frame_ms" in patch ||
    "reference_channels" in patch
  );
}

export function pipelineForEngineKind(
  kind: string,
  pipeline: PipelineCfg,
): PipelineCfg {
  if (kind !== "nvidia_afx_aec") return pipeline;
  if (
    pipeline.sample_rate === 48_000 &&
    pipeline.frame_ms === 10 &&
    pipeline.reference_channels === "mono"
  ) {
    return pipeline;
  }
  return {
    ...pipeline,
    sample_rate: 48_000,
    frame_ms: 10,
    reference_channels: "mono",
  };
}

/** 仅改 near_delay_ms 的补丁走热控路径,不需重建 sidecar。 */
export function isNearDelayOnlyPatch(patch: Partial<PipelineCfg>): boolean {
  const keys = Object.keys(patch);
  return keys.length === 1 && keys[0] === "near_delay_ms";
}

/** initial_delay 热控值:空→0,非有限→null(拒发),否则四舍五入。 */
export function hotInitialDelayValue(value: unknown): number | null {
  if (value == null || value === "") return 0;
  const delayMs = Number(value);
  if (!Number.isFinite(delayMs)) return null;
  return Math.round(delayMs);
}

/** LocalVQE 噪声门热控值:阈值空→-45dBFS 默认,非有限→null(拒发)。 */
export function hotLocalvqeNoiseGateValue(next: Record<string, unknown>): {
  enabled: boolean;
  thresholdDbfs: number;
} | null {
  const threshold =
    next.noise_gate_threshold_dbfs == null ||
    next.noise_gate_threshold_dbfs === ""
      ? -45
      : Number(next.noise_gate_threshold_dbfs);
  if (!Number.isFinite(threshold)) return null;
  return { enabled: Boolean(next.noise_gate), thresholdDbfs: threshold };
}

/** near_delay 平台默认:macOS 25ms(Process Tap 固有延迟),其它 0。 */
export function platformNearDelayDefault(platform: string): number {
  return platform === "macos" ? 25 : 0;
}

export interface BypassControlSnapshot {
  bypassed: boolean;
  bypassPending: boolean | null;
}

export function bypassToggleTarget(
  state: BypassControlSnapshot,
): boolean | null {
  return state.bypassPending == null ? !state.bypassed : null;
}

export function settleBypassObservation(
  state: BypassControlSnapshot,
  observed: boolean,
): BypassControlSnapshot {
  return {
    bypassed: observed,
    bypassPending: state.bypassPending === observed ? null : state.bypassPending,
  };
}

export function clearBypassPending(
  state: BypassControlSnapshot,
  target?: boolean,
): BypassControlSnapshot {
  if (target != null && state.bypassPending !== target) return state;
  if (state.bypassPending == null) return state;
  return { ...state, bypassPending: null };
}

export interface SerialQueue<T> {
  /** 入队一个 delta;若已有排队项则合并,只在当前执行结束后跑一次合并结果。 */
  enqueue(delta: Partial<T>): void;
  /** 当前链尾 promise(测试用:await 到全部执行完)。 */
  settled(): Promise<void>;
}

// 审计 B-04:设备/参考/NOISE 切换的 applyChange 必须串行化,否则手动切换与
// 热插拔自动刷新交错的 stop→start 可致「UI 显示 OFF 但 sidecar 仍在采集」。
// 排队期多次 enqueue 合并 delta,只跑最后一次(run 内读最新 state,中间态
// 选择无需逐个重启)。run 应通过 ref 读取最新回调,避免闭包过期。
export function createSerialQueue<T>(
  run: (merged: Partial<T>) => Promise<void>,
): SerialQueue<T> {
  let chain: Promise<void> = Promise.resolve();
  let queued: Partial<T> | null = null;
  return {
    enqueue(delta) {
      if (queued) {
        queued = { ...queued, ...delta };
        return;
      }
      queued = delta;
      chain = chain.then(async () => {
        const merged = queued;
        queued = null;
        if (merged) await run(merged);
      });
    },
    settled() {
      return chain;
    },
  };
}
