import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  NvafxCheck,
  NvafxDoctor,
  Platform,
  Processor,
} from "../types";
import {
  downloadLocalvqeModel,
  localvqeAssets,
  openPath,
  type LocalvqeAssets,
} from "../api";
import { useI18n } from "../i18n";

// LocalVQE 官方模型(HF repo)。default=随 app 打包的默认模型。
const LVQE_MODELS: {
  file: string;
  ver: string;
  params: string;
  size: string;
  def?: boolean;
}[] = [
  { file: "localvqe-v1.3-4.8M-f32.gguf", ver: "v1.3", params: "4.8M", size: "~18 MB", def: true },
  { file: "localvqe-v1.2-1.3M-f32.gguf", ver: "v1.2", params: "1.3M", size: "~5 MB" },
  { file: "localvqe-v1.1-1.3M-f32.gguf", ver: "v1.1", params: "1.3M", size: "~5 MB" },
];

// 引擎能力画像(前端描述性数据,非配置 contract)。
//   echo  = 消回声强度    voice = 人声干净度(neural 优势)
// NV 的差异化是「人声最干净」而非「消回声最强」;NV 模型有 16k/48k,Echoless 当前跑 48k。
interface Profile {
  kind: string;
  name: string;
  tier: { en: string; zh: string };
  echo: number; // 0..10
  voice: number; // 0..10
  cost: string;
  sr: string;
  os: string;
}
const PROFILES: Profile[] = [
  {
    kind: "sonora_aec3",
    name: "AEC3",
    tier: { en: "DEFAULT", zh: "默认" },
    echo: 9,
    voice: 6,
    cost: "CPU · light",
    sr: "48k / 16k",
    os: "Win · mac",
  },
  {
    kind: "localvqe",
    name: "LOCALVQE",
    tier: { en: "EXPERIMENTAL", zh: "试验" },
    echo: 8,
    voice: 6,
    cost: "CPU · neural",
    sr: "16k only",
    os: "Win · mac",
  },
  {
    kind: "nvidia_afx_aec",
    name: "NVAFX",
    tier: { en: "CLEANEST VOICE", zh: "人声最干净" },
    echo: 7,
    voice: 10,
    cost: "GPU · Tensor Core",
    sr: "16k / 48k",
    os: "Win · only",
  },
];

function Meter({ label, n }: { label: string; n: number }) {
  return (
    <div className="emeter">
      <span className="el">{label}</span>
      <span className="ebar">
        {Array.from({ length: 10 }, (_, i) => (
          <i key={i} className={i < n ? "on" : ""} />
        ))}
      </span>
    </div>
  );
}

function checkPill(c: NvafxCheck) {
  return (
    <div className="echk" key={c.name}>
      <span className={`cpill ${c.status}`}>{c.status}</span>
      <span className="cname">{c.name}</span>
      <span className="cdetail" title={c.detail}>
        {c.detail}
      </span>
    </div>
  );
}

interface Props {
  processors: Processor[];
  platform: Platform;
  kind: string;
  params: Record<string, unknown>;
  doctor: NvafxDoctor | null;
  dev: boolean;
  onSelect: (kind: string) => void;
  onParam: (key: string, val: unknown) => void;
  onPickModel: (path: string) => void;
  localvqeModel: string | null;
  onRecheck: (runtimeDir?: string) => void;
  onSetup: () => void;
}

