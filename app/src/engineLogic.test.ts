import { describe, it, expect } from "vitest";
import {
  allowedNoiseModes,
  modelFileName,
  normalizeNoiseMode,
  shouldSelectNoiseMode,
  isNearDelayOnlyPatch,
  hotInitialDelayValue,
  hotLocalvqeNoiseGateValue,
  platformNearDelayDefault,
  bypassToggleTarget,
  settleBypassObservation,
  clearBypassPending,
  createRunIntentGuard,
  createSerialQueue,
  routeEngineKindSelection,
  shouldPickLocalvqeModel,
} from "./engineLogic";
import type { NoiseSuppressionManifest } from "./types";

const NOISE_MANIFEST: NoiseSuppressionManifest = {
  modes: [
    { id: "webrtc", processor_kind: "webrtc_ns" },
    { id: "rnnoise", processor_kind: "rnnoise" },
    { id: "off", processor_kind: null },
  ],
  engine_defaults: {
    aec3: ["webrtc", "rnnoise", "off"],
    nvidia_afx_aec: ["webrtc", "rnnoise", "off"],
  },
  localvqe_models: [
    {
      file: "localvqe-v1.2-1.3M-f32.gguf",
      version: "v1.2",
      capability: "built_in_ns",
      allowed_modes: ["off"],
    },
    {
      file: "localvqe-v1.3-4.8M-f32.gguf",
      version: "v1.3",
      capability: "built_in_ns",
      allowed_modes: ["off"],
    },
    {
      file: "localvqe-v1.4-aec-200K-f32.gguf",
      version: "v1.4",
      capability: "pure_aec",
      allowed_modes: ["webrtc", "rnnoise", "off"],
    },
  ],
  unknown_localvqe_allowed_modes: ["off"],
};

describe("routeEngineKindSelection", () => {
  it.each(["localvqe", "nvidia_afx_aec"])(
    "keeps AEC3 active while unready %s setup stops the old run once",
    (target) => {
      const current = { current: "aec3" };
      const aec3Params = { tail_ms: 52 };
      const targetParams = { existing: target };
      const paramsByKind: Record<string, Record<string, unknown>> = {
        aec3: aec3Params,
        [target]: targetParams,
      };
      let activeParams: Record<string, unknown> = aec3Params;
      let running = true;
      let stops = 0;
      let applies = 0;
      const setups: string[] = [];
      const handlers = {
        setup: (next: string) => {
          if (running) {
            running = false;
            stops += 1;
          }
          setups.push(next);
        },
        apply: (previous: string, next: string) => {
          paramsByKind[previous] = activeParams;
          activeParams = paramsByKind[next];
          applies += 1;
        },
      };

      expect(
        routeEngineKindSelection(current, target, false, handlers),
      ).toBe("setup");
      expect(
        routeEngineKindSelection(current, target, false, handlers),
      ).toBe("setup");

      expect(current.current).toBe("aec3");
      expect(activeParams).toBe(aec3Params);
      expect(paramsByKind[target]).toBe(targetParams);
      expect(stops).toBe(1);
      expect(applies).toBe(0);
      expect(setups).toEqual([target, target]);

      expect(routeEngineKindSelection(current, target, true, handlers)).toBe(
        "apply",
      );
      expect(routeEngineKindSelection(current, target, true, handlers)).toBe(
        "noop",
      );
      expect(current.current).toBe(target);
      expect(activeParams).toBe(targetParams);
      expect(applies).toBe(1);
    },
  );
});

describe("shouldPickLocalvqeModel", () => {
  it("rejects the selected path and accepts a different or first model", () => {
    expect(shouldPickLocalvqeModel("/models/a.gguf", "/models/a.gguf")).toBe(
      false,
    );
    expect(shouldPickLocalvqeModel("/models/a.gguf", "/models/b.gguf")).toBe(
      true,
    );
    expect(shouldPickLocalvqeModel(undefined, "/models/a.gguf")).toBe(true);
  });
});

