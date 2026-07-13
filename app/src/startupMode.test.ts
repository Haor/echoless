import { describe, expect, it } from "vitest";
import {
  createStartupCleanup,
  expectStartupRun,
  INITIAL_STARTUP_RUN_HANDSHAKE,
  observeStartupRunStarted,
  startupDataReady,
  shouldAttemptAutoStart,
  shouldRevealWindow,
} from "./startupMode";

describe("startup launch policy", () => {
  it("shows manual launches only after boot and keeps autostart hidden", () => {
    expect(shouldRevealWindow(false, "manual")).toBe(false);
    expect(shouldRevealWindow(true, "unknown")).toBe(false);
    expect(shouldRevealWindow(true, "manual")).toBe(true);
    expect(shouldRevealWindow(true, "autostart")).toBe(false);
  });

  it("attempts autostart once after cleanup and configuration are ready", () => {
    const ready = {
      mode: "autostart" as const,
      dataReady: true,
      cleanupReady: true,
      attempted: false,
      running: false,
    };

    expect(shouldAttemptAutoStart(ready)).toBe(true);
    expect(shouldAttemptAutoStart({ ...ready, mode: "manual" })).toBe(false);
    expect(shouldAttemptAutoStart({ ...ready, dataReady: false })).toBe(false);
    expect(shouldAttemptAutoStart({ ...ready, cleanupReady: false })).toBe(false);
    expect(shouldAttemptAutoStart({ ...ready, attempted: true })).toBe(false);
    expect(shouldAttemptAutoStart({ ...ready, running: true })).toBe(false);
  });

  it("waits for NVAFX doctor only when NVAFX is the selected engine", () => {
    expect(startupDataReady(true, "aec3", false)).toBe(true);
    expect(startupDataReady(true, "localvqe", false)).toBe(true);
    expect(startupDataReady(true, "nvidia_afx_aec", false)).toBe(false);
    expect(startupDataReady(true, "nvidia_afx_aec", true)).toBe(true);
    expect(startupDataReady(false, "aec3", true)).toBe(false);
  });

  it("shares one cleanup barrier across StrictMode remounts", async () => {
    let calls = 0;
    const cleanup = createStartupCleanup(async () => {
      calls += 1;
    });

    const first = cleanup();
    const second = cleanup();
    expect(second).toBe(first);
    await Promise.all([first, second]);
    expect(calls).toBe(1);
  });

  it("opens the cleanup barrier even when stale-run cleanup reports an error", async () => {
    const cleanup = createStartupCleanup(async () => {
      throw new Error("no stale run");
    });

    await expect(cleanup()).resolves.toBeUndefined();
  });

  it("settles only when started belongs to the reserved autostart run", () => {
    const staleFirst = observeStartupRunStarted(
      INITIAL_STARTUP_RUN_HANDSHAKE,
      7,
    );
    const reserved = expectStartupRun(staleFirst, 8);
    expect(reserved.settled).toBe(false);

    const actual = observeStartupRunStarted(reserved, 8);
    expect(actual.settled).toBe(true);
  });

  it("handles started arriving before the start command returns its run id", () => {
    const early = observeStartupRunStarted(INITIAL_STARTUP_RUN_HANDSHAKE, 11);
    const reserved = expectStartupRun(early, 11);

    expect(reserved.settled).toBe(true);
  });
});
