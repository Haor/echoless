import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export type Lang = "en" | "zh";

// 文案字典。技术标识(设备名/参数键/MIC·REF·OUT·dBFS/ON·OFF/采样率数字)保留原文。
const D: Record<string, { en: string; zh: string }> = {
  overview: { en: "Overview", zh: "总览" },
  engine: { en: "Engine", zh: "引擎" },
  advanced: { en: "Advanced", zh: "高级" },
  diagnostics: { en: "Diagnostics", zh: "诊断" },

  kicker: {
    en: "Acoustic Echo Cancellation",
    zh: "声学回声消除",
  },

  removingEcho: { en: "Removing Echo", zh: "正在消除回声" },
  echoStopped: { en: "Echo Stopped", zh: "已停止" },
  unstable: { en: "Unstable", zh: "不稳定" },
  noReference: { en: "No Reference", zh: "无参考信号" },
  volWheel: {
    en: "scroll to adjust output volume",
    zh: "滚轮调节输出音量",
  },
  latency: { en: "Pipeline", zh: "管线延迟" },
  ms: { en: "MS", zh: "毫秒" },
  stable: { en: "Stable", zh: "稳定" },
  checkSetup: { en: "Check Setup", zh: "检查设置" },

  input: { en: "Input", zh: "输入" },
  model: { en: "Model", zh: "模型" },
  output: { en: "Output", zh: "输出" },
  noise: { en: "Noise", zh: "降噪" },
  // 术语保留英文(近端/参考 译成中文反而怪)。
  micNearEnd: { en: "Microphone · Near-end", zh: "Microphone · Near-end" },
  reference: { en: "Reference", zh: "Reference" },
  noLoopback: { en: "No Loopback", zh: "No Loopback" },
  installCable: { en: "install virtual cable", zh: "安装虚拟声卡" },
  reduceNoise: { en: "Reduce background noise", zh: "抑制背景噪声" },

  signal: { en: "Signal", zh: "Signal" },
  sigFlow: {
    en: "Near-end Mic + Ref » Clean Output",
    zh: "Near-end Mic + Ref » Clean Output",
  },

  backToOverview: { en: "Overview", zh: "返回总览" },

  // 系统音频录制权限(mac Process Tap reference)
  sysAudioGrant: { en: "grant system audio permission", zh: "授予系统音频权限" },
  sysAudioFirstRun: {
    en: "first run will request system audio permission",
    zh: "首次运行将请求系统音频权限",
  },
  sysAudioRequest: {
    en: "request system audio permission",
    zh: "请求系统音频权限",
  },
  // 系统音频参考(Process Tap)要求全局采样率 = 48k(与引擎无关)。
  sysRefRate: {
    en: "System Audio ref needs 48k sample rate",
    zh: "系统音频参考需 48k 采样率",
  },

  // Engine
  engNote: {
    en: "Engine selection",
    zh: "引擎选择",
  },
  active: { en: "ACTIVE", zh: "运行中" },
  rdyReady: { en: "READY", zh: "就绪" },
  rdySetup: { en: "SET UP", zh: "待配置" },
  rdyIssues: { en: "ISSUES", zh: "项待处理" },
  engPair: {
    en: "pair with NVIDIA Broadcast for residual noise",
    zh: "建议后接 NVIDIA Broadcast 消残留噪声",
  },
  engWinOnly: {
    en: "Windows + RTX GPU only · unavailable on this OS",
    zh: "仅 Windows + RTX 显卡 · 当前系统不可用",
  },
  engNoGpu: { en: "no NVIDIA GPU detected", zh: "未检测到 NVIDIA GPU" },
  engRecheck: { en: "recheck", zh: "重检" },
  engPickModel: { en: "pick .gguf model…", zh: "选择 .gguf 模型…" },
  engModelReq: { en: "model required", zh: "需要模型文件" },
  // LocalVQE 模型列表
  lvqeDefault: { en: "DEFAULT", zh: "默认" },
  lvqeDownload: { en: "download", zh: "下载" },
  lvqeDownloading: { en: "downloading…", zh: "下载中…" },
  lvqeUse: { en: "use", zh: "使用" },
  lvqeInUse: { en: "in use", zh: "使用中" },
  lvqeCustom: { en: "pick local .gguf…", zh: "选本地 .gguf…" },
  lvqeGet: { en: "GET", zh: "下载" },
  lvqeOpenDir: { en: "open model folder", zh: "打开模型目录" },
  lvqeSource: { en: "official repo", zh: "官方 repo" },
  lvqeRuntimeMissing: {
    en: "native runtime missing",
    zh: "缺少原生运行库",
  },
  engSetupHint: { en: "set up in Engine", zh: "去 Engine 配置" },
  engSetupRtx: { en: "set up RTX", zh: "配置 RTX" },

  // RTX Setup 向导
  rtxSetup: { en: "RTX SETUP", zh: "RTX 配置" },
  back: { en: "back", zh: "返回" },
  wzSystem: { en: "System", zh: "系统" },
  wzReadiness: { en: "Readiness", zh: "就绪进度" },
  wzAction: { en: "Action", zh: "操作" },
  wzGpu: { en: "GPU", zh: "GPU" },
  wzDriver: { en: "Driver", zh: "驱动" },
  wzRuntime: { en: "Runtime", zh: "运行时" },
  recheck: { en: "recheck", zh: "重检" },
  // 状态标题 / 说明
  stUnsupportedPlatform: { en: "Unavailable on this OS", zh: "当前系统不可用" },
  stUnsupportedGpu: { en: "GPU not supported", zh: "显卡不受支持" },
  stMissingDriver: { en: "NVIDIA driver required", zh: "需要 NVIDIA 驱动" },
  stDriverTooOld: { en: "Driver too old", zh: "驱动版本过旧" },
  stMissingVc: { en: "VC++ runtime required", zh: "需要 VC++ 运行库" },
  stRuntimeMissing: { en: "Install RTX runtime", zh: "安装 RTX 运行时" },
  stModelMissing: { en: "Install RTX model", zh: "安装 RTX 模型" },
  stReady: { en: "RTX AEC ready", zh: "RTX AEC 就绪" },
  wzHardBlock: {
    en: "RTX AEC needs Windows + an RTX / Tensor-Core GPU (Turing / Ampere / Ada / Blackwell).",
    zh: "RTX AEC 需要 Windows + RTX / Tensor Core 显卡(Turing / Ampere / Ada / Blackwell)。",
  },
  wzOpenDriver: { en: "open NVIDIA drivers", zh: "打开 NVIDIA 驱动下载" },
  wzOpenVc: { en: "open VC++ redistributable", zh: "打开 VC++ 运行库下载" },
  wzInstallTitle: { en: "Install RTX AEC runtime", zh: "安装 RTX AEC 运行时" },
  wzInstallSize: {
    en: "runtime ~1 GB + model · extracted via Echoless CLI",
    zh: "运行时约 1 GB + 模型 · 由 Echoless CLI 解压",
  },
  wzSource: { en: "Source", zh: "来源" },
  wzLocalZip: { en: "Local zip", zh: "本地 zip" },
  wzDownload: { en: "Download", zh: "下载" },
  wzCommon: { en: "common runtime", zh: "公共运行时" },
  wzModel: { en: "model", zh: "模型" },
  wzPickZip: { en: "pick .zip…", zh: "选择 .zip…" },
  wzAutoArch: { en: "auto", zh: "自动" },
  wzArchMismatch: { en: "zip name does not match", zh: "zip 文件名与架构不符" },
  wzInstall: { en: "install", zh: "安装" },
  wzInstalling: { en: "installing… extracting, may take a minute", zh: "安装中… 解压中,可能需要一会" },
  wzDownloadSrc: {
    en: "from GitHub public release · auto-matches your GPU model",
    zh: "来自 GitHub 公共 release · 自动匹配你的 GPU 模型",
  },
  wzDownloadInstall: { en: "download & install", zh: "下载并安装" },
  wzDownloading: { en: "downloading… ~1 GB, may take a while", zh: "下载中… 约 1 GB,可能需要一会" },
  wzUseEngine: { en: "use this engine", zh: "使用该引擎" },
  wzNoGpuArch: { en: "fix GPU / driver detection first", zh: "请先修复 GPU / 驱动检测" },

  // 虚拟麦克风诊断 / 向导
  micSetup: { en: "MIC SETUP", zh: "虚拟麦配置" },
  virtualMic: { en: "Virtual Mic", zh: "虚拟麦克风" },
  setupBtn: { en: "set up", zh: "配置" },
  micRouteHead: { en: "VIRTUAL MIC ROUTE", zh: "虚拟麦路由" },
  micRoute: { en: "Route", zh: "路由" },
  micOut: { en: "out", zh: "输出" },
  micCallApp: { en: "call app mic", zh: "通话软件麦克风" },
  micSetAsOutput: { en: "set as Echoless output", zh: "设为 Echoless 输出" },
  micPickHere: { en: "pick this in your call app", zh: "在通话软件里选这个" },
  micNodeDriver: { en: "driver", zh: "驱动" },
  micNodeRoute: { en: "route", zh: "路由" },
  micNodePerm: { en: "mic perm", zh: "麦权限" },
  // 状态摘要(Diagnostics 行)
  micReadyShort: { en: "route ready", zh: "路由就绪" },
  micSetupShort: { en: "set up", zh: "待配置" },
  micPickShort: { en: "in app mic", zh: "通话软件麦选" },
  // 动作卡
  micReady: { en: "Virtual mic ready", zh: "虚拟麦已就绪" },
  micPickInApp: { en: "In your call app pick mic:", zh: "在通话软件里把麦克风选成:" },
  micMissing: { en: "Virtual audio not installed", zh: "未安装虚拟声卡" },
  micInstallHint: { en: "Install a virtual audio device:", zh: "安装一个虚拟声卡:" },
  micRebootAfter: { en: "reboot after install", zh: "装完需重启" },
  micIncomplete: { en: "Route incomplete", zh: "路由不完整" },
  micIncompleteHint: {
    en: "Only one side detected. Reopen the driver installer or finish setup.",
    zh: "只检测到一端。重开驱动安装器或完成安装。",
  },
  micMacRestartHint: {
    en: "Only one side detected. Restart CoreAudio, then recheck:",
    zh: "只检测到一端。重启 CoreAudio 后重检:",
  },
  micCopy: { en: "copy", zh: "复制" },
  micCopied: { en: "copied", zh: "已复制" },
  micReboot: { en: "Reboot to finish the virtual audio install.", zh: "重启以完成虚拟声卡安装。" },
  micPermDenied: { en: "Microphone permission denied", zh: "麦克风权限被拒绝" },
  micPermHint: {
    en: "Echoless needs microphone access to capture your voice.",
    zh: "Echoless 需要麦克风权限来采集你的声音。",
  },
  micPermUndet: {
    en: "Grant microphone access on first run if prompted.",
    zh: "首次运行若有提示,请允许麦克风权限。",
  },
  micOpenDriver: { en: "open driver download", zh: "打开驱动下载" },
  micOpenPrivacy: { en: "open privacy settings", zh: "打开隐私设置" },

  // Advanced
  advNote: {
    en: "Advanced parameters",
    zh: "高级参数",
  },
  secPipeline: { en: "Pipeline", zh: "管线" },
  secSession: { en: "Session", zh: "会话" },
  sampleRate: { en: "Sample Rate", zh: "采样率" },
  frameMs: { en: "Frame", zh: "帧长" },
  referenceChannels: { en: "Reference Channels", zh: "参考声道" },
  language: { en: "Language", zh: "语言" },
  auto: { en: "auto", zh: "自动" },
  applyHint: {
    en: "changes restart the runtime",
    zh: "改动会重启运行时",
  },
  needsRestart: { en: "needs restart", zh: "需重启" },

  // Advanced · 延迟侦测 / AEC 链路诊断
  secProbe: { en: "Delay Probe", zh: "延迟侦测" },
  nearDelay: { en: "Near Delay", zh: "近端延迟" },
  probeRun: { en: "RUN PROBE", zh: "运行侦测" },
  probing: { en: "PROBING…", zh: "侦测中…" },
  probeQuiet: {
    en: "keep quiet · plays a beep train (~15s)",
    zh: "请保持安静 · 会外放一串蜂鸣(约 15 秒)",
  },
  probeStopFirst: {
    en: "stop the runtime first",
    zh: "请先停机再侦测",
  },
  probeAutoPause: {
    en: "running → engine auto-pauses, then restores",
    zh: "运行中 · 会自动暂停引擎并在完成后恢复",
  },
  probePausing: { en: "pausing engine…", zh: "暂停引擎中…" },
  probeRestoring: { en: "restoring engine…", zh: "恢复引擎中…" },
  probeRef: { en: "Ref", zh: "Ref" },
  probeMic: { en: "Mic", zh: "Mic" },
  probeEcho: { en: "echo", zh: "echo" },
  probeOk: { en: "OK", zh: "OK" },
  probeNoSig: { en: "no signal", zh: "无信号" },
  probeStable: { en: "stable", zh: "稳定" },
  probeUnstable: { en: "unstable", zh: "不稳定" },
  probeRec: { en: "set", zh: "建议" },
  probeNoFix: { en: "no fix needed · 0ms", zh: "无需修正 · 0ms" },
  probeFilled: { en: "filled into Near Delay", zh: "已填入近端延迟" },
  probeInit: { en: "init", zh: "初始延迟" },

  // Diagnostics
  diagNote: {
    en: "Record & diagnose",
    zh: "录制并诊断",
  },
  openFolder: { en: "open", zh: "打开" },
  secRecord: { en: "Record", zh: "录制" },
  secHealth: { en: "Health", zh: "健康" },
  record: { en: "Record", zh: "录制" },
  maxSeconds: { en: "Max Seconds", zh: "最长秒数" },
  unlimited: { en: "unlimited", zh: "不限" },
  recordDir: { en: "Output Dir", zh: "输出目录" },
  choose: { en: "choose…", zh: "选择…" },
  recording: { en: "recording…", zh: "录制中…" },
  recordHint: {
    en: "writes mic / ref / out .wav + stats.csv",
    zh: "写出 mic / ref / out .wav + stats.csv",
  },
  notRunning: { en: "turn ON to record", zh: "开启后开始录制" },
  comingSoon: { en: "coming next", zh: "下一步填充" },
};

interface Ctx {
  lang: Lang;
  setLang: (l: Lang) => void;
  t: (k: keyof typeof D | string) => string;
}

const LangCtx = createContext<Ctx>({
  lang: "en",
  setLang: () => {},
  t: (k) => String(k),
});

export function LangProvider({ children }: { children: ReactNode }) {
  const [lang, setLangState] = useState<Lang>(() => {
    try {
      const v = localStorage.getItem("echoless.lang");
      return v === "zh" ? "zh" : "en";
    } catch {
      return "en";
    }
  });
  const setLang = useCallback((l: Lang) => {
    setLangState(l);
    try {
      localStorage.setItem("echoless.lang", l);
    } catch {
      /* ignore */
    }
  }, []);
  const t = useCallback((k: string) => D[k]?.[lang] ?? k, [lang]);
  const value = useMemo(() => ({ lang, setLang, t }), [lang, setLang, t]);
  return <LangCtx.Provider value={value}>{children}</LangCtx.Provider>;
}

export const useI18n = () => useContext(LangCtx);
