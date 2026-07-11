import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { NvafxDoctor } from "../types";
import { openUrl } from "../api";
import { useI18n } from "../i18n";
import { SegButtons } from "../components/Controls";
import { Hint } from "../components/Hint";
import {
  deriveRtxState,
  ladderStatus,
  nvafxAssetUrl,
  nvafxModelAsset,
  NVAFX_COMMON_ASSET,
  RTX_DEV_STATES,
  RTX_HARD_BLOCK,
  RTX_LADDER,
  type LadderKey,
  type RtxState,
} from "../nvafx";

const DRIVER_URL = "https://www.nvidia.com/Download/index.aspx";
const VC_URL = "https://aka.ms/vs/17/release/vc_redist.x64.exe";

const base = (p: string) => p.split(/[\\/]/).pop() ?? p;

const LADDER_LABEL: Record<LadderKey, string> = {
  platform: "platform",
  driver: "driver",
  gpu: "gpu",
  "vc++": "vc++",
  runtime: "runtime",
  model: "model",
  ready: "ready",
};

async function pickZip(set: (v: string) => void) {
  try {
    const sel = await open({
      directory: false,
      filters: [{ name: "zip", extensions: ["zip"] }],
    });
    if (typeof sel === "string") set(sel);
  } catch {
    /* cancelled */
  }
}

interface Props {
  doctor: NvafxDoctor | null;
  busy: boolean;
  pct?: number | null;
  stage?: "runtime" | "model" | null;
  recv?: number | null;
  dev: boolean;
  devState: RtxState;
  onDevState: (s: RtxState) => void;
  onRecheck: () => void;
  onInstall: (commonZip: string, modelZip: string) => void;
  onDownloadInstall: () => void;
  onUse: () => void;
}

