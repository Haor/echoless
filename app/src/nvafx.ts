// NVAFX / RTX AEC runtime 安装向导的共享逻辑:把 `nvafx doctor --json` 的 checks
// 派生成「单一当前状态」与「就绪阶梯」,并提供按 GPU 架构自动选模型的资产名。
// 依据 docs/frontend/ENGINE_RUNTIME_WIZARD_GUIDANCE.md + NVAFX_RUNTIME_INSTALLER_HANDOFF.md。
import type { GpuArch, NvafxDoctor } from "./types";

const NVAFX_SDK_VERSION = "2.1.0";
export const NVAFX_COMMON_ASSET = `echoless-rtx-aec-common-runtime-win64-${NVAFX_SDK_VERSION}.zip`;
export function nvafxModelAsset(arch: GpuArch): string {
  return `echoless-rtx-aec-model-win64-${NVAFX_SDK_VERSION}-${arch}-aec48.zip`;
}

// 状态机(Codex handoff)。优先级见 deriveRtxState。
export type RtxState =
  | "unsupported_platform"
  | "missing_driver"
  | "unsupported_gpu"
  | "driver_too_old"
  | "missing_vc_redist"
  | "runtime_not_installed"
  | "model_not_installed"
  | "ready"
  | "unknown";

// 硬阻断:用户无法在本机修复(换平台/换显卡)。
export const RTX_HARD_BLOCK: RtxState[] = [
  "unsupported_platform",
  "unsupported_gpu",
];
const miss = (s: string) => s === "missing" || s === "unsupported";

export function deriveRtxState(d: NvafxDoctor | null): RtxState {
  if (!d) return "unknown";
  if (d.ok) return "ready";
  const c = d.report.checks;
  const hit = (pred: (name: string, status: string) => boolean) =>
    c.some((x) => pred(x.name, x.status));

  // 按优先级派生(平台 → 驱动 → GPU → 驱动版本 → VC++ → runtime → model)。
  if (hit((n, s) => n === "platform" && s === "unsupported"))
    return "unsupported_platform";
  if (hit((n, s) => (n === "nvidia-smi" || n === "nvcuda.dll") && s === "missing"))
    return "missing_driver";
  if (hit((n, s) => n === "gpu" && s === "missing")) return "missing_driver";
  if (hit((n, s) => n.endsWith(":arch") && s === "unsupported"))
    return "unsupported_gpu";
  if (hit((n, s) => n.endsWith(":driver") && s === "missing"))
    return "driver_too_old";
  if (hit((n, s) => n.startsWith("vc-runtime:") && s === "missing"))
    return "missing_vc_redist";
  if (
    hit(
      (n, s) =>
        (n === "runtime-dir" || (n.startsWith("runtime:") && n !== "runtime:model")) &&
        miss(s),
    )
  )
    return "runtime_not_installed";
  if (hit((n, s) => n === "runtime:model" && s === "missing"))
    return "model_not_installed";
  return "runtime_not_installed"; // not ok 但不明确 → 当作需安装
}

// 就绪阶梯(固定 7 节点)。当前状态之前的节点 = ok,当前 = active/fail,之后 = pending。
export type LadderKey =
  | "platform"
  | "driver"
  | "gpu"
  | "vc++"
  | "runtime"
  | "model"
  | "ready";
export const RTX_LADDER: LadderKey[] = [
  "platform",
  "driver",
  "gpu",
  "vc++",
  "runtime",
  "model",
  "ready",
];

const STATE_LADDER_IDX: Record<RtxState, number> = {
  unsupported_platform: 0,
  missing_driver: 1,
  driver_too_old: 1,
  unsupported_gpu: 2,
  missing_vc_redist: 3,
  runtime_not_installed: 4,
  model_not_installed: 5,
  ready: 6,
  unknown: -1,
};

export type NodeStatus = "ok" | "active" | "fail" | "pending";

