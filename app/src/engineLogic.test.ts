import { describe, it, expect } from "vitest";
import {
  LVQE_NS_ON_FILE,
  LVQE_NS_OFF_FILE,
  lvqePureAec,
  lvqeNoiseTargetFile,
  isNearDelayOnlyPatch,
  hotInitialDelayValue,
  hotLocalvqeNoiseGateValue,
  platformNearDelayDefault,
  bypassToggleTarget,
  settleBypassObservation,
  clearBypassPending,
  createSerialQueue,
  routeEngineKindSelection,
} from "./engineLogic";

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

describe("LocalVQE NOISE ↔ model mapping", () => {
  it("NOISE on 选 v1.3(AEC+降噪),off 选 v1.4(纯 AEC)", () => {
    expect(lvqeNoiseTargetFile(true)).toBe(LVQE_NS_ON_FILE);
    expect(lvqeNoiseTargetFile(false)).toBe(LVQE_NS_OFF_FILE);
  });

  it("往返一致:on→file→pureAec 判定还原 NOISE 态", () => {
    // NOISE on = 非纯 AEC(v1.3);off = 纯 AEC(v1.4)。
    expect(lvqePureAec(lvqeNoiseTargetFile(true))).toBe(false);
    expect(lvqePureAec(lvqeNoiseTargetFile(false))).toBe(true);
  });

  it("pureAec 靠文件名 -aec- 标记,兼容路径与空值", () => {
    expect(lvqePureAec("/data/localvqe-v1.4-aec-200K-f32.gguf")).toBe(true);
    expect(lvqePureAec("localvqe-v1.3-4.8M-f32.gguf")).toBe(false);
    expect(lvqePureAec(null)).toBe(false);
    expect(lvqePureAec(undefined)).toBe(false);
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
