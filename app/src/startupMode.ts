export type StartupMode = "unknown" | "manual" | "autostart";

export type StartupRunHandshake = {
  expectedRunId: number | null;
  earlyStartedRunIds: number[];
  settled: boolean;
};

export const INITIAL_STARTUP_RUN_HANDSHAKE: StartupRunHandshake = {
  expectedRunId: null,
  earlyStartedRunIds: [],
  settled: false,
};

function validRunId(runId: number): boolean {
  return Number.isSafeInteger(runId) && runId > 0;
}

export function expectStartupRun(
  state: StartupRunHandshake,
  runId: number,
): StartupRunHandshake {
  if (state.settled || !validRunId(runId)) return state;
  return {
    expectedRunId: runId,
    earlyStartedRunIds: state.earlyStartedRunIds,
    settled: state.earlyStartedRunIds.includes(runId),
  };
}

export function observeStartupRunStarted(
  state: StartupRunHandshake,
  runId: number,
): StartupRunHandshake {
  if (state.settled || !validRunId(runId)) return state;
  if (state.expectedRunId != null) {
    return runId === state.expectedRunId ? { ...state, settled: true } : state;
  }
  if (state.earlyStartedRunIds.includes(runId)) return state;
  return {
    ...state,
    earlyStartedRunIds: [...state.earlyStartedRunIds, runId],
  };
}

export function createStartupCleanup(
  cleanup: () => Promise<unknown>,
): () => Promise<void> {
  let barrier: Promise<void> | null = null;
  return () => {
    if (barrier == null) {
      barrier = cleanup().then(
        () => undefined,
        () => undefined,
      );
    }
    return barrier;
  };
}

export function shouldRevealWindow(
  booted: boolean,
  mode: StartupMode,
): boolean {
  return booted && mode === "manual";
}

export function shouldAttemptAutoStart(input: {
  mode: StartupMode;
  dataReady: boolean;
  cleanupReady: boolean;
  attempted: boolean;
  running: boolean;
}): boolean {
  return (
    input.mode === "autostart" &&
    input.dataReady &&
    input.cleanupReady &&
    !input.attempted &&
    !input.running
  );
}

export function startupDataReady(
  coreReady: boolean,
  engineKind: string,
  nvafxChecked: boolean,
): boolean {
  return (
    coreReady && (engineKind !== "nvidia_afx_aec" || nvafxChecked)
  );
}
