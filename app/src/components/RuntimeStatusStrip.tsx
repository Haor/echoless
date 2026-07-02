import { memo } from "react";
import { openUrl } from "../api";
import { useI18n } from "../i18n";
import { useRuntimeLive } from "../runtimeTelemetry";
import { ScrambleText } from "./ScrambleText";

const SYS_AUDIO_PRIVACY_URL =
  "x-apple.systempreferences:com.apple.preference.security?Privacy";

const dash = (v: number | null, d = 1) => (v === null ? "—" : v.toFixed(d));

export const RuntimeStatusStrip = memo(function RuntimeStatusStrip({
  powerOn,
  activeReady,
  refSel,
  dev,
  sysRefRateConflict,
  sysAudioDenied,
  sysAudioUndet,
  onEngineSetup,
  onAdvanced,
  onProbeSystemAudio,
}: {
  powerOn: boolean;
  activeReady: boolean;
  refSel: string;
  dev: boolean;
  sysRefRateConflict: boolean;
  sysAudioDenied: boolean;
  sysAudioUndet: boolean;
  onEngineSetup: () => void;
  onAdvanced: () => void;
  onProbeSystemAudio: () => void;
}) {
  const live = useRuntimeLive();
  const { t } = useI18n();
  const stopped = !powerOn;
  const unstable = powerOn && !live.healthy;
  const hasReference =
    refSel !== "none" && (dev || !(live.ref !== null && live.ref <= -100));
  const noRef = powerOn && !unstable && !hasReference;
  const statusText = stopped
    ? t("echoStopped")
    : unstable
      ? t("unstable")
      : noRef
        ? t("noReference")
        : t("removingEcho");
  const boxClass = stopped
    ? "box stopped"
    : unstable
      ? "box warn"
      : noRef
        ? "box idle"
        : "box";

  return (
    <div className="status">
      <span className={boxClass}>
        <span className={`sq ${powerOn ? "dot" : ""} ${noRef ? "tri" : ""}`} />{" "}
        <ScrambleText text={statusText} />
      </span>
      <span className="m">
        {t("latency")} <b>{dash(live.lat, 0)}</b> {t("ms")}
      </span>
      {!activeReady ? (
        <button
          type="button"
          className="m setup plainbtn"
          onClick={onEngineSetup}
          style={{ color: "var(--warn)", cursor: "default" }}
        >
          {t("engSetupHint")} <span className="mk">&raquo;</span>
        </button>
      ) : sysRefRateConflict ? (
        <button
          type="button"
          className="m setup plainbtn"
          onClick={onAdvanced}
          style={{ color: "var(--warn)" }}
        >
          {t("sysRefRate")} <span className="mk">&raquo;</span>
        </button>
      ) : sysAudioDenied ? (
        <button
          type="button"
          className="m setup plainbtn"
          onClick={() => openUrl(SYS_AUDIO_PRIVACY_URL)}
          style={{ color: "var(--warn)" }}
        >
          {t("sysAudioGrant")} <span className="mk">&raquo;</span>
        </button>
      ) : sysAudioUndet ? (
        <button
          type="button"
          className="m setup plainbtn"
          onClick={onProbeSystemAudio}
        >
          {t("sysAudioRequest")} <span className="mk">&raquo;</span>
        </button>
      ) : (
        <span className="m">{unstable ? t("checkSetup") : t("stable")}</span>
      )}
    </div>
  );
});
