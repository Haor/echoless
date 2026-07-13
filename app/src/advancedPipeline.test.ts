import { describe, expect, it, vi } from "vitest";
import appSource from "./App.tsx?raw";
import controlsSource from "./components/Controls.tsx?raw";
import advancedPageSource from "./pages/AdvancedPage.tsx?raw";
import { canChangePipeline, pipelineForEngineKind } from "./engineLogic";

const FIXED_PIPELINE_PATCHES = [
  { sample_rate: 16_000 },
  { frame_ms: 20 },
  { reference_channels: "stereo" as const },
];

describe("NVAFX Advanced pipeline lock", () => {
  it.each(FIXED_PIPELINE_PATCHES)(
    "blocks NVAFX fixed pipeline patch %o before side effects",
    (patch) => {
      const effect = vi.fn();

      if (canChangePipeline("nvidia_afx_aec", patch)) effect();

      expect(effect).not.toHaveBeenCalled();
    },
  );

  it.each(["aec3", "localvqe"])(
    "keeps all pipeline options editable for %s",
    (kind) => {
      for (const patch of FIXED_PIPELINE_PATCHES) {
        expect(canChangePipeline(kind, patch)).toBe(true);
      }
    },
  );

  it("keeps NVAFX near-delay tuning outside the three-option lock", () => {
    expect(canChangePipeline("nvidia_afx_aec", { near_delay_ms: 25 })).toBe(
      true,
    );
  });

  it("normalizes a non-default pipeline when entering NVAFX", () => {
    expect(
      pipelineForEngineKind("nvidia_afx_aec", {
        sample_rate: 16_000,
        frame_ms: 20,
        reference_channels: "stereo",
        near_delay_ms: 31,
        output_level: 72,
      }),
    ).toEqual({
      sample_rate: 48_000,
      frame_ms: 10,
      reference_channels: "mono",
      near_delay_ms: 31,
      output_level: 72,
    });
  });

  it.each(["aec3", "localvqe"])(
    "does not normalize the pipeline when entering %s",
    (kind) => {
      const pipeline = {
        sample_rate: 16_000,
        frame_ms: 20,
        reference_channels: "stereo" as const,
      };

      expect(pipelineForEngineKind(kind, pipeline)).toBe(pipeline);
    },
  );

  it("natively disables exactly the three Advanced PIPELINE controls", () => {
    expect(advancedPageSource).toContain(
      'const pipelineDisabled = kind === "nvidia_afx_aec";',
    );
    expect(
      advancedPageSource.match(/disabled=\{pipelineDisabled\}/g),
    ).toHaveLength(3);
    expect(controlsSource).toContain('segg ${disabled ? "dim" : ""}');
    expect(controlsSource).toContain("disabled={disabled}");
  });

  it("guards the App pipeline handler before state or run side effects", () => {
    expect(appSource).toMatch(
      /function changePipeline\(patch: Partial<PipelineCfg>\) \{\s*if \(!canChangePipeline\(kindRef\.current, patch\)\) return;/,
    );
  });

  it("normalizes ready NVAFX selection in the unified engine transaction", () => {
    expect(appSource).toMatch(
      /const nextPipeline = pipelineForEngineKind\(\s*target,\s*pipelineRef\.current,\s*\);/,
    );
    expect(appSource).toMatch(
      /updateEngine\(\{\s*kind: target,\s*noiseMode: nextNoiseMode,\s*params: np,\s*pipeline: nextPipeline,\s*\}\);/,
    );
    expect(appSource).toMatch(
      /applyChangeRef\.current\(\{\s*kind: target,\s*noiseMode: nextNoiseMode,\s*params: np,\s*pipeline: nextPipeline,\s*\}\);/,
    );
    expect(appSource).toContain("onClick={() => changeKind(m.kind)}");
    expect(appSource.match(/pipelineRef\.current = nextPipeline;/g)).toHaveLength(
      1,
    );
  });

  it("normalizes a persisted NVAFX pipeline before locked controls render", () => {
    expect(appSource).toContain(
      "pipeline: pipelineForEngineKind(kind, savedPipeline)",
    );
  });
});

describe("shared NS Advanced parameters", () => {
  it("shows manifest-backed WebRTC strength inside Pipeline only", () => {
    expect(advancedPageSource).toContain(
      "const noiseProcessorKind = noiseSuppression?.modes.find(",
    );
    const pipeline = advancedPageSource.indexOf('t("secPipeline")');
    const strength = advancedPageSource.indexOf(
      'noiseMode === "webrtc" &&',
      pipeline,
    );
    const backend = advancedPageSource.indexOf(
      "backendLabel(kind, proc)",
      pipeline,
    );

    expect(strength).toBeGreaterThan(pipeline);
    expect(strength).toBeLessThan(backend);
    expect(advancedPageSource).toContain(
      "arow(key, `NS ${key}`, spec, noiseParams, onNoiseParam)",
    );
    expect(advancedPageSource).not.toContain("anoise-section");
  });

  it("keeps mode parameters separate and ignores the selected value", () => {
    expect(appSource).toContain("noiseParams={noiseParamsByMode[noiseMode] ?? {}}");
    expect(appSource).toContain("const next = patchNoiseModeParam(");
    expect(appSource).toContain("if (!next) return;");
  });

  it("starts delay-probe lights only from the real beep event", () => {
    expect(advancedPageSource).not.toContain("PROBE_FIRST_MS");
    expect(advancedPageSource).toMatch(
      /if \(p\.stage !== "beep_train_start"\) return;[\s\S]*timer\.current = window\.setInterval/,
    );
  });

  it("keeps the probe action visually bracketed without polluting its label", () => {
    expect(advancedPageSource).toContain(
      '{probing ? t("probing") : t("probeRun")}',
    );
    expect(advancedPageSource).not.toContain('probing ? "•••" : "↻"');
  });

  it("keeps Session in the left flow instead of below Delay Probe", () => {
    const lowerStart = advancedPageSource.indexOf(
      '<div className="alower-left">',
    );
    const session = advancedPageSource.indexOf('t("secSession")', lowerStart);
    const probe = advancedPageSource.indexOf("<ProbeSection", lowerStart);

    expect(lowerStart).toBeGreaterThan(-1);
    expect(session).toBeGreaterThan(lowerStart);
    expect(probe).toBeGreaterThan(session);
  });
});

describe("delay probe page lifecycle", () => {
  it("keeps Advanced mounted and clears settled results only while hidden", () => {
    expect(appSource).toContain(
      '<div className="persistent-view" hidden={view !== "advanced"}>',
    );
    expect(appSource).toContain('visible={view === "advanced"}');
    expect(appSource).not.toContain("probeActive");
    expect(advancedPageSource).toContain(
      "if (!visible && !probing && (probe != null || probeErr != null || lit > 0))",
    );
    expect(advancedPageSource).toContain("updateProbe(PROBE_INITIAL_STATE)");
  });
});
