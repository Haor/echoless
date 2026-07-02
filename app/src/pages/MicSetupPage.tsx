import { useState } from "react";
import type { DoctorAudio, DoctorCandidate, Platform } from "../types";
import { openUrl } from "../api";
import { useI18n } from "../i18n";
import { MIC_DEV_STATES, type MicState } from "../mic";

// 虚拟麦克风路由诊断/向导。后端 doctor audio 缺增强字段时,从现有字段派生。
// 路由模型:Echoless 输出 → 虚拟设备 input 端(如 CABLE Input);
//          虚拟设备 output 端(如 CABLE Output / BlackHole)→ 通话软件选作 mic。

const DRIVER_URL: Record<string, string> = {
  "vb-cable": "https://vb-audio.com/Cable/",
  "vb-cable-mac": "https://vb-audio.com/Cable/",
  "blackhole-2ch": "https://github.com/ExistentialAudio/BlackHole",
};
const MIC_PRIVACY_URL =
  "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
// mac 上路由只露一端,多半是 CoreAudio 没刷新 —— 重启它再 recheck。
const MAC_RESTART_CMD = "sudo killall coreaudiod";

interface Props {
  doctor: DoctorAudio | null;
  platform: Platform;
  dev: boolean;
  devState: MicState;
  onDevState: (s: MicState) => void;
  onBack: () => void;
  onRecheck: () => void;
}

function pickOutput(d: DoctorAudio): DoctorCandidate | null {
  if (d.recommended_output) return d.recommended_output;
  const outs = d.candidate_outputs ?? [];
  return outs.find((o) => /cable input/i.test(o.name)) ?? outs[0] ?? null;
}
function pickAppMic(d: DoctorAudio): DoctorCandidate | null {
  if (d.recommended_app_mic) return d.recommended_app_mic;
  const ins = d.candidate_inputs ?? [];
  return ins.find((i) => /cable output|blackhole/i.test(i.name)) ?? ins[0] ?? null;
}

