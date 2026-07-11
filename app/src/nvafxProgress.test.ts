import { describe, expect, it } from "vitest";

import apiSource from "./api.ts?raw";
import appSource from "./App.tsx?raw";
import setupSource from "./pages/RtxSetupPage.tsx?raw";

describe("NVAFX download progress", () => {
  it("renders known percentages and keeps a received-byte fallback", () => {
    expect(apiSource).toContain("pct: number | null;");
    expect(setupSource).toContain("pct != null");
    expect(setupSource).toContain("recv != null && recv > 0");
    expect(setupSource).toContain("MiB");
    expect(appSource).toContain("nvafxPct: p");
    expect(appSource).toContain("nvafxPct: Math.min(pct, 99)");
  });

  it("does not restore an extra HEAD or Content-Length request", () => {
    expect(appSource).not.toMatch(/\bHEAD\b|Content-Length/);
    expect(apiSource).not.toMatch(/\bHEAD\b|Content-Length/);
    expect(appSource).toContain("nvafxPct: null");
  });
});
