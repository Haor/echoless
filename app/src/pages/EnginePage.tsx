import { useEffect, useState } from "react";
import type {
  NvafxCheck,
  NvafxDoctor,
  NoiseSuppressionManifest,
  Platform,
  Processor,
} from "../types";
import {
  downloadLocalvqeModel,
  localvqeAssets,
  macSystemInfo,
  onLocalvqeProgress,
  openPath,
  type LocalvqeAssets,
  type MacSystemInfo,
} from "../api";
import { useI18n } from "../i18n";
import { createAsyncListenerScope } from "../asyncListener";

// macOS 主版本号 → 代号(sw_vers 只给版本号,代号本地映射)。落不到映射时只显版本号。
function macOsName(version?: string | null): string | null {
  if (!version) return null;
  const major = version.split(".")[0];
  const NAMES: Record<string, string> = {
    "26": "Tahoe",
    "15": "Sequoia",
    "14": "Sonoma",
    "13": "Ventura",
    "12": "Monterey",
    "11": "Big Sur",
  };
  const name = NAMES[major];
  return name ? `macOS ${name} (${version})` : `macOS ${version}`;
}

// Official LocalVQE models from HF. The default is recommended, not bundled.
// 行内只标参数量(体积不上行 —— 列宽预算有限,详情看 hover title)。
const LVQE_MODELS: {
  file: string;
  ver: string;
  params: string;
  descriptionKey: string;
}[] = [
  {
    file: "localvqe-v1.4-aec-200K-f32.gguf",
    ver: "v1.4",
    params: "200K",
    descriptionKey: "lvqeModel14Description",
  },
  {
    file: "localvqe-v1.3-4.8M-f32.gguf",
    ver: "v1.3",
    params: "4.8M",
    descriptionKey: "lvqeModel13Description",
  },
  {
    file: "localvqe-v1.2-1.3M-f32.gguf",
    ver: "v1.2",
    params: "1.3M",
    descriptionKey: "lvqeModel12Description",
  },
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
    kind: "aec3",
    name: "AEC3",
    tier: { en: "DEFAULT", zh: "默认" },
    echo: 9,
    voice: 6,
    cost: "CPU · webrtc",
    sr: "48k / 16k",
    os: "Win · mac · Linux",
  },
  {
    kind: "localvqe",
    name: "LOCALVQE",
    tier: { en: "EXPERIMENTAL", zh: "试验" },
    echo: 9,
    voice: 5,
    cost: "CPU · neural",
    sr: "48k (src) / 16k", // 模型原生 16k;48k 管线经 SRC 重采样进出(A6)
    os: "Win · mac · Linux",
  },
  {
    kind: "nvidia_afx_aec",
    name: "NVAFX",
    tier: { en: "ADVANCED", zh: "高级" },
    echo: 7,
    voice: 9,
    cost: "GPU · Tensor Core",
    sr: "48k / 16k",
    os: "Win only",
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
  noiseSuppression: NoiseSuppressionManifest | null;
  platform: Platform;
  kind: string;
  doctor: NvafxDoctor | null;
  dev: boolean;
  onSelect: (kind: string) => void;
  onPickModel: (path: string) => void;
  localvqeModel: string | null;
  onRecheck: () => void;
  onSetup: () => void;
}

// 本机系统信息整段会话不变:模块级缓存,避免每次进 Engine 页 NvafxCard 重挂载都
// 异步重取致「空 → 延迟 pop」的闪现。首取后缓存,重进用缓存同步初始化 state。
let macSysInfoCache: MacSystemInfo | null = null;