// 开发态模拟:产出一份可信的 Windows RTX doctor 报告,用于在 mac 上逐屏走流程。
// 仅 dev 使用,不调后端。
export function simNvafxDoctor(state: RtxState): NvafxDoctor {
  const RT = "C:\\Users\\you\\AppData\\Local\\Echoless\\nvafx\\2.1.0";
  const gpu = {
    name: "NVIDIA GeForce RTX 5080",
    driver_version: "596.49",
    compute_capability: "120",
    arch: "blackwell" as GpuArch,
  };
  type C = NvafxDoctor["report"]["checks"][number];
  const ok = (name: string, detail: string): C => ({ name, status: "ok", detail, action: null });
  const miss = (name: string, detail: string): C => ({ name, status: "missing", detail, action: "" });
  const uns = (name: string, detail: string): C => ({ name, status: "unsupported", detail, action: "" });
  const vcOk = ok("vc-runtime:VCRUNTIME140.dll", "Microsoft VC++ runtime 已存在");
  const nvcudaOk = ok("nvcuda.dll", "CUDA driver DLL present");
  const driverOk = ok("gpu:0:driver", `${gpu.name} driver=${gpu.driver_version}`);
  const archOk = ok("gpu:0:arch", `${gpu.name} compute_cap=120 -> blackwell`);
  const rtDirOk = ok("runtime-dir", `runtime 目录: ${RT}`);
  const rtBinOk = ok("runtime:bin/NVAudioEffects.dll", `found ${RT}\\bin\\NVAudioEffects.dll`);
  const modelOk = ok("runtime:model", "found blackwell aec_48k.trtpkg");
  const rtBinMiss = miss("runtime:bin/NVAudioEffects.dll", `missing ${RT}\\bin\\NVAudioEffects.dll`);
  const rtDirMiss = miss("runtime-dir", `runtime 目录不存在: ${RT}`);
  const modelMiss = miss("runtime:model", "missing blackwell aec_48k.trtpkg");

  const mk = (
    isOk: boolean,
    gpus: typeof gpu[],
    arch: GpuArch | null,
    checks: C[],
    runtimeDir = RT,
  ): NvafxDoctor => ({
    ok: isOk,
    report: { runtime_dir: runtimeDir, runtime_dir_source: "%LOCALAPPDATA%", gpus, selected_arch: arch, checks },
  });

  switch (state) {
    case "ready":
      return mk(true, [gpu], "blackwell", [vcOk, nvcudaOk, driverOk, archOk, rtDirOk, rtBinOk, modelOk]);
    case "model_not_installed":
      return mk(false, [gpu], "blackwell", [vcOk, nvcudaOk, driverOk, archOk, rtDirOk, rtBinOk, modelMiss]);
    case "runtime_not_installed":
      return mk(false, [gpu], "blackwell", [vcOk, nvcudaOk, driverOk, archOk, rtDirMiss, rtBinMiss, modelMiss]);
    case "missing_vc_redist":
      return mk(false, [gpu], "blackwell", [
        miss("vc-runtime:VCRUNTIME140.dll", "未找到 Microsoft VC++ runtime DLL"),
        nvcudaOk, driverOk, archOk, rtDirMiss,
      ]);
    case "driver_too_old": {
      const old = { ...gpu, driver_version: "551.86" };
      return mk(false, [old], "blackwell", [
        vcOk, nvcudaOk,
        miss("gpu:0:driver", `${old.name} driver=551.86 低于最低要求 572.61`),
        archOk, rtDirMiss,
      ]);
    }
    case "unsupported_gpu": {
      const g = { name: "NVIDIA GeForce GTX 1060", driver_version: "596.49", compute_capability: "61", arch: null };
      return mk(false, [g as unknown as typeof gpu], null, [
        vcOk, nvcudaOk, ok("gpu:0:driver", `${g.name} driver=596.49`),
        uns("gpu:0:arch", `${g.name} compute_cap=61 不在支持列表`),
      ]);
    }
    case "missing_driver":
      return mk(false, [], null, [
        miss("nvidia-smi", "无法运行 nvidia-smi"),
        miss("gpu", "未检测到 NVIDIA GPU"),
        rtDirMiss,
      ]);
    case "unsupported_platform":
      return mk(false, [], null, [
        uns("platform", "NVIDIA AFX AEC runtime 目前只支持 Windows x64"),
        miss("nvidia-smi", "无法运行 nvidia-smi"),
        miss("gpu", "未检测到 NVIDIA GPU"),
      ], "/Users/you/.local/share/Echoless/nvafx/2.1.0");
    default:
      return mk(false, [gpu], "blackwell", [vcOk, nvcudaOk, driverOk, archOk, rtDirMiss, rtBinMiss, modelMiss]);
  }
}

// dev 状态切换条用的状态顺序。
export const RTX_DEV_STATES: RtxState[] = [
  "unsupported_platform",
  "missing_driver",
  "unsupported_gpu",
  "driver_too_old",
  "missing_vc_redist",
  "runtime_not_installed",
  "model_not_installed",
  "ready",
];

export function ladderStatus(state: RtxState): Record<LadderKey, NodeStatus> {
  const idx = STATE_LADDER_IDX[state];
  const hard = RTX_HARD_BLOCK.includes(state);
  const out = {} as Record<LadderKey, NodeStatus>;
  RTX_LADDER.forEach((k, i) => {
    if (state === "ready") out[k] = "ok";
    else if (idx < 0) out[k] = "pending";
    else if (i < idx) out[k] = "ok";
    else if (i === idx) out[k] = hard ? "fail" : "active";
    else out[k] = "pending";
  });
  return out;
}
