import { describe, expect, it } from "vitest";
import {
  beginAutostartChange,
  displayAutostartEnabled,
  rejectAutostartChange,
  settleAutostart,
  type AutostartPreference,
} from "./autostartPreference";

describe("Windows autostart preference", () => {
  it("shows the pending choice while Windows registration is changing", () => {
    const loaded: AutostartPreference = { enabled: false, pending: null };
    const changing = beginAutostartChange(loaded, true);

    expect(displayAutostartEnabled(changing)).toBe(true);
    expect(changing.pending).toBe(true);
  });

  it("uses the actual Windows result and rolls back a rejected change", () => {
    const loaded: AutostartPreference = { enabled: false, pending: null };
    const changing = beginAutostartChange(loaded, true);

    expect(settleAutostart(false)).toEqual({ enabled: false, pending: null });
    expect(rejectAutostartChange(changing)).toEqual(loaded);
  });
});