function NvafxCard({
  kind,
  doctor,
  dev,
  platform,
  nvSupported,
  nvReady,
  problems,
  onSelect,
  onRecheck,
  onSetup,
}: {
  kind: string;
  doctor: NvafxDoctor | null;
  dev: boolean;
  platform: Platform;
  nvSupported: boolean;
  nvReady: boolean;
  problems: number;
  onSelect: (kind: string) => void;
  onRecheck: () => void;
  onSetup: () => void;
}) {
  const { t, lang } = useI18n();
  const nv = doctor?.report;
  const active = kind === "nvidia_afx_aec";

  // 不可用态(仅 macOS)拉本机系统信息填充右栏。dev 模拟给一份样例。
  const [sysInfo, setSysInfo] = useState<MacSystemInfo | null>(macSysInfoCache);
  useEffect(() => {
    if (nvSupported || platform !== "macos") return;
    if (sysInfo) return; // 缓存命中,已同步渲染,无需再取
    if (dev) {
      const info: MacSystemInfo = {
        model: "MacBook Pro",
        os_version: "26.5.1",
        chip: "Apple M4",
        memory_gb: 24,
        cores: 10,
      };
      macSysInfoCache = info;
      setSysInfo(info);
      return;
    }
    macSystemInfo()
      .then((info) => {
        macSysInfoCache = info;
        setSysInfo(info);
      })
      .catch(() => {});
  }, [nvSupported, platform, dev, sysInfo]);

  return (
    <div
      className={`ecard wide ${active ? "active" : ""} ${nvSupported ? "" : "na"}`}
      role="button"
      tabIndex={nvSupported && !active ? 0 : -1}
      aria-pressed={active}
      aria-disabled={!nvSupported || active}
      onClick={() => nvSupported && !active && onSelect("nvidia_afx_aec")}
      onKeyDown={(e) => {
        if (
          nvSupported &&
          !active &&
          (e.key === "Enter" || e.key === " ")
        ) {
          e.preventDefault();
          onSelect("nvidia_afx_aec");
        }
      }}
    >
      <div className="eh">
        <span className="en">
          NVAFX <i className="sub">· RTX AEC</i>
        </span>
        <button
          type="button"
          className={`etag plainbtn ${nvReady ? "" : nvSupported ? "warn" : "na"}`}
          disabled={!nvSupported || active}
          aria-pressed={active}
          onClick={() => onSelect("nvidia_afx_aec")}
        >
          {active && <i className="dot" />}{" "}
          {dev && !doctor?.ok
            ? active
              ? `${t("active")} · DEV`
              : `${t("rdyReady")} · DEV`
            : !nvSupported
              ? t("windowsRtxOnly")
              : doctor?.ok
                ? active
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
          {/* Maxine SDK 许可要求:集成应用须在应用内做品牌归属(README/release 已有,
              这里是 UI 侧唯一归属点)。 */}
          <div className="epair">powered by NVIDIA Maxine</div>
          <div className="epair">{t("engPair")}</div>
        </div>
        <div className={`ecol nvcol ${nvSupported ? "" : "nvna"}`}>
          {!nvSupported ? (
            <div className="nvnainfo">
              {sysInfo?.model && (
                <div className="nvnarow">
                  <span className="nvnak">{t("nvnaModel")}</span>
                  <span className="nvnav">{sysInfo.model}</span>
                </div>
              )}
              {macOsName(sysInfo?.os_version) && (
                <div className="nvnarow">
                  <span className="nvnak">{t("nvnaOs")}</span>
                  <span className="nvnav">{macOsName(sysInfo?.os_version)}</span>
                </div>
              )}
              {sysInfo?.chip && (
                <div className="nvnarow">
                  <span className="nvnak">{t("nvnaChip")}</span>
                  <span className="nvnav">
                    {sysInfo.chip}
                    {sysInfo.cores ? ` · ${sysInfo.cores}${t("nvnaCoresSuffix")}` : ""}
                  </span>
                </div>
              )}
              {sysInfo?.memory_gb != null && (
                <div className="nvnarow">
                  <span className="nvnak">{t("nvnaMemory")}</span>
                  <span className="nvnav">{sysInfo.memory_gb} GB</span>
                </div>
              )}
              <div className="cdetail na">{t("engWinOnly")}</div>
            </div>
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
                <span className="dpath" title={nv?.runtime_dir}>
                  {nv?.runtime_dir || t("auto")}
                </span>
                <button
                  type="button"
                  className="dopen"
                  onClick={(e) => {
                    e.stopPropagation();
                    onRecheck();
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
  noiseSuppression,
  platform,
  kind,
  doctor,
  dev,
  onSelect,
  onPickModel,
  localvqeModel,
  onRecheck,
  onSetup,
}: Props) {
  const { t, lang } = useI18n();

  // Available LocalVQE models/native runtime.
  const [lvAssets, setLvAssets] = useState<LocalvqeAssets | null>(null);
  // 按文件名跟踪在下载的模型:键存在=下载中,值=百分比(null=还没进度)。
  // 单槽会串台——点了 B 就把 A 的按钮重新启用,允许对 A 发起第二次并发下载,
  // 两个下载写同一个 .part 互相踩,导致大小/SHA 不匹配。
  const [lvDl, setLvDl] = useState<Record<string, number | null>>({});
  const [lvErr, setLvErr] = useState<string | null>(null);
  useEffect(() => {
    localvqeAssets().then(setLvAssets).catch(() => {});
  }, []);
  // 下载进度事件:只更新仍在下载的文件的百分比。
  useEffect(() => {
    const listeners = createAsyncListenerScope();
    listeners.listen(onLocalvqeProgress, (p) => {
      setLvDl((cur) =>
        p.filename in cur ? { ...cur, [p.filename]: p.pct } : cur,
      );
    });
    return () => listeners.dispose();
  }, []);
  async function downloadModel(file: string) {
    // 同名下载进行中则忽略(按钮已 disabled,这里再兜一层快速双击竞态)。
    if (file in lvDl) return;
    setLvDl((cur) => ({ ...cur, [file]: null }));
    setLvErr(null);
    try {
      const path = await downloadLocalvqeModel(file);
      onPickModel(path);
      setLvAssets(await localvqeAssets());
    } catch (e) {
      setLvErr(String(e));
    } finally {
      setLvDl((cur) => {
        const next = { ...cur };
        delete next[file];
        return next;
      });
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
        const capability = noiseSuppression?.localvqe_models.find(
          (entry) => entry.file === m.file,
        )?.capability;
        const capabilityLabel =
          capability === "pure_aec"
            ? "AEC"
            : capability === "built_in_ns"
              ? "NS"
              : null;
        const found = lvAssets?.models.find((x) => x.filename === m.file);
        const selected = !!found && localvqeModel === found.path;
        const downloading = m.file in lvDl;
        const pct = lvDl[m.file];
        const box = downloading
          ? pct != null
            ? `${pct}%`
            : "···"
          : selected
            ? "✓"
            : found
              ? "OK"
              : t("lvqeGet");
        return (
          <button
            type="button"
            key={m.file}
            className={`lvmod ${selected ? "on" : found ? "have" : "miss"}`}
            disabled={downloading || selected}
            onClick={(e) => {
              e.stopPropagation();
              if (selected) return;
              found ? onPickModel(found.path) : downloadModel(m.file);
            }}
            title={`${m.ver} · ${m.params} · ${t(m.descriptionKey)}`}
          >
            <span className={`lvbox ${found ? "ok" : "miss"}`}>{box}</span>
            <span className="lvver">{m.ver}</span>
            {capabilityLabel && <i className="lvdef">{capabilityLabel}</i>}
            <span className="lvsp" />
            <span className="lvms">
              <span className="lvp">{m.params}</span>
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
      {/* native runtime 随包分发(2026-07-05 定案),正常永远就绪;
          这条 warn 只兜 dev 环境资源缺失的病态 case,不提供下载按钮 */}
      {lvAssets && !lvAssets.native_ready && (
        <div className="cdetail warn" title={lvAssets.native_dir ?? undefined}>
          {t("lvqeRuntimeMissing")}
        </div>
      )}
      {lvErr && (
        <div className="cdetail warn lverr" title={lvErr}>
          {lvErr}
        </div>
      )}
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
          role="button"
          tabIndex={sup && !active ? 0 : -1}
          aria-pressed={active}
          aria-disabled={!sup || active}
          onClick={() => sup && !active && onSelect(p.kind)}
          onKeyDown={(e) => {
            if (sup && !active && (e.key === "Enter" || e.key === " ")) {
              e.preventDefault();
              onSelect(p.kind);
            }
          }}
        >
          {body}
        </div>
      );
    }
    return (
      <button
        type="button"
        aria-pressed={active}
        disabled={!sup || active}
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
        doctor={doctor}
        dev={dev}
        platform={platform}
        nvSupported={nvSupported}
        nvReady={nvReady}
        problems={problems}
        onSelect={onSelect}
        onRecheck={onRecheck}
        onSetup={onSetup}
      />
      </div>

    </div>
  );
}
