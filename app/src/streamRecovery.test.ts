import { describe, expect, it } from "vitest";

import {
  consumeStreamRecovery,
  INITIAL_STREAM_RECOVERY,
  requestStreamRecovery,
} from "./streamRecovery";

describe("stream recovery", () => {
  it("uses a bounded backoff sequence for repeated invalidations", () => {
    let state = INITIAL_STREAM_RECOVERY;
    const delays: Array<number | null> = [];

    for (let attempt = 0; attempt < 4; attempt += 1) {
      state = requestStreamRecovery(state, 1_000 + attempt * 100);
      const consumed = consumeStreamRecovery(state);
      state = consumed.state;
      delays.push(consumed.delayMs);
    }

    expect(delays).toEqual([750, 1_500, 3_000, null]);
  });

  it("starts a fresh retry window after the run stayed healthy", () => {
    const exhausted = {
      attempts: 3,
      windowStartedAtMs: 1_000,
      pendingDelayMs: null,
    };

    const recovered = requestStreamRecovery(exhausted, 31_001);
    expect(recovered.attempts).toBe(1);
    expect(recovered.pendingDelayMs).toBe(750);
  });
});
