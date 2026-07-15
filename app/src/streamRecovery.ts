const RECOVERY_DELAYS_MS = [750, 1_500, 3_000] as const;
const RECOVERY_WINDOW_MS = 30_000;

export type StreamRecoveryState = {
  attempts: number;
  windowStartedAtMs: number | null;
  pendingDelayMs: number | null;
};

export const INITIAL_STREAM_RECOVERY: StreamRecoveryState = {
  attempts: 0,
  windowStartedAtMs: null,
  pendingDelayMs: null,
};

export function requestStreamRecovery(
  state: StreamRecoveryState,
  nowMs: number,
): StreamRecoveryState {
  const inCurrentWindow =
    state.windowStartedAtMs != null &&
    nowMs - state.windowStartedAtMs <= RECOVERY_WINDOW_MS;
  const attempts = inCurrentWindow ? state.attempts : 0;
  const windowStartedAtMs = inCurrentWindow ? state.windowStartedAtMs : nowMs;
  const pendingDelayMs = RECOVERY_DELAYS_MS[attempts] ?? null;

  return {
    attempts: pendingDelayMs == null ? attempts : attempts + 1,
    windowStartedAtMs,
    pendingDelayMs,
  };
}

export function consumeStreamRecovery(
  state: StreamRecoveryState,
): { state: StreamRecoveryState; delayMs: number | null } {
  return {
    state: { ...state, pendingDelayMs: null },
    delayMs: state.pendingDelayMs,
  };
}