describe("shared noise suppression compatibility", () => {
  it("uses the manifest for all supported engines and LocalVQE models", () => {
    expect(allowedNoiseModes(NOISE_MANIFEST, "aec3", {})).toEqual([
      "webrtc",
      "rnnoise",
      "off",
    ]);
    expect(
      allowedNoiseModes(NOISE_MANIFEST, "localvqe", {
        model: "C:\\models\\localvqe-v1.3-4.8M-f32.gguf",
      }),
    ).toEqual(["off"]);
    expect(
      allowedNoiseModes(NOISE_MANIFEST, "localvqe", {
        model: "/models/localvqe-v1.4-aec-200K-f32.gguf",
      }),
    ).toEqual(["webrtc", "rnnoise", "off"]);
  });

  it("forces built-in and unknown LocalVQE models to OFF", () => {
    expect(
      normalizeNoiseMode(
        NOISE_MANIFEST,
        "localvqe",
        { model: "localvqe-v1.2-1.3M-f32.gguf" },
        "rnnoise",
      ),
    ).toBe("off");
    expect(
      normalizeNoiseMode(
        NOISE_MANIFEST,
        "localvqe",
        { model: "custom.gguf" },
        "webrtc",
      ),
    ).toBe("off");
  });

  it("does not restore an old mode or reselect the current mode", () => {
    const forced = normalizeNoiseMode(
      NOISE_MANIFEST,
      "localvqe",
      { model: "localvqe-v1.3-4.8M-f32.gguf" },
      "webrtc",
    );
    expect(
      normalizeNoiseMode(NOISE_MANIFEST, "aec3", {}, forced),
    ).toBe("off");
    expect(
      shouldSelectNoiseMode("off", "off", ["webrtc", "rnnoise", "off"]),
    ).toBe(false);
    expect(shouldSelectNoiseMode("off", "rnnoise", ["off"])).toBe(false);
  });

  it("extracts model filenames without path heuristics", () => {
    expect(modelFileName("C:\\models\\model.gguf")).toBe("model.gguf");
    expect(modelFileName("/models/model.gguf")).toBe("model.gguf");
    expect(modelFileName(undefined)).toBeNull();
  });
});

describe("hotInitialDelayValue", () => {
  it("空/null → 0", () => {
    expect(hotInitialDelayValue("")).toBe(0);
    expect(hotInitialDelayValue(null)).toBe(0);
    expect(hotInitialDelayValue(undefined)).toBe(0);
  });
  it("有限数四舍五入", () => {
    expect(hotInitialDelayValue("12.4")).toBe(12);
    expect(hotInitialDelayValue(12.6)).toBe(13);
  });
  it("非有限 → null(拒发)", () => {
    expect(hotInitialDelayValue("abc")).toBeNull();
    expect(hotInitialDelayValue(NaN)).toBeNull();
    expect(hotInitialDelayValue(Infinity)).toBeNull();
  });
});

describe("hotLocalvqeNoiseGateValue", () => {
  it("阈值空 → 默认 -45dBFS", () => {
    expect(hotLocalvqeNoiseGateValue({ noise_gate: true })).toEqual({
      enabled: true,
      thresholdDbfs: -45,
    });
  });
  it("解析数值阈值 + enabled 布尔化", () => {
    expect(
      hotLocalvqeNoiseGateValue({
        noise_gate: 1,
        noise_gate_threshold_dbfs: "-60",
      }),
    ).toEqual({ enabled: true, thresholdDbfs: -60 });
  });
  it("非有限阈值 → null(拒发)", () => {
    expect(
      hotLocalvqeNoiseGateValue({ noise_gate_threshold_dbfs: "nope" }),
    ).toBeNull();
  });
});

describe("isNearDelayOnlyPatch / platformNearDelayDefault", () => {
  it("仅 near_delay_ms 走热控", () => {
    expect(isNearDelayOnlyPatch({ near_delay_ms: 30 })).toBe(true);
    expect(isNearDelayOnlyPatch({ near_delay_ms: 30, frame_ms: 10 })).toBe(
      false,
    );
    expect(isNearDelayOnlyPatch({ frame_ms: 10 })).toBe(false);
    expect(isNearDelayOnlyPatch({})).toBe(false);
  });
  it("平台默认:macOS 25,其它 0", () => {
    expect(platformNearDelayDefault("macos")).toBe(25);
    expect(platformNearDelayDefault("windows")).toBe(0);
    expect(platformNearDelayDefault("linux")).toBe(0);
  });
});

