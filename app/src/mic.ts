// 虚拟麦克风路由向导的开发态模拟:把「单一当前状态」映射成一份可信的
// `doctor audio --json` 报告,用于在 mac 上逐屏走完 missing → ready 流程。
// 仅 dev 使用,不调后端。状态优先级与 MicSetupPage 的派生一致。
import type { DoctorAudio, DoctorCandidate, Platform } from "./types";

export type MicState =
  | "unknown"
  | "missing"
  | "incomplete"
  | "permission"
  | "ready";

// dev 状态切换条用的顺序(unknown 不暴露,它是「未探测」的真实初值)。
export const MIC_DEV_STATES: MicState[] = [
  "missing",
  "incomplete",
  "permission",
  "ready",
];

// macOS:BlackHole 同名设备同时作为 output / input 出现(环回)。
// Windows:VB-CABLE 的输入/输出端名字不同(CABLE Input ↔ CABLE Output)。
function cables(platform: Platform): { out: DoctorCandidate; mic: DoctorCandidate } {
  if (platform === "windows") {
    return {
      out: {
        index: 3,
        kind: "output",
        name: "CABLE Input (VB-Audio Virtual Cable)",
        selector: "3",
        stable_id: "cable-in",
      },
      mic: {
        index: 4,
        kind: "input",
        name: "CABLE Output (VB-Audio Virtual Cable)",
        selector: "4",
        stable_id: "cable-out",
      },
    };
  }
  return {
    out: { index: 3, kind: "output", name: "BlackHole 2ch", selector: "3", stable_id: "bh-out" },
    mic: { index: 4, kind: "input", name: "BlackHole 2ch", selector: "4", stable_id: "bh-in" },
  };
}

export function simMicDoctor(
  state: MicState,
  platform: Platform = "macos",
): DoctorAudio {
  const isWin = platform === "windows";
  const { out, mic } = cables(platform);
  const base: DoctorAudio = {
    ok: false,
    platform,
    virtual_output_detected: false,
    candidate_inputs: [],
    candidate_outputs: [],
    recommended_driver: isWin ? "vb-cable" : "blackhole-2ch",
    install_status: "missing",
    needs_reboot: false,
    // Windows 当前后端不暴露麦权限态;mac 才有 granted/denied/undetermined。
    permission_state: isWin ? "unknown" : "granted",
    reference_sources: [],
    virtual_route_ready: false,
    route_status: "missing",
    recommended_output: null,
    recommended_app_mic: null,
  };

  switch (state) {
    case "ready":
      return {
        ...base,
        ok: true,
        virtual_output_detected: true,
        candidate_outputs: [out],
        candidate_inputs: [mic],
        install_status: "installed",
        permission_state: isWin ? "unknown" : "granted",
        virtual_route_ready: true,
        route_status: "ready",
        recommended_output: out,
        recommended_app_mic: mic,
      };
    case "permission":
      // Windows 无此态 → 退化为 ready;mac 显示「权限被拒」。
      return {
        ...base,
        virtual_output_detected: true,
        candidate_outputs: [out],
        candidate_inputs: [mic],
        install_status: "installed",
        ok: isWin,
        permission_state: isWin ? "unknown" : "denied",
        virtual_route_ready: true,
        route_status: "ready",
        recommended_output: out,
        recommended_app_mic: mic,
      };
    case "incomplete":
      // 只检测到输出端(input 端缺失)→ 路由不完整。Windows 多半需重启,mac 重启 CoreAudio。
      return {
        ...base,
        virtual_output_detected: true,
        candidate_outputs: [out],
        candidate_inputs: [],
        install_status: "installed",
        needs_reboot: isWin,
        virtual_route_ready: false,
        route_status: "incomplete",
        recommended_output: out,
        recommended_app_mic: null,
      };
    case "missing":
    case "unknown":
    default:
      return base;
  }
}
