import { describe, expect, it } from "vitest";

import appSource from "./App.tsx?raw";
import { controlErrorMessage, streamErrorMessage } from "./runEventDisplay";
import type { RunEvent, RuntimeStatus } from "./types";

const baseStatus: RuntimeStatus = {
  type: "status",
  elapsed_s: 1,
  frames: 480,
  sample_rate: 48_000,
  frame_ms: 10,
  backend: "aec3",
  mic_dbfs: -20,
  ref_dbfs: -30,
  out_dbfs: -25,
  mic_q_samples: 0,
  ref_q_samples: 0,
  out_q_samples: 0,
  input_queue_latency_ms: 0,
  output_queue_latency_ms: 0,
  algorithmic_latency_ms: 0,
  estimated_user_latency_ms: 5,
  aec_estimated_delay_ms: 0,
  mic_input_drops: 0,
  ref_input_drops: 0,
  input_drops: 0,
  ref_underruns: 0,
  output_underruns: 0,
  output_overruns: 0,
  stale_drops: 0,
  node_process_time_ms: 0,
  runtime_errors: 0,
  diverged: false,
  clock_skew_ref_correlated: true,
  clock_skew_direction: "output_faster_than_capture",
};

describe("run event contract", () => {
  it("models every discriminator currently emitted by the runtime", () => {
    const discriminators: RunEvent["type"][] = [
      "status",
      "started",
      "diagnostics_done",
      "diagnostics_started",
      "diagnostics_stopping",
      "control_error",
      "stream_error",
      "clock_skew_warning",
      "clock_skew_resolved",
      "error",
      "output_level_changed",
      "near_delay_changed",
      "initial_delay_changed",
      "aec3_agc_changed",
      "localvqe_noise_gate_changed",
      "bypass_changed",
    ];
    expect(new Set(discriminators).size).toBe(16);
  });

  it("accepts newly modeled failures, skew events, and status fields", () => {
    const statusEvent = { ...baseStatus, run_id: 1 } satisfies RunEvent;
    const events = [
      {
        type: "stream_error",
        stream: "reference",
        message: "device unavailable",
        fatal: true,
        recoverable: true,
        run_id: 1,
      },
      {
        type: "clock_skew_warning",
        output_skew_pct: 22.4,
        ref_skew_pct: 22.4,
        ref_correlated: true,
        direction: "output_faster_than_capture",
        hint: "align sample rates",
        run_id: 1,
      },
      {
        type: "clock_skew_resolved",
        output_skew_pct: 0.5,
        ref_skew_pct: 0.4,
        ref_correlated: true,
        direction: "output_faster_than_capture",
        hint: "resolved",
        run_id: 1,
      },
      {
        type: "error",
        message: "status serialization failed",
        run_id: 1,
      },
      statusEvent,
    ] satisfies RunEvent[];

    expect(events.map((event) => event.type)).toEqual([
      "stream_error",
      "clock_skew_warning",
      "clock_skew_resolved",
      "error",
      "status",
    ]);
    expect(statusEvent.clock_skew_ref_correlated).toBe(true);
  });

  it("gives null control commands an explicit label", () => {
    const event = {
      type: "control_error",
      cmd: null,
      message: "invalid JSON",
      run_id: 1,
    } satisfies RunEvent;

    expect(controlErrorMessage(event)).toBe("runtime control: invalid JSON");
  });

  it("shows fatal stream errors and immediately converges the GUI to OFF", () => {
    const event = {
      type: "stream_error",
      stream: "output",
      message: "A backend-specific error has occurred: injected",
      fatal: true,
      run_id: 1,
    } satisfies RunEvent;

    expect(streamErrorMessage(event)).toBe(
      "output stream error: A backend-specific error has occurred: injected",
    );
    expect(appSource).toMatch(
      /if \(ev\.type === "stream_error"\) \{[\s\S]*?powerOnRef\.current = false;[\s\S]*?updateApp\(\{[\s\S]*?powerOn: false,[\s\S]*?err: message,[\s\S]*?\}\);/,
    );
  });

  it("restarts recoverable stream invalidations before falling back to OFF", () => {
    expect(appSource).toMatch(
      /if \(ev\.recoverable && runIntentRef\.current\.wantsRun\(\)\) \{[\s\S]*?requestStreamRecovery\([\s\S]*?if \(next\.pendingDelayMs != null\) \{[\s\S]*?return;[\s\S]*?\}[\s\S]*?resetStreamRecovery\(\);[\s\S]*?runIntentRef\.current\.request\(false\);/,
    );
    expect(appSource).toMatch(
      /consumeStreamRecovery\(streamRecoveryRef\.current\)[\s\S]*?recovery\.delayMs != null[\s\S]*?window\.setTimeout\([\s\S]*?restartRunRef\.current\(\)/,
    );
    expect(appSource).toMatch(
      /ev\.recoverable &&[\s\S]*?streamRecoveryRef\.current\.pendingDelayMs == null[\s\S]*?requestStreamRecovery\(/,
    );
  });
});
