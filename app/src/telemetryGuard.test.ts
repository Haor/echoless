import { describe, it, expect } from "vitest";
import { dash } from "./numeric";
import { statusToLive } from "./runtimeTelemetry";
import type { RuntimeStatus } from "./types";

// 回归:黑屏根因是 undefined 遥测值流进 .toFixed() 卸载整个 React 树。
// 两道防线各自锁死 —— 源头不产 undefined(statusToLive),消费端挡住任何非有限值(dash)。

describe("dash", () => {
  it("挡住 null / undefined / NaN / Infinity(全部显示 —)", () => {
    expect(dash(null)).toBe("—");
    expect(dash(undefined)).toBe("—");
    expect(dash(NaN)).toBe("—");
    expect(dash(Infinity)).toBe("—");
    expect(dash(-Infinity)).toBe("—");
  });

  it("正常数值走 toFixed", () => {
    expect(dash(-20.456)).toBe("-20.5");
    expect(dash(12, 0)).toBe("12");
    expect(dash(0)).toBe("0.0");
  });

  it("undefined 不再抛错(旧 `=== null` 守卫的黑屏根因)", () => {
    expect(() => dash(undefined)).not.toThrow();
  });
});

describe("statusToLive", () => {
  it("缺字段的 status 落成 null,绝不放 undefined 流出", () => {
    // 后端某帧只发部分字段(其余 key 缺失 → 前端读到 undefined)。
    const partial = { frames: 7 } as unknown as RuntimeStatus;
    const live = statusToLive(partial);
    expect(live.mic).toBeNull();
    expect(live.ref).toBeNull();
    expect(live.out).toBeNull();
    expect(live.lat).toBeNull();
    // 关键不变量:四个数值口全都不是 undefined(否则 dash 会崩)。
    for (const v of [live.mic, live.ref, live.out, live.lat]) {
      expect(v).not.toBe(undefined);
    }
  });

  it("完整 status 透传数值", () => {
    const full = {
      mic_dbfs: -20,
      ref_dbfs: -30,
      out_dbfs: -40,
      estimated_user_latency_ms: 12,
      frames: 5,
      diverged: false,
      runtime_errors: 0,
    } as unknown as RuntimeStatus;
    const live = statusToLive(full);
    expect(live.mic).toBe(-20);
    expect(live.ref).toBe(-30);
    expect(live.out).toBe(-40);
    expect(live.lat).toBe(12);
    expect(live.healthy).toBe(true);
  });

  it("端到端:缺字段 status → statusToLive → dash 不崩", () => {
    const live = statusToLive({ frames: 1 } as unknown as RuntimeStatus);
    expect(() => dash(live.mic)).not.toThrow();
    expect(dash(live.mic)).toBe("—");
  });
});
