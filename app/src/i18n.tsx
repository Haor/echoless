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
  // P8-D1:OFF = 穿透 —— 强调 mic 仍然活着,只是 AEC 旁路。
  bypassLive: { en: "Bypass · Mic Live", zh: "直通 · 麦克风在线" },
  unstable: { en: "Unstable", zh: "不稳定" },
  noReference: { en: "No Reference", zh: "无参考信号" },
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
  installCable: { en: "install virtual cable", zh: "安装虚拟声卡" },
  inAppPickMic: { en: "in app pick {name} as mic", zh: "通话软件麦克风选 {name}" },
  reduceNoise: { en: "Reduce background noise", zh: "抑制背景噪声" },
  aec3Only: { en: "AEC3 only", zh: "仅 AEC3" },
  lvqeNsHint: {
    en: "on = v1.3 · off = v1.4 pure aec",
    zh: "开 = v1.3 · 关 = v1.4 纯回声消除",
  },

  signal: { en: "Signal", zh: "Signal" },
  sigFlow: {
    en: "Near-end Mic + Ref » Clean Output",
    zh: "Near-end Mic + Ref » Clean Output",
  },

  backToOverview: { en: "Overview", zh: "返回总览" },

  // 系统音频录制权限(mac Process Tap reference)
  sysAudioGrant: { en: "grant system audio permission", zh: "授予系统音频权限" },
  sysAudioRequest: {
    en: "request system audio permission",
    zh: "请求系统音频权限",
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
    en: "pair with NVIDIA Broadcast",
    zh: "pair with NVIDIA Broadcast",
  },
  engWinOnly: {
    en: "unavailable on this OS",
    zh: "当前系统不可用",
  },
  windowsRtxOnly: { en: "WINDOWS · RTX ONLY", zh: "仅 WINDOWS · RTX" },
  engNoGpu: { en: "no NVIDIA GPU detected", zh: "未检测到 NVIDIA GPU" },
  // NVAFX 不可用态右栏:本机系统信息(填补空白)
  nvnaModel: { en: "device", zh: "机型" },
  nvnaOs: { en: "OS", zh: "系统" },
  nvnaChip: { en: "chip", zh: "芯片" },
  nvnaMemory: { en: "memory", zh: "内存" },
  nvnaCoresSuffix: { en: "-core", zh: " 核" },
  engRecheck: { en: "recheck", zh: "重检" },
  // LocalVQE 模型列表
  // 徽标重设计(2026-07-05):DEFAULT 全词太宽挤掉参数量 → 工程 BOM 的标准件记号 STD
  lvqeDefault: { en: "STD", zh: "标配" },
  lvqeDefaultHint: { en: "default model", zh: "默认模型" },
  lvqeDownload: { en: "download", zh: "下载" },
  lvqeGet: { en: "GET", zh: "下载" },
  lvqeOpenDir: { en: "open model folder", zh: "打开模型目录" },
  lvqeRuntimeMissing: {
    en: "native runtime missing",
    zh: "缺少原生运行库",
  },
  engSetupHint: { en: "set up in Engine", zh: "去 Engine 配置" },
  engSetupRtx: { en: "set up RTX", zh: "配置 RTX" },

  // RTX Setup 向导
  rtxSetup: { en: "RTX SETUP", zh: "RTX 配置" },
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
  wzAssetDownload: {
    en: "open download link in browser",
    zh: "在浏览器中打开下载链接",
  },
  wzDownloadInstall: { en: "download & install", zh: "下载并安装" },
  wzDownloading: { en: "downloading… ~1 GB, may take a while", zh: "下载中… 约 1 GB,可能需要一会" },
  wzDl: { en: "downloading", zh: "下载中" },
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
  micLinuxMissing: { en: "PipeWire null sink not found", zh: "未检测到 PipeWire null sink" },
  micLinuxInstallHint: {
    en: "Create the Echoless null sink in a terminal:",
    zh: "在终端创建 Echoless null sink:",
  },
  micLinuxMonitorHint: {
    en: 'In GNOME/KDE sound settings and your call app, choose "Monitor of Echoless-Output" as the microphone.',
    zh: '在 GNOME/KDE 声音设置与通话软件里,把麦克风选成 "Monitor of Echoless-Output"。',
  },
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
  micRebootTitle: {
    en: "Driver installed · devices not active yet",
    zh: "驱动已安装 · 设备尚未生效",
  },
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
  // Advanced · 延迟侦测 / AEC 链路诊断
  secProbe: { en: "Delay Probe", zh: "延迟侦测" },
  nearDelay: { en: "Near Delay", zh: "近端延迟" },
  probeRun: { en: "RUN PROBE", zh: "运行侦测" },
  probeRunHint: {
    en: "Plays test beeps to measure the echo delay and auto-fill the delay params. The engine pauses during the test and restores after (~15s).",
    zh: "外放测试蜂鸣测量回声延迟并自动填入延迟参数。测试期间引擎暂停,结束后恢复(约 15 秒)。",
  },
  probing: { en: "PROBING…", zh: "侦测中…" },
  probeQuiet: {
    en: "keep quiet · plays a beep train (~15s)",
    zh: "请保持安静 · 会外放一串蜂鸣(约 15 秒)",
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
  probeNoFix: {
    // v8/C6:去掉「no fix needed」歧义 —— 正 lag 由 AEC3 自行追踪,near_delay 不动。
    en: "aligned · near_delay kept at 0ms",
    zh: "已对齐 · 近端延迟保持 0ms",
  },
  probeFilled: { en: "filled into Near Delay", zh: "已填入近端延迟" },
  probeInit: { en: "init", zh: "初始延迟" },

  // Session · Windows 托盘偏好(P5;只留「关闭到托盘」,最小化开关退役 2026-07-05)
  trayClose: { en: "Close to Tray", zh: "关闭到托盘" },
  trayCloseHint: {
    en: "Closing hides to tray instead of quitting. Quit via tray menu.",
    zh: "点关闭改为收进托盘而非退出;从托盘菜单 Quit 才真正退出。",
  },

  // Diagnostics
  diagNote: {
    en: "Record & diagnose",
    zh: "录制并诊断",
  },
  openFolder: { en: "open", zh: "打开" },
  secRecord: { en: "Record", zh: "录制" },
  secHealth: { en: "Health", zh: "健康" },
  // Health · clock skew 悬停提示(不点名任何具体虚拟音频软件)
  clockSkewHint: {
    en: "Output clock drift vs. the microphone clock. High or unstable drift usually means a device in the audio chain runs at a mismatched sample rate — set every input/output device (including virtual audio devices) to the same rate, e.g. 48000 Hz.",
    zh: "输出时钟相对麦克风时钟的漂移。漂移偏高或不稳定,通常说明音频链路中有设备采样率不一致——请把所有输入/输出设备(含虚拟音频设备)统一为同一采样率,如 48000 Hz。",
  },
  // Health · 其余计数器悬停提示
  hInputDrops: {
    en: "Microphone/reference frames dropped before processing — the capture side couldn't keep up. Occasional drops are harmless; a steadily growing count means CPU pressure or a misbehaving input device.",
    zh: "进入处理前被丢弃的麦克风/参考帧——采集侧没跟上。偶发丢帧无碍;持续增长说明 CPU 吃紧或输入设备异常。",
  },
  hRefUnderruns: {
    en: "The echo canceller needed reference audio (system playback) but none had arrived yet. Frequent underruns weaken echo cancellation — often a sign of reference capture stalling.",
    zh: "回声消除需要参考音频(系统播放声)时参考还没到。频繁欠载会削弱回声消除效果——常见原因是参考采集卡顿。",
  },
  hOutputUnderruns: {
    en: "The output device asked for audio but the processed stream wasn't ready — heard as clicks or dropouts. Usually clock drift or CPU pressure.",
    zh: "输出设备要数据时处理后的音频还没就绪——听感为咔哒/断续。通常是时钟漂移或 CPU 吃紧。",
  },
  hMicStale: {
    en: "Microphone frames discarded for arriving too late to be useful. Growing count points at input-side stalls (device sleep, USB hiccups, scheduling).",
    zh: "因到得太晚而被丢弃的麦克风帧。持续增长指向输入侧卡顿(设备休眠、USB 抖动、系统调度)。",
  },
  hRefStale: {
    en: "Reference frames discarded for arriving too late to align with the microphone. Growing count degrades echo cancellation — check reference capture stability.",
    zh: "因到得太晚、无法与麦克风对齐而被丢弃的参考帧。持续增长会劣化回声消除——检查参考采集是否稳定。",
  },
  hRuntimeErrors: {
    en: "Errors thrown by the processing engine at runtime. Any non-zero value is worth a look — check the session log for details.",
    zh: "处理引擎运行期抛出的错误数。非零就值得关注——详情看会话日志。",
  },
  hDiverged: {
    en: "The echo canceller's internal filter has diverged (output no longer trustworthy). The engine flags this state; if it persists, restart the run and check clock skew.",
    zh: "回声消除内部滤波器已发散(输出不再可信)。若持续出现,重启运行并检查时钟漂移。",
  },
  // Diagnostics · macOS 权限检查(麦克风 + 系统录音)
  secPermissions: { en: "System Permissions", zh: "系统权限" },
  permMic: { en: "Microphone", zh: "麦克风" },
  permSysAudio: { en: "System Audio", zh: "系统录音" },
  permGranted: { en: "granted", zh: "已授予" },
  permDenied: { en: "denied", zh: "已拒绝" },
  permUndet: { en: "not requested", zh: "未申请" },
  permUnknown: { en: "unknown", zh: "未知" },
  permRequest: { en: "request", zh: "申请" },
  permOpenSettings: { en: "open settings", zh: "打开设置" },
  record: { en: "Record", zh: "录制" },
  maxSeconds: { en: "Max Seconds", zh: "最长秒数" },
  // 74px 输入框放不下 UNLIMITED(9 字符),用 NO MAX
  unlimited: { en: "no max", zh: "不限" },
  volMuteHint: {
    en: "click: mute / restore · wheel: adjust",
    zh: "点按:静音/恢复 · 滚轮:调节",
  },
  recordDir: { en: "Output Dir", zh: "输出目录" },
  choose: { en: "choose…", zh: "选择…" },
  recording: { en: "recording…", zh: "录制中…" },
  notRunning: { en: "turn ON to record", zh: "开启后开始录制" },
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
