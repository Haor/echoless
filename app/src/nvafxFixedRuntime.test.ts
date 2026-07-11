import { describe, expect, it } from "vitest";
import apiSource from "./api.ts?raw";
import appSource from "./App.tsx?raw";
import engineSource from "./pages/EnginePage.tsx?raw";

describe("NVAFX fixed runtime contract", () => {
  it("does not expose a runtime override through frontend API or engine UI", () => {
    expect(apiSource).not.toContain("runtimeDir");
    expect(appSource).not.toContain("paramsRef.current.runtime_dir");
    expect(engineSource).not.toContain('onParam("runtime_dir"');
    expect(engineSource).not.toContain("pickRuntime");
  });
});
