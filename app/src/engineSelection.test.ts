import { describe, expect, it, vi } from "vitest";
import appSource from "./App.tsx?raw";
import enginePageSource from "./pages/EnginePage.tsx?raw";
import { claimEngineKindChange } from "./engineLogic";

const ENGINE_KINDS = ["aec3", "localvqe", "nvidia_afx_aec"] as const;

describe("claimEngineKindChange", () => {
  it.each(ENGINE_KINDS)("rejects the current %s engine without effects", (kind) => {
    const current = { current: kind };
    const effect = vi.fn();

    if (claimEngineKindChange(current, kind)) effect();

    expect(current.current).toBe(kind);
    expect(effect).not.toHaveBeenCalled();
  });

  it.each([
    ["nvidia_afx_aec", "aec3"],
    ["aec3", "localvqe"],
    ["localvqe", "nvidia_afx_aec"],
  ])("allows %s → %s once and immediately claims the target", (from, to) => {
    const current = { current: from };
    const effect = vi.fn();
    const select = () => {
      if (claimEngineKindChange(current, to)) effect();
    };

    select();
    select();

    expect(current.current).toBe(to);
    expect(effect).toHaveBeenCalledTimes(1);
  });
});

describe("current engine UI lock wiring", () => {
  it("routes unready selections through the explicit setup stop without claiming kind", () => {
    expect(appSource).toContain(
      "routeEngineKindSelection(kindRef, k, engineReady(k)",
    );
    expect(appSource).toContain("stopForEngineSetupRef.current();");
    expect(appSource).toContain(
      "paramsByKind.current[previous] = paramsRef.current;",
    );
    expect(appSource).toContain("paramsByKind.current[target] = np;");
    expect(appSource).toMatch(
      /stopForEngineSetupRef\.current = \(\) => \{\s*if \(!powerOnRef\.current \|\| engineSetupStopPendingRef\.current\) return;\s*engineSetupStopPendingRef\.current = true;\s*powerOnRef\.current = false;\s*void stop\(\);/,
    );
    expect(appSource).toContain("onClick={() => changeKind(m.kind)}");
    expect(appSource).not.toContain("if (powerOnRef.current) stop();");
  });

  it("keeps active engine segments locked", () => {
    expect(appSource).toContain("const active = kind === m.kind;");
    expect(appSource).toContain("disabled={!supported || active}");
  });

  it("locks the active AEC3 and LocalVQE cards without removing active styling", () => {
    expect(enginePageSource).toContain("disabled={!sup || active}");
    expect(enginePageSource).toContain("aria-disabled={!sup || active}");
    expect(enginePageSource).toContain("tabIndex={sup && !active ? 0 : -1}");
    expect(enginePageSource).toContain("sup && !active && onSelect(p.kind)");
    expect(enginePageSource).toContain("${active ? \"active\" : \"\"}");
  });

  it("locks both NVAFX selection surfaces while leaving the active card visible", () => {
    expect(enginePageSource).toContain(
      "aria-disabled={!nvSupported || active}",
    );
    expect(enginePageSource).toContain(
      "tabIndex={nvSupported && !active ? 0 : -1}",
    );
    expect(enginePageSource).toContain("disabled={!nvSupported || active}");
    expect(enginePageSource).toContain("nvSupported && !active && onSelect");
    expect(enginePageSource).toContain(
      'className={`ecard wide ${active ? "active" : ""}',
    );
  });
});
