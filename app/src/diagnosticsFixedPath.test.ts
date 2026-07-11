import { describe, expect, it } from "vitest";
import apiSource from "./api.ts?raw";
import appSource from "./App.tsx?raw";
import diagnosticsSource from "./pages/DiagnosticsPage.tsx?raw";
import controlsSource from "./runtimeControls.ts?raw";

describe("fixed diagnostics directory contract", () => {
  it("does not expose a directory picker or runtime path input", () => {
    expect(apiSource).not.toContain("record_dir");
    expect(controlsSource).not.toContain("record_dir");
    expect(appSource).not.toContain("diagDirRef");
    expect(appSource).not.toContain("setRecDir");
    expect(diagnosticsSource).not.toContain("@tauri-apps/plugin-dialog");
    expect(diagnosticsSource).not.toContain("onDir");
    expect(diagnosticsSource).toContain("openDiagnosticsDir");
  });
});