function NvafxCard({
  kind,
  params,
  doctor,
  dev,
  nvSupported,
  nvReady,
  problems,
  onSelect,
  onParam,
  onRecheck,
  onSetup,
}: {
  kind: string;
  params: Record<string, unknown>;
  doctor: NvafxDoctor | null;
  dev: boolean;
  nvSupported: boolean;
  nvReady: boolean;
  problems: number;
  onSelect: (kind: string) => void;
  onParam: (key: string, val: unknown) => void;
  onRecheck: (runtimeDir?: string) => void;
  onSetup: () => void;
}) {
  const { t, lang } = useI18n();
  const nv = doctor?.report;

  async function pickRuntime() {
    try {
      const sel = await open({ directory: true });
      if (typeof sel === "string") {
        onParam("runtime_dir", sel);
        onRecheck(sel);
      }
    } catch {
      /* cancelled */
    }
  }

  return (
    <div
      className={`ecard wide ${kind === "nvidia_afx_aec" ? "active" : ""} ${
        nvSupported ? "" : "na"
      }`}
    >
      <div className="eh">
        <span className="en">
          NVAFX <i className="sub">· RTX AEC</i>
        </span>
        <button
          type="button"
          className={`etag plainbtn ${nvReady ? "" : nvSupported ? "warn" : "na"}`}
          disabled={!nvSupported}
          aria-pressed={kind === "nvidia_afx_aec"}
          onClick={() => onSelect("nvidia_afx_aec")}
        >
          {kind === "nvidia_afx_aec" && <i className="dot" />}{" "}
          {dev && !doctor?.ok
            ? kind === "nvidia_afx_aec"
              ? `${t("active")} · DEV`
              : `${t("rdyReady")} · DEV`
            : !nvSupported
              ? "WINDOWS · RTX ONLY"
              : doctor?.ok
                ? kind === "nvidia_afx_aec"
                  ? t("active")
                  : t("rdyReady")
                : `${problems} ${t("rdyIssues")}`}
        </button>
      </div>
      <div className="etier">{PROFILES[2].tier[lang]}</div>
      <div className="ewrap">
        <div className="ecol">
          <Meter label="ECHO" n={PROFILES[2].echo} />
          <Meter label="VOICE" n={PROFILES[2].voice} />
          <div className="espec">
            <span>{PROFILES[2].cost}</span>
            <span className="sep">·</span>
            <span>{PROFILES[2].sr}</span>
          </div>
          <div className="espec os">{PROFILES[2].os}</div>
          <div className="epair">
            <span className="mk">»</span> {t("engPair")}
          </div>
        </div>
        <div className="ecol nvcol">
          {!nvSupported ? (
            <div className="cdetail na">{t("engWinOnly")}</div>
          ) : (
            <>
              <div className="nvgpu">
                {nv && nv.gpus.length > 0 ? (
                  <>
                    {nv.gpus[0].name}
                    <i>
                      {" "}
                      · {nv.gpus[0].driver_version}
                      {nv.selected_arch ? ` · ${nv.selected_arch}` : ""}
                    </i>
                  </>
                ) : (
                  <span className="cdetail na">{t("engNoGpu")}</span>
                )}
              </div>
              <div className="echks">{(nv?.checks ?? []).map(checkPill)}</div>
              <div className="drow nvrt">
                <span className="dk">RUNTIME</span>
                <button
                  type="button"
                  className="dpick plainbtn"
                  onClick={(e) => {
                    e.stopPropagation();
                    pickRuntime();
                  }}
                  title={(params.runtime_dir as string) || nv?.runtime_dir}
                >
                  {(params.runtime_dir as string) || nv?.runtime_dir || t("auto")}
                </button>
                <button
                  type="button"
                  className="dopen"
                  onClick={(e) => {
                    e.stopPropagation();
                    onRecheck((params.runtime_dir as string) || undefined);
                  }}
                >
                  {t("engRecheck")} <span className="mk">↻</span>
                </button>
                {!doctor?.ok && (
                  <button
                    type="button"
                    className="setupbtn"
                    onClick={(e) => {
                      e.stopPropagation();
                      onSetup();
                    }}
                  >
                    {t("engSetupRtx")} <span className="mk">&raquo;</span>
                  </button>
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

export function EnginePage({
  processors,
  platform,
  kind,
  params,
  doctor,
  dev,
  onSelect,
  onParam,
  onPickModel,
  localvqeModel,
  onRecheck,
  onSetup,
}: Props) {
  const { t, lang } = useI18n();

  // LocalVQE 可用模型(下载目录 + 打包资源);选中 localvqe 时拉取。
  const [lvAssets, setLvAssets] = useState<LocalvqeAssets | null>(null);
  const [lvDl, setLvDl] = useState<string | null>(null);
  const [lvErr, setLvErr] = useState<string | null>(null);
  useEffect(() => {
    localvqeAssets().then(setLvAssets).catch(() => {});
  }, []);
  async function downloadModel(file: string) {
    setLvDl(file);
    setLvErr(null);
    try {
      const path = await downloadLocalvqeModel(file);
      onPickModel(path);
      setLvAssets(await localvqeAssets());
    } catch (e) {
      setLvErr(String(e));
    } finally {
      setLvDl(null);
    }
  }

  const proc = (k: string) => processors.find((p) => p.kind === k);
  // 开发态(dev)临时解开 NVAFX 平台/doctor 门槛,用于走通前端流程。
  const supported = (k: string) =>
    dev || (proc(k)?.platforms.includes(platform) ?? true);
  // 就绪判定:AEC3 永远就绪;LocalVQE 需模型;NVAFX 需 doctor 通过(dev 跳过)。
  const ready = (k: string): boolean => {
    if (!supported(k)) return false;
    if (k === "localvqe") return Boolean(localvqeModel && lvAssets?.native_ready);
    if (k === "nvidia_afx_aec") return dev || Boolean(doctor?.ok);
    return true;
  };

  // LocalVQE 模型清单(卡片内,checklist 盒子风格):绿=已下载可用,黄=未下载可点下载。
  // 点模型 = 选 LocalVQE 引擎 + 设该模型(onPickModel 原子处理),清单常驻不展开。
  const localvqeModels = () => (
    <div className="lvmods">
      {LVQE_MODELS.map((m) => {
        const found = lvAssets?.models.find((x) => x.filename === m.file);
        const selected = !!found && localvqeModel === found.path;
        const downloading = lvDl === m.file;
        const box = downloading ? "···" : selected ? "✓" : found ? "OK" : t("lvqeGet");
        return (
          <button
            type="button"
            key={m.file}
            className={`lvmod ${selected ? "on" : found ? "have" : "miss"}`}
            disabled={downloading}
            onClick={(e) => {
              e.stopPropagation();
              found ? onPickModel(found.path) : downloadModel(m.file);
            }}
            title={found ? found.path : `${t("lvqeDownload")} · ${m.file}`}
          >
            <span className={`lvbox ${found ? "ok" : "miss"}`}>{box}</span>
            <span className="lvver">{m.ver}</span>
            {m.def && <i className="lvdef">{t("lvqeDefault")}</i>}
            <span className="lvsp" />
            <span className="lvms">
              <span className="lvp">{m.params}</span>
              <span className="lvsep">·</span>
              <span className="lvz">{m.size}</span>
            </span>
          </button>
        );
      })}
      <div className="lvtools">
        <button
          type="button"
          className="dopen"
          onClick={(e) => {
            e.stopPropagation();
            if (lvAssets) openPath(lvAssets.models_dir);
          }}
          title={lvAssets?.models_dir}
        >
          {t("lvqeOpenDir")} <span className="mk">↗</span>
        </button>
      </div>
      {lvAssets && !lvAssets.native_ready && (
        <div className="cdetail warn" title={lvAssets.native_dir ?? undefined}>
          {t("lvqeRuntimeMissing")}
        </div>
      )}
      {lvErr && <div className="cdetail warn">{lvErr}</div>}
    </div>
  );

  const card = (p: Profile) => {
    const sup = supported(p.kind);
    const active = kind === p.kind;
    const rdy = ready(p.kind);
    const isLvqe = p.kind === "localvqe";
    const status = !sup
      ? "UNAVAILABLE"
      : rdy
        ? active
          ? t("active")
          : t("rdyReady")
        : t("rdySetup");
    const meters = (
      <>
        <Meter label="ECHO" n={p.echo} />
        <Meter label="VOICE" n={p.voice} />
        <div className="espec">
          <span>{p.cost}</span>
          <span className="sep">·</span>
          <span>{p.sr}</span>
        </div>
        <div className="espec os">{p.os}</div>
      </>
    );
    const body = (
      <>
        <div className="eh">
          <span className="en">{p.name}</span>
          <span className={`etag ${rdy ? "" : sup ? "warn" : "na"}`}>
            {active && <i className="dot" />} {status}
          </span>
        </div>
        <div className="etier">{p.tier[lang]}</div>
        {isLvqe ? (
          <div className="ewrap">
            <div className="ecol">{meters}</div>
            <div className="ecol nvcol lvcol">{localvqeModels()}</div>
          </div>
        ) : (
          meters
        )}
      </>
    );
    if (isLvqe) {
      return (
        <div
          className={`ecard ${active ? "active" : ""} ${sup ? "" : "na"} lvwide`}
        >
          {body}
        </div>
      );
    }
    return (
      <button
        type="button"
        aria-pressed={active}
        disabled={!sup}
        className={`ecard cardbtn ${active ? "active" : ""} ${sup ? "" : "na"}`}
        onClick={() => onSelect(p.kind)}
      >
        {body}
      </button>
    );
  };

  const nv = doctor?.report;
  const nvSupported = supported("nvidia_afx_aec");
  const nvReady = dev || Boolean(doctor?.ok);
  const problems = (nv?.checks ?? []).filter(
    (c) => c.status === "missing" || c.status === "unsupported",
  ).length;

  return (
    <div className="page engine">
      <div className="kick">
        <span className="d">
          <i />
          <i />
          <i />
        </span>{" "}
        {t("engNote")}
      </div>
      <hr className="hair" />

      {/* 卡片区在 kick/分隔线之下的剩余空间里垂直居中(上下留白相等) */}
      <div className="enbody">
      <div className="ecards">
        {card(PROFILES[0])}
        {card(PROFILES[1])}
      </div>

      <NvafxCard
        kind={kind}
        params={params}
        doctor={doctor}
        dev={dev}
        nvSupported={nvSupported}
        nvReady={nvReady}
        problems={problems}
        onSelect={onSelect}
        onParam={onParam}
        onRecheck={onRecheck}
        onSetup={onSetup}
      />
      </div>

    </div>
  );
}