export function RtxSetupPage({
  doctor,
  busy,
  pct,
  stage,
  recv,
  dev,
  devState,
  onDevState,
  onRecheck,
  onInstall,
  onDownloadInstall,
  onUse,
}: Props) {
  const { t } = useI18n();
  // 默认落在 DOWNLOAD:多数用户直接从 GitHub public release 拉取(自动匹配 GPU);
  // LOCAL ZIP 是已手动下好包的备用路径。
  const [source, setSource] = useState<"local" | "download">("download");
  const [commonZip, setCommonZip] = useState("");
  const [modelZip, setModelZip] = useState("");

  const state = deriveRtxState(doctor);
  const ladder = ladderStatus(state);
  const rep = doctor?.report;
  const gpu = rep?.gpus[0];
  const arch = rep?.selected_arch ?? null;
  const runtimeDir = rep?.runtime_dir ?? "—";

  const mismatch =
    !!modelZip && !!arch && !base(modelZip).toLowerCase().includes(arch);
  // dev 模拟可不选 zip 直接走安装。
  const canInstall = (dev || (!!commonZip && !!modelZip)) && !busy;

  function fixCheckDetail(): string {
    // 取该状态对应 check 的 detail 作为说明(诚实显示后端原因)。
    const c = rep?.checks ?? [];
    const find = (pred: (n: string) => boolean) =>
      c.find((x) => pred(x.name) && (x.status === "missing" || x.status === "unsupported"));
    if (state === "missing_driver")
      return (find((n) => n === "nvidia-smi" || n === "nvcuda.dll" || n === "gpu")?.detail) ?? "";
    if (state === "driver_too_old")
      return (find((n) => n.endsWith(":driver"))?.detail) ?? "";
    if (state === "missing_vc_redist")
      return (find((n) => n.startsWith("vc-runtime:"))?.detail) ?? "";
    if (state === "unsupported_gpu")
      return (find((n) => n.endsWith(":arch"))?.detail) ?? "";
    return "";
  }

  function action() {
    if (RTX_HARD_BLOCK.includes(state)) {
      return (
        <div className="wzcard">
          <div className="wzh warn">
            {state === "unsupported_gpu" ? t("stUnsupportedGpu") : t("stUnsupportedPlatform")}
          </div>
          {fixCheckDetail() && <div className="wznote">{fixCheckDetail()}</div>}
          <div className="wznote">{t("wzHardBlock")}</div>
        </div>
      );
    }
    if (state === "missing_driver" || state === "driver_too_old") {
      return (
        <div className="wzcard">
          <div className="wzh warn">
            {state === "driver_too_old" ? t("stDriverTooOld") : t("stMissingDriver")}
          </div>
          {fixCheckDetail() && <div className="wznote">{fixCheckDetail()}</div>}
          <div className="wzgo">
            <button type="button" className="wzbtn" onClick={() => openUrl(DRIVER_URL)}>
              {t("wzOpenDriver")} <span className="mk">↗</span>
            </button>
            <button type="button" className="dopen" onClick={onRecheck}>
              {t("recheck")} <span className="mk">↻</span>
            </button>
          </div>
        </div>
      );
    }
    if (state === "missing_vc_redist") {
      return (
        <div className="wzcard">
          <div className="wzh warn">{t("stMissingVc")}</div>
          {fixCheckDetail() && <div className="wznote">{fixCheckDetail()}</div>}
          <div className="wzgo">
            <button type="button" className="wzbtn" onClick={() => openUrl(VC_URL)}>
              {t("wzOpenVc")} <span className="mk">↗</span>
            </button>
            <button type="button" className="dopen" onClick={onRecheck}>
              {t("recheck")} <span className="mk">↻</span>
            </button>
          </div>
        </div>
      );
    }
    if (state === "ready") {
      return (
        <div className="wzcard">
          <div className="wzh ok">✓ {t("stReady")}</div>
          <div className="wzgo">
            <button type="button" className="wzbtn ok" onClick={onUse}>
              {t("wzUseEngine")} <span className="mk">»</span>
            </button>
          </div>
        </div>
      );
    }
    // RTX_INSTALL:runtime_not_installed / model_not_installed
    return (
      <div className="wzcard">
        <div className="wzh">
          {state === "model_not_installed" ? t("stModelMissing") : t("stRuntimeMissing")}
        </div>
        <div className="wznote">{t("wzInstallSize")}</div>
        <div className="wzsrc">
          <span className="wlbl">{t("wzSource")}</span>
          <SegButtons
            value={source}
            options={[
              { value: "local", label: t("wzLocalZip").toUpperCase() },
              { value: "download", label: t("wzDownload").toUpperCase() },
            ]}
            onChange={(v) => setSource(v as "local" | "download")}
          />
        </div>
        {source === "local" ? (
          <>
            <div className="drow">
              <span className="dk">{t("wzCommon")}</span>
              <button
                type="button"
                className="dpick plainbtn"
                onClick={() => pickZip(setCommonZip)}
                title={commonZip}
              >
                {commonZip ? base(commonZip) : t("wzPickZip")}
              </button>
            </div>
            <div className="drow">
              <span className="dk">{t("wzModel")}</span>
              <span className="wv">
                {t("wzAutoArch")} → {arch ?? "?"}
              </span>
              <button
                type="button"
                className="dpick plainbtn"
                onClick={() => pickZip(setModelZip)}
                title={modelZip}
              >
                {modelZip ? base(modelZip) : t("wzPickZip")}
              </button>
              {mismatch && <span className="cdetail warn">{t("wzArchMismatch")}</span>}
            </div>
            <div className="wzgo">
              {busy ? (
                <span className="wzbusy">{t("wzInstalling")}</span>
              ) : (
                <button
                  type="button"
                  className="wzbtn"
                  disabled={!canInstall}
                  onClick={() => onInstall(commonZip, modelZip)}
                >
                  {t("wzInstall")} <span className="mk">»</span>
                </button>
              )}
            </div>
          </>
        ) : (
          <>
            <div className="wznote">{t("wzDownloadSrc")}</div>
            <div className="wzassets">
              <div>
                <span className="dk">{t("wzCommon")}</span>{" "}
                <Hint text={t("wzAssetDownload")} attach>
                  <button
                    type="button"
                    className="wzasset plainbtn"
                    onClick={() => openUrl(nvafxAssetUrl(NVAFX_COMMON_ASSET))}
                  >
                    {NVAFX_COMMON_ASSET}
                  </button>
                </Hint>
                <span className="sz"> · 955 MiB</span>
              </div>
              <div>
                <span className="dk">{t("wzModel")}</span>{" "}
                {arch ? (
                  <>
                    <Hint text={t("wzAssetDownload")} attach>
                      <button
                        type="button"
                        className="wzasset plainbtn"
                        onClick={() => openUrl(nvafxAssetUrl(nvafxModelAsset(arch)))}
                      >
                        {nvafxModelAsset(arch)}
                      </button>
                    </Hint>
                    <span className="sz"> · 46 MiB</span>
                  </>
                ) : (
                  t("wzNoGpuArch")
                )}
              </div>
            </div>
            <div className="wzgo">
              {busy ? (
                <span className="wzbusy">
                  {stage != null
                    ? `${t("wzDl")} · ${stage === "model" ? t("wzModel") : t("wzCommon")}${
                        pct != null
                          ? ` ${pct}%`
                          : recv != null && recv > 0
                            ? ` ${(recv / 1048576).toFixed(1)} MiB`
                            : ""
                      }`
                    : t("wzDownloading")}
                </span>
              ) : (
                <button
                  type="button"
                  className="wzbtn"
                  disabled={!arch || busy}
                  onClick={onDownloadInstall}
                >
                  {t("wzDownloadInstall")} <span className="mk">»</span>
                </button>
              )}
            </div>
          </>
        )}
      </div>
    );
  }

  const driverOk = ladder.driver === "ok";

  return (
    <div className="page wz">
      <div className="kick">
        <span className="d">
          <i />
          <i />
          <i />
        </span>{" "}
        <span className="slashText">RTX AEC RUNTIME · AFX SDK 2.1.0 · win64 · aec48</span>
      </div>
      {dev && (
        <div className="devbar">
          <span className="dvk">DEV · simulate</span>
          {RTX_DEV_STATES.map((s) => (
            <button
              type="button"
              key={s}
              className={`dvb ${devState === s ? "on" : ""}`}
              onClick={() => onDevState(s)}
            >
              {s.replace(/_/g, " ")}
            </button>
          ))}
        </div>
      )}
      <hr className="hair" />

      <div className="asec">{t("wzSystem")}</div>
      <div className="wzsys">
        <div className="drow">
          <span className="dk">{t("wzGpu")}</span>
          <span className="dpath">
            {gpu
              ? `${gpu.name} · ${arch ?? "—"} · cc ${gpu.compute_capability}`
              : t("engNoGpu")}
          </span>
        </div>
        <div className="drow">
          <span className="dk">{t("wzDriver")}</span>
          <span className="dpath">
            {gpu ? gpu.driver_version : "—"}
            {gpu && (
              <i className={driverOk ? "okmk" : "warnmk"}>
                {driverOk ? "  ✓ ≥ 572.61" : "  ⚠ < 572.61"}
              </i>
            )}
          </span>
        </div>
        <div className="drow">
          <span className="dk">{t("wzRuntime")}</span>
          <span className="dpath" title={runtimeDir}>
            {runtimeDir}
          </span>
          <button type="button" className="dopen" onClick={onRecheck}>
            {t("recheck")} <span className="mk">↻</span>
          </button>
        </div>
      </div>

      <div className="asec">{t("wzReadiness")}</div>
      <div className="wzladder">
        {RTX_LADDER.map((k) => (
          <span key={k} className={`wznode ${ladder[k]}`}>
            <i className="d" />
            {LADDER_LABEL[k]}
          </span>
        ))}
      </div>

      <div className="asec">{t("wzAction")}</div>
      {action()}
    </div>
  );
}
