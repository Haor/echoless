import { open } from "@tauri-apps/plugin-dialog";
import type { Health } from "../runtimeTelemetry";
import type { DoctorAudio, Platform } from "../types";
import { openPath, openUrl } from "../api";
import { useI18n } from "../i18n";
import { Field } from "../components/Controls";
import { Toggle } from "../components/Toggle";

// macOS 隐私设置深链。麦克风有专属锚点;系统录音(14.4+ Audio Capture)无稳定
// 专属锚点,回退到隐私根面板。
const MIC_PRIVACY_URL =
  "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
const SYS_AUDIO_PRIVACY_URL =
  "x-apple.systempreferences:com.apple.preference.security?Privacy";

export interface DiagnosticsPageProps {
  rec: boolean;
  seconds: number | null;
  diagDir: string;
  running: boolean;
  health: Health;
  doctor: DoctorAudio | null;
  platform: Platform;
  onMicSetup: () => void;
  onRequestSystemAudio: () => void;
  onRecheck: () => void;
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
  platform,
  onMicSetup,
  onRequestSystemAudio,
  onRecheck,
  onRec,
  onSeconds,
  onDir,
}: DiagnosticsPageProps) {
  const { t } = useI18n();
  const active = rec && running;
  const isMac = platform === "macos";

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

  // macOS 权限态(麦克风 + 系统录音)。缺字段 → unknown;非 mac 不渲染此区块。
  type PermState = "granted" | "denied" | "undetermined" | "unknown";
  const micPerm = (doctor?.permission_state ?? "unknown") as PermState;
  const sysPerm = (doctor?.system_audio_permission ?? "unknown") as PermState;
  const permLabel = (s: PermState) =>
    s === "granted"
      ? t("permGranted")
      : s === "denied"
        ? t("permDenied")
        : s === "undetermined"
          ? t("permUndet")
          : t("permUnknown");

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

      {/* macOS 权限检查:麦克风 + 系统录音(Process Tap reference)。有问题可二次申请:
          undetermined → 直接触发系统授权弹窗;denied → 系统已记住拒绝,跳隐私设置手动开。 */}
      {isMac && (
        <>
          <div className="asec">{t("secPermissions")}</div>
          <div className="drow">
            <span className="dk">{t("permMic")}</span>
            <span
              className={`dpath ${micPerm === "granted" ? "live" : ""}`}
              style={micPerm === "denied" ? { color: "var(--warn)" } : undefined}
            >
              {permLabel(micPerm)}
            </span>
            {micPerm !== "granted" && (
              <button
                type="button"
                className="dopen"
                onClick={() =>
                  micPerm === "denied" ? openUrl(MIC_PRIVACY_URL) : onRecheck()
                }
              >
                {micPerm === "denied" ? t("permOpenSettings") : t("recheck")}{" "}
                <span className="mk">&raquo;</span>
              </button>
            )}
          </div>
          <div className="drow">
            <span className="dk">{t("permSysAudio")}</span>
            <span
              className={`dpath ${sysPerm === "granted" ? "live" : ""}`}
              style={sysPerm === "denied" ? { color: "var(--warn)" } : undefined}
            >
              {permLabel(sysPerm)}
            </span>
            {sysPerm !== "granted" && (
              <button
                type="button"
                className="dopen"
                onClick={() =>
                  sysPerm === "denied"
                    ? openUrl(SYS_AUDIO_PRIVACY_URL)
                    : onRequestSystemAudio()
                }
              >
                {sysPerm === "denied" ? t("permOpenSettings") : t("permRequest")}{" "}
                <span className="mk">&raquo;</span>
              </button>
            )}
          </div>
        </>
      )}

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
