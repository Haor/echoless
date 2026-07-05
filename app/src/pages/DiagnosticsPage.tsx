import { open } from "@tauri-apps/plugin-dialog";
import type { Health } from "../runtimeTelemetry";
import type { DoctorAudio } from "../types";
import { openPath } from "../api";
import { useI18n } from "../i18n";
import { Field } from "../components/Controls";
import { Toggle } from "../components/Toggle";

export interface DiagnosticsPageProps {
  rec: boolean;
  seconds: number | null;
  diagDir: string;
  running: boolean;
  health: Health;
  doctor: DoctorAudio | null;
  onMicSetup: () => void;
  onRec: (v: boolean) => void;
  onSeconds: (v: number | null) => void;
  onDir: (v: string) => void;
}

export function DiagnosticsPage({
  rec,
  seconds,
  diagDir,
  running,
  health,
  doctor,
  onMicSetup,
  onRec,
  onSeconds,
  onDir,
}: DiagnosticsPageProps) {
  const { t } = useI18n();
  const active = rec && running;

  // 虚拟麦路由摘要(诊断行):就绪? 通话软件该选哪个 mic?
  const routeReady =
    doctor?.virtual_route_ready ??
    ((doctor?.candidate_outputs.length ?? 0) > 0 &&
      (doctor?.candidate_inputs.length ?? 0) > 0);
  const appMic =
    doctor?.recommended_app_mic ??
    doctor?.candidate_inputs.find((i) => /cable output|blackhole/i.test(i.name)) ??
    doctor?.candidate_inputs[0] ??
    null;

  async function pickDir() {
    try {
      const sel = await open({
        directory: true,
        defaultPath: diagDir || undefined,
      });
      if (typeof sel === "string") onDir(sel);
    } catch {
      /* cancelled */
    }
  }

  const counters: { label: string; value: number | string; warn: boolean }[] = [
    { label: "input drops", value: health.input_drops, warn: health.input_drops > 0 },
    { label: "ref underruns", value: health.ref_underruns, warn: health.ref_underruns > 0 },
    { label: "output underruns", value: health.output_underruns, warn: health.output_underruns > 0 },
    { label: "mic stale", value: health.mic_stale_drops, warn: health.mic_stale_drops > 0 },
    { label: "ref stale", value: health.ref_stale_drops, warn: health.ref_stale_drops > 0 },
    { label: "stale drops", value: health.stale_drops, warn: health.stale_drops > 0 },
    { label: "runtime errors", value: health.runtime_errors, warn: health.runtime_errors > 0 },
    { label: "diverged", value: health.diverged ? "YES" : "NO", warn: health.diverged },
  ];

  return (
    <div className="page">
      <div className="kick">
        <span className="d">
          <i />
          <i />
          <i />
        </span>{" "}
        {t("diagNote")}
      </div>
      <hr className="hair" />

      <div className="asec">{t("virtualMic")}</div>
      <div className="drow">
        <span className="dk">ROUTE</span>
        <span className={`dpath ${routeReady ? "live" : ""}`}>
          {routeReady ? t("micReadyShort") : t("micSetupShort")}
          {appMic ? ` · ${t("micPickShort")}: ${appMic.name}` : ""}
        </span>
        <button type="button" className="dopen" onClick={onMicSetup}>
          {t("setupBtn")} <span className="mk">&raquo;</span>
        </button>
      </div>

      <div className="asec">{t("secRecord")}</div>
      <div className="acols">
        <div className="arow">
          <span className="alabel">{t("record")}</span>
          <span className="aval">
            <Toggle on={rec} onToggle={() => onRec(!rec)} />
          </span>
        </div>
        <div className="arow">
          <span className="alabel">{t("maxSeconds")}</span>
          <span className="aval">
            <Field
              value={seconds}
              numeric
              min={1}
              integer
              placeholder={t("unlimited")}
              onCommit={(v) => onSeconds(v as number | null)}
            />
          </span>
        </div>
      </div>

      <div className="drow">
        <span className="dk">{t("recordDir")}</span>
        <button
          type="button"
          className="dpick plainbtn"
          onClick={pickDir}
          title={diagDir}
        >
          {diagDir || t("choose")}
        </button>
        <button type="button" className="dopen" onClick={() => openPath(diagDir)}>
          {t("openFolder")} <span className="mk">&raquo;</span>
        </button>
      </div>
      <div className="drow">
        <span className="dk">SESSION</span>
        {active && health.session_dir ? (
          <>
            <span className="dpath live">{health.session_dir}</span>
            {health.recording && (
              <span className="recbadge">
                ● {health.rec_elapsed_s.toFixed(1)}s
                {seconds ? ` / ${seconds}s` : ""}
              </span>
            )}
            {health.rec_drops > 0 && (
              <span className="recbadge warn">{health.rec_drops} drops</span>
            )}
            <button
              type="button"
              className="dopen"
              onClick={() => openPath(health.session_dir!)}
            >
              {t("openFolder")} <span className="mk">&raquo;</span>
            </button>
          </>
        ) : (
          <span className="dpath">
            {active ? t("recording") : rec ? t("notRunning") : "—"}
          </span>
        )}
      </div>

      <div className="asec">{t("secHealth")}</div>
      <div className={`acols ${running ? "" : "dim-soft"}`}>
        {counters.map((c) => (
          <div className="arow" key={c.label}>
            <span className="alabel">{c.label}</span>
            <span className={`aval dval ${c.warn ? "warn" : ""}`}>
              {c.value}
            </span>
          </div>
        ))}
      </div>
      {health.backend_error && (
        <div className="drow">
          <span className="dk" style={{ color: "var(--warn)" }}>
            ERROR
          </span>
          <span className="dpath" style={{ color: "var(--warn)" }}>
            {health.backend_error}
          </span>
        </div>
      )}
    </div>
  );
}