describe("bypass pending state", () => {
  it("only allows one in-flight bypass toggle", () => {
    expect(bypassToggleTarget({ bypassed: false, bypassPending: null })).toBe(
      true,
    );
    expect(bypassToggleTarget({ bypassed: false, bypassPending: true })).toBe(
      null,
    );
  });

  it("settles pending only when observation reaches the target", () => {
    expect(
      settleBypassObservation({ bypassed: false, bypassPending: true }, false),
    ).toEqual({ bypassed: false, bypassPending: true });
    expect(
      settleBypassObservation({ bypassed: false, bypassPending: true }, true),
    ).toEqual({ bypassed: true, bypassPending: null });
  });

  it("clears pending for matching send failures only", () => {
    expect(
      clearBypassPending({ bypassed: false, bypassPending: true }, false),
    ).toEqual({ bypassed: false, bypassPending: true });
    expect(
      clearBypassPending({ bypassed: false, bypassPending: true }, true),
    ).toEqual({ bypassed: false, bypassPending: null });
  });
});

describe("run intent guard", () => {
  it("invalidates an in-flight restart as soon as stop becomes the terminal intent", () => {
    const guard = createRunIntentGuard(true);
    const applyIntent = guard.snapshot();

    expect(guard.allowsStart(applyIntent)).toBe(true);
    guard.request(false);

    expect(guard.wantsRun()).toBe(false);
    expect(guard.allowsStart(applyIntent)).toBe(false);
  });

  it("requires a fresh generation before a later explicit start", () => {
    const guard = createRunIntentGuard(true);
    const staleApply = guard.snapshot();
    guard.request(false);
    const explicitStart = guard.request(true);

    expect(guard.allowsStart(staleApply)).toBe(false);
    expect(guard.allowsStart(explicitStart)).toBe(true);
  });

  it("lets a newer configuration restart invalidate an older one", () => {
    const guard = createRunIntentGuard(true);
    const firstRestart = guard.request(true);
    const latestRestart = guard.request(true);

    expect(guard.allowsStart(firstRestart)).toBe(false);
    expect(guard.allowsStart(latestRestart)).toBe(true);
    expect(guard.wantsRun()).toBe(true);
  });
});

describe("createSerialQueue (B-04 串行化)", () => {
  it("重叠 enqueue 不并发执行,严格串行", async () => {
    const events: string[] = [];
    let active = 0;
    const q = createSerialQueue<{ v: number }>(async (m) => {
      active += 1;
      expect(active).toBe(1); // 任何时刻至多一个在跑
      events.push(`start:${m.v}`);
      await new Promise((r) => setTimeout(r, 5));
      events.push(`end:${m.v}`);
      active -= 1;
    });

    q.enqueue({ v: 1 });
    await Promise.resolve(); // 让 1 先启动(模拟不同事件回合,非同一同步栈)
    q.enqueue({ v: 2 }); // 1 正在跑,2/3 合并排队
    q.enqueue({ v: 3 });
    await q.settled();
    await q.settled(); // 链在执行中又接了新任务,再等一次

    // 1 独立执行;2 与 3 合并成一次(取最后 v=3)。
    expect(events).toEqual(["start:1", "end:1", "start:3", "end:3"]);
  });

  it("同一同步栈的连发全部合并成一次(只保最终态)", async () => {
    const runs: number[] = [];
    const q = createSerialQueue<{ v: number }>(async (m) => {
      runs.push(m.v!);
    });
    q.enqueue({ v: 1 });
    q.enqueue({ v: 2 });
    q.enqueue({ v: 3 });
    await q.settled();
    expect(runs).toEqual([3]); // 中间态无需逐个重启,直接跑最终选择
  });

  it("排队期合并 delta:后到字段覆盖先到", async () => {
    const runs: Array<Record<string, unknown>> = [];
    const q = createSerialQueue<{ a?: number; b?: number }>(async (m) => {
      runs.push(m);
      await new Promise((r) => setTimeout(r, 1));
    });
    q.enqueue({ a: 1 }); // 立即开跑
    await Promise.resolve();
    q.enqueue({ a: 2 }); // 排队
    q.enqueue({ b: 9 }); // 合并进排队项
    await q.settled();
    await q.settled();
    expect(runs).toEqual([{ a: 1 }, { a: 2, b: 9 }]);
  });
});
