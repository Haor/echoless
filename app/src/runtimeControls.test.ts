import { describe, expect, it } from "vitest";
import {
  REQUIRED_RUN_CONTROLS,
  RUN_CONTROL_COMMANDS,
  setAec3AgcControlLine,
  setAec3NsControlLine,
  setBypassControlLine,
  setInitialDelayMsControlLine,
  setLocalvqeNoiseGateControlLine,
  setNearDelayMsControlLine,
  setOutputLevelControlLine,
  startDiagnosticsControlLine,
  stopDiagnosticsControlLine,
} from "./runtimeControls";

const parse = (line: string): unknown => JSON.parse(line);

describe("runtime control JSON contract", () => {
  it("serializes every stdin command shape used by the app", () => {
    expect(parse(startDiagnosticsControlLine(3))).toEqual({
      cmd: "start_diagnostics",
      max_seconds: 3,
    });
    expect(parse(startDiagnosticsControlLine(null))).toEqual({
      cmd: "start_diagnostics",
      max_seconds: null,
    });
    expect(parse(stopDiagnosticsControlLine())).toEqual({
      cmd: "stop_diagnostics",
    });
    expect(parse(setOutputLevelControlLine(75))).toEqual({
      cmd: "set_output_level",
      level: 75,
    });
    expect(parse(setBypassControlLine(true))).toEqual({
      cmd: "set_bypass",
      enabled: true,
    });
    expect(parse(setNearDelayMsControlLine(25))).toEqual({
      cmd: "set_near_delay_ms",
      near_delay_ms: 25,
    });
    expect(parse(setInitialDelayMsControlLine(8))).toEqual({
      cmd: "set_initial_delay_ms",
      initial_delay_ms: 8,
    });
    expect(parse(setAec3NsControlLine(true, "high"))).toEqual({
      cmd: "set_aec3_ns",
      ns: true,
      ns_level: "high",
    });
    expect(parse(setAec3AgcControlLine(false))).toEqual({
      cmd: "set_aec3_agc",
      agc: false,
    });
    expect(parse(setLocalvqeNoiseGateControlLine(true, -45))).toEqual({
      cmd: "set_localvqe_noise_gate",
      noise_gate: true,
      noise_gate_threshold_dbfs: -45,
    });
  });

  it("keeps startup-required controls inside the known command set", () => {
    const known = new Set(Object.values(RUN_CONTROL_COMMANDS));
    for (const command of REQUIRED_RUN_CONTROLS) {
      expect(known.has(command)).toBe(true);
    }
  });
});