export function MicSetupPage({
  doctor,
  platform,
  dev,
  devState,
  onDevState,
  onRecheck,
}: Props) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);

  function copyCmd() {
    navigator.clipboard
      ?.writeText(MAC_RESTART_CMD)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      })
      .catch(() => {});
  }

  const driver = doctor?.recommended_driver ?? "vb-cable";
  const outDev = doctor ? pickOutput(doctor) : null;
  const micDev = doctor ? pickAppMic(doctor) : null;

  const routeReady =
    doctor?.virtual_route_ready ??
    ((doctor?.candidate_outputs.length ?? 0) > 0 &&
      (doctor?.candidate_inputs.length ?? 0) > 0);
  const installed = doctor
    ? doctor.install_status !== "missing" ||
      (doctor.candidate_outputs.length + doctor.candidate_inputs.length > 0)
    : false;
  const isMac = platform === "macos";
  const permDenied = isMac && doctor?.permission_state === "denied";
  const permUndet = isMac && doctor?.permission_state === "undetermined";

  const state: MicState = !doctor
    ? "unknown"
    : !installed
      ? "missing"
      : !routeReady
        ? "incomplete"
        : permDenied
          ? "permission"
          : "ready";

  // 阶梯节点状态
  const node = (key: "driver" | "route" | "perm" | "ready"): string => {
    const order = ["driver", "route", "perm", "ready"];
    const idx = { missing: 0, incomplete: 1, permission: 2, ready: 3, unknown: 0 }[
      state
    ];
    const i = order.indexOf(key);
    if (state === "ready") return "ok";
    if (i < idx) return "ok";
    if (i === idx) return "active";
    return "pending";
  };

  function action() {
    if (state === "ready") {
      return (
        <div className="wzcard">
          <div className="wzh ok">✓ {t("micReady")}</div>
          <div className="wznote">
            {t("micPickInApp")}{" "}
            <b style={{ color: "var(--live)" }}>{micDev?.name ?? "—"}</b>
          </div>
          {permUndet && <div className="wznote">{t("micPermUndet")}</div>}
        </div>
      );
    }
    if (state === "permission") {
      return (
        <div className="wzcard">
          <div className="wzh warn">{t("micPermDenied")}</div>
          <div className="wznote">{t("micPermHint")}</div>
          <div className="wzgo">
            <button
              type="button"
              className="wzbtn"
              onClick={() => openUrl(MIC_PRIVACY_URL)}
            >
              {t("micOpenPrivacy")} <span className="mk">↗</span>
            </button>
            <button type="button" className="dopen" onClick={onRecheck}>
              {t("recheck")} <span className="mk">↻</span>
            </button>
          </div>
        </div>
      );
    }
    if (state === "incomplete") {
      return (
        <div className="wzcard">
          <div className="wzh warn">{t("micIncomplete")}</div>
          <div className="wznote">
            {isMac
              ? t("micMacRestartHint")
              : doctor?.needs_reboot
                ? t("micReboot")
                : t("micIncompleteHint")}
          </div>
          {isMac && (
            <div className="wzcmd">
              <code>{MAC_RESTART_CMD}</code>
              <button type="button" className="dopen" onClick={copyCmd}>
                {copied ? t("micCopied") : t("micCopy")}{" "}
                <span className="mk">⧉</span>
              </button>
            </div>
          )}
          <div className="wzgo">
            <button
              type="button"
              className="wzbtn"
              onClick={() => openUrl(DRIVER_URL[driver] ?? DRIVER_URL["vb-cable"])}
            >
              {t("micOpenDriver")} <span className="mk">↗</span>
            </button>
            <button type="button" className="dopen" onClick={onRecheck}>
              {t("recheck")} <span className="mk">↻</span>
            </button>
          </div>
        </div>
      );
    }
    // missing / unknown
    return (
      <div className="wzcard">
        <div className="wzh warn">{t("micMissing")}</div>
        <div className="wznote">
          {t("micInstallHint")} <b>{driver}</b>
          {doctor?.needs_reboot ? ` · ${t("micRebootAfter")}` : ""}
        </div>
        <div className="wzgo">
          <button
            type="button"
            className="wzbtn"
            onClick={() => openUrl(DRIVER_URL[driver] ?? DRIVER_URL["vb-cable"])}
          >
            {t("micOpenDriver")} <span className="mk">↗</span>
          </button>
          <button type="button" className="dopen" onClick={onRecheck}>
            {t("recheck")} <span className="mk">↻</span>
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="page wz">
      <div className="kick">
        <span className="d">
          <i />
          <i />
          <i />
        </span>{" "}
        <span className="slashText">{t("micRouteHead")} · {driver}</span>
      </div>
      <hr className="hair" />

      {dev && (
        <div className="devbar">
          <span className="dvk">DEV · simulate</span>
          {MIC_DEV_STATES.map((s) => (
            <button
              type="button"
              key={s}
              className={`dvb ${devState === s ? "on" : ""}`}
              onClick={() => onDevState(s)}
            >
              {s}
            </button>
          ))}
        </div>
      )}

      {/* 路由图:核心诊断 —— 谁连谁、通话软件选哪个 */}
      <div className="asec">{t("micRoute")}</div>
      <div className="mroute">
        <div className="mrow">
          <span className="mfrom">Echoless {t("micOut")}</span>
          <span className="marrow">→</span>
          <span className="mdev">{outDev?.name ?? "—"}</span>
          <span className="mnote">{t("micSetAsOutput")}</span>
        </div>
        <div className="mrow">
          <span className="mfrom">{micDev?.name ?? "—"}</span>
          <span className="marrow">→</span>
          <span className="mdev">{t("micCallApp")}</span>
          <span className="mnote hl">{t("micPickHere")}</span>
        </div>
      </div>

      <div className="asec">{t("wzReadiness")}</div>
      <div className="wzladder">
        <span className={`wznode ${node("driver")}`}>
          <i className="d" />
          {t("micNodeDriver")}
        </span>
        <span className={`wznode ${node("route")}`}>
          <i className="d" />
          {t("micNodeRoute")}
        </span>
        <span className={`wznode ${node("perm")}`}>
          <i className="d" />
          {t("micNodePerm")}
        </span>
        <span className={`wznode ${node("ready")}`}>
          <i className="d" />
          ready
        </span>
      </div>

      <div className="asec">{t("wzAction")}</div>
      {action()}
    </div>
  );
}
