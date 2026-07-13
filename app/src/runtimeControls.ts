export const RUN_CONTROL_COMMANDS = {
  startDiagnostics: "start_diagnostics",
  stopDiagnostics: "stop_diagnostics",
  setOutputLevel: "set_output_level",
  setBypass: "set_bypass",
  setNearDelayMs: "set_near_delay_ms",
  setInitialDelayMs: "set_initial_delay_ms",
  setAec3Agc: "set_aec3_agc",
  setLocalvqeNoiseGate: "set_localvqe_noise_gate",
} as const;

export const REQUIRED_RUN_CONTROLS = [
  RUN_CONTROL_COMMANDS.startDiagnostics,
  RUN_CONTROL_COMMANDS.stopDiagnostics,
  RUN_CONTROL_COMMANDS.setOutputLevel,
  RUN_CONTROL_COMMANDS.setNearDelayMs,
  RUN_CONTROL_COMMANDS.setInitialDelayMs,
  RUN_CONTROL_COMMANDS.setAec3Agc,
] as const;

type RunControlPayload =
  | {
      cmd: typeof RUN_CONTROL_COMMANDS.startDiagnostics;
      max_seconds: number | null;
    }
  | { cmd: typeof RUN_CONTROL_COMMANDS.stopDiagnostics }
  | { cmd: typeof RUN_CONTROL_COMMANDS.setOutputLevel; level: number }
  | { cmd: typeof RUN_CONTROL_COMMANDS.setBypass; enabled: boolean }
  | { cmd: typeof RUN_CONTROL_COMMANDS.setNearDelayMs; near_delay_ms: number }
  | {
      cmd: typeof RUN_CONTROL_COMMANDS.setInitialDelayMs;
      initial_delay_ms: number;
    }
  | { cmd: typeof RUN_CONTROL_COMMANDS.setAec3Agc; agc: boolean }
  | {
      cmd: typeof RUN_CONTROL_COMMANDS.setLocalvqeNoiseGate;
      noise_gate: boolean;
      noise_gate_threshold_dbfs: number;
    };

function runtimeControlLine(payload: RunControlPayload): string {
  return JSON.stringify(payload);
}

export function startDiagnosticsControlLine(
  maxSeconds: number | null,
): string {
  return runtimeControlLine({
    cmd: RUN_CONTROL_COMMANDS.startDiagnostics,
    max_seconds: maxSeconds,
  });
}

export function stopDiagnosticsControlLine(): string {
  return runtimeControlLine({ cmd: RUN_CONTROL_COMMANDS.stopDiagnostics });
}

export function setOutputLevelControlLine(level: number): string {
  return runtimeControlLine({ cmd: RUN_CONTROL_COMMANDS.setOutputLevel, level });
}

export function setBypassControlLine(enabled: boolean): string {
  return runtimeControlLine({ cmd: RUN_CONTROL_COMMANDS.setBypass, enabled });
}

export function setNearDelayMsControlLine(nearDelayMs: number): string {
  return runtimeControlLine({
    cmd: RUN_CONTROL_COMMANDS.setNearDelayMs,
    near_delay_ms: nearDelayMs,
  });
}

export function setInitialDelayMsControlLine(initialDelayMs: number): string {
  return runtimeControlLine({
    cmd: RUN_CONTROL_COMMANDS.setInitialDelayMs,
    initial_delay_ms: initialDelayMs,
  });
}

export function setAec3AgcControlLine(agc: boolean): string {
  return runtimeControlLine({ cmd: RUN_CONTROL_COMMANDS.setAec3Agc, agc });
}

export function setLocalvqeNoiseGateControlLine(
  noiseGate: boolean,
  noiseGateThresholdDbfs: number,
): string {
  return runtimeControlLine({
    cmd: RUN_CONTROL_COMMANDS.setLocalvqeNoiseGate,
    noise_gate: noiseGate,
    noise_gate_threshold_dbfs: noiseGateThresholdDbfs,
  });
}
