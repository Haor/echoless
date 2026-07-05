import { memo, useEffect, useRef, useState } from "react";
import { openUrl } from "../api";
import { useI18n } from "../i18n";
import { useRuntimeLive } from "../runtimeTelemetry";
import { ScrambleText } from "./ScrambleText";

// 直达「屏幕与系统音频录制」隐私面板;未知 anchor 时 macOS 回退打开隐私根页。
export const SYS_AUDIO_PRIVACY_URL =
  "x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture";

const dash = (v: number | null, d = 1) => (v === null ? "—" : v.toFixed(d));

// 运行五态:工作中 / 无参考 / 不稳定 / 穿透(P8-D1:OFF = mic 直通) / 已停止。
export type RunStatusKind = "live" | "noref" | "warn" | "bypass" | "stopped";

// srail 监视状态字(v14:随状态 scramble)。
export const RAIL_TEXT: Record<RunStatusKind, string> = {
  live: "MONITOR LIVE",
  noref: "REF SILENT",
  warn: "MONITOR LIVE",
  bypass: "AEC BYPASS",
  stopped: "MONITOR HELD",
};

// 四态判定 + A4 防抖。两层配合:
//   1. 参考电平滞回:REF 有声(> -100 dBFS)立即视为有参考;判「参考静音」
//      须连续静音 REF_SILENT_HOLD_MS —— 音乐/语音的自然间隙逐帧跌破阈值,
//      瞬时判定会让恢复计时不断被重置,状态卡死在 NO REFERENCE。
//   2. 状态不对称防抖:劣化(live→warn/noref)须稳定 STATUS_HOLD_MS 才显示;
//      恢复到 live 与电源开关切换立即反映。
const STATUS_HOLD_MS = 2500;
const REF_SILENT_HOLD_MS = 3000;

export function useRunStatusKind(
  powerOn: boolean,
  refSel: string,
  dev: boolean,
  bypassed = false,
): RunStatusKind {
  const live = useRuntimeLive();

  // 滞回:记录最近一次「参考有声」时刻;开机时刻也算(给采集链路启动宽限)。
  const lastRefLoudAt = useRef(0);
  useEffect(() => {
    if (powerOn) lastRefLoudAt.current = performance.now();
  }, [powerOn]);
  if (live.ref !== null && live.ref > -100) {
    lastRefLoudAt.current = performance.now();
  }
  const refSilent =
    live.ref !== null &&
    live.ref <= -100 &&
    performance.now() - lastRefLoudAt.current > REF_SILENT_HOLD_MS;
  const hasReference = refSel !== "none" && (dev || !refSilent);

  const raw: RunStatusKind = !powerOn
    ? "stopped"
    : bypassed
      ? "bypass"
      : !live.healthy
        ? "warn"
        : !hasReference
          ? "noref"
          : "live";

  const [shown, setShown] = useState<RunStatusKind>(raw);
  const pending = useRef<{ kind: RunStatusKind; timer: number } | null>(null);
  useEffect(() => {
    const clearPending = () => {
      if (pending.current) {
        clearTimeout(pending.current.timer);
        pending.current = null;
      }
    };
    if (raw === shown) {
      clearPending(); // 候选态回归当前显示 → 取消切换
      return;
    }
    if (
      raw === "stopped" ||
      shown === "stopped" ||
      raw === "bypass" ||
      shown === "bypass" ||
      raw === "live"
    ) {
      clearPending();
      setShown(raw); // 开/关机、进出穿透、恢复正常立即反映;只有劣化才防抖
      return;
    }
    if (pending.current?.kind === raw) return; // 已在计时
    clearPending();
    const timer = window.setTimeout(() => {
      pending.current = null;
      setShown(raw);
    }, STATUS_HOLD_MS);
    pending.current = { kind: raw, timer };
  }, [raw, shown]);
  useEffect(
    () => () => {
      if (pending.current) clearTimeout(pending.current.timer);
    },
    [],
  );
  return shown;
}

const BOX_CLASS: Record<RunStatusKind, string> = {
  live: "box",
  noref: "box idle",
  warn: "box warn",
  bypass: "box stopped", // 穿透 = 用户主动的「关」态,同停机灰阶
  stopped: "box stopped",
};

// 状态盒(zb 电源格内的灯行;v12 起格子即容器,盒子褪框)。
export const RuntimeStatusStrip = memo(function RuntimeStatusStrip({
  statusKind,
}: {
  statusKind: RunStatusKind;
}) {
  const { t } = useI18n();
  const statusText =
    statusKind === "stopped"
      ? t("echoStopped")
      : statusKind === "bypass"
        ? t("bypassLive")
        : statusKind === "warn"
          ? t("unstable")
          : statusKind === "noref"
            ? t("noReference")
            : t("removingEcho");
  return (
    <div className="status">
      <span className={BOX_CLASS[statusKind]}>
        <span
          className={`sq ${statusKind === "live" || statusKind === "warn" ? "dot" : ""} ${statusKind === "noref" ? "tri" : ""}`}
        />{" "}
        <ScrambleText text={statusText} />
      </span>
    </div>
  );
});

// 电源格注脚(zsub):PIPELINE 延迟 + 右侧状态字/引导动作(v7 层级调低)。
export const RuntimeSubline = memo(function RuntimeSubline({
  statusKind,
  activeReady,
  sysAudioDenied,
  sysAudioUndet,
  onEngineSetup,
  onProbeSystemAudio,
  onCheckSetup,
}: {
  statusKind: RunStatusKind;
  activeReady: boolean;
  sysAudioDenied: boolean;
  sysAudioUndet: boolean;
  onEngineSetup: () => void;
  onProbeSystemAudio: () => void;
  onCheckSetup: () => void;
}) {
  const live = useRuntimeLive();
  const { t } = useI18n();
  return (
    <div className="zsub">
      <span className="m">
        PIPELINE <b>{dash(live.lat, 0)}</b> {t("ms")}
      </span>
      <span className="fdot">·</span>
      {!activeReady ? (
        <button type="button" className="m act plainbtn" onClick={onEngineSetup}>
          {t("engSetupHint")} <span className="mk">&raquo;</span>
        </button>
      ) : sysAudioDenied ? (
        <button
          type="button"
          className="m act plainbtn"
          onClick={() => openUrl(SYS_AUDIO_PRIVACY_URL)}
        >
          {t("sysAudioGrant")} <span className="mk">&raquo;</span>
        </button>
      ) : sysAudioUndet ? (
        <button
          type="button"
          className="m act plainbtn"
          onClick={onProbeSystemAudio}
        >
          {t("sysAudioRequest")} <span className="mk">&raquo;</span>
        </button>
      ) : statusKind === "warn" ? (
        <button type="button" className="m act plainbtn" onClick={onCheckSetup}>
          <ScrambleText text={t("checkSetup")} />
        </button>
      ) : (
        <span className="m">
          <ScrambleText text={t("stable")} />
        </span>
      )}
    </div>
  );
});
