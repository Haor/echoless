import { useEffect, useRef, type MutableRefObject } from "react";

// 共享遥测(由 App 从 status 事件写入;canvas 按运行态/新波形事件绘制)。
export interface Telemetry {
  mic: number;
  ref: number;
  out: number;
  on: boolean;
  // 后端未来若提供真实降采样波形,这里直接用之;否则走合成包络。
  micWave?: number[];
  refWave?: number[];
  outWave?: number[];
}

type TraceKey = "mic" | "ref" | "out";

// 三路性格区分(亮度/幅度/毛糙度)。见 Design.md §7。
const CFG: Record<
  TraceKey,
  { base: number; f1: number; f2: number; nz: number; col: string; lw: number }
> = {
  mic: { base: 0.92, f1: 4.5, f2: 12, nz: 0.62, col: "222,223,228", lw: 2.0 },
  ref: { base: 0.55, f1: 2.4, f2: 6.6, nz: 0.2, col: "104,107,117", lw: 1.6 },
  out: { base: 0.66, f1: 3.3, f2: 7.7, nz: 0.16, col: "244,244,246", lw: 2.3 },
};

function clamp01(v: number) {
  return Math.max(0, Math.min(1, v));
}

// OFF(穿透/停机)态的波形灰(--t-mut 系):功能照常,视觉降级。
const DIM_COL = "118, 117, 112";

export function Scope({
  traceKey,
  telRef,
  active,
  revision,
  phase,
  dimmed = false,
}: {
  traceKey: TraceKey;
  telRef: MutableRefObject<Telemetry>;
  active: boolean;
  revision: number;
  phase: number;
  // true = 波形改灰色低亮(OFF 直通中仍在流动,但退出视觉主角)
  dimmed?: boolean;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const activeRef = useRef(active);
  const dimmedRef = useRef(dimmed);
  const scheduleRef = useRef<() => void>(() => {});

  activeRef.current = active;
  dimmedRef.current = dimmed;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const x = canvas.getContext("2d");
    if (!x) return;
    const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const g = CFG[traceKey];
    let prof: Float32Array | null = null;
    let lastN = 0;
    let acc = 0;
    let last = performance.now();
    let raf = 0;
    const size = {
      w: Math.max(1, canvas.clientWidth | 0),
      h: Math.max(1, canvas.clientHeight | 0),
      dpr: Math.min(2, window.devicePixelRatio || 1),
    };

    const applyCanvasSize = () => {
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      const bw = Math.max(1, Math.round(size.w * dpr));
      const bh = Math.max(1, Math.round(size.h * dpr));
      if (canvas.width !== bw || canvas.height !== bh) {
        canvas.width = bw;
        canvas.height = bh;
      }
      size.dpr = dpr;
      return dpr;
    };

    const buildProfile = (n: number) => {
      const a = new Float32Array(n);
      for (let i = 0; i < n; i++) {
        const fx = i / (n - 1);
        let v =
          Math.sin(fx * g.f1 * 6.283 + phase) * 0.62 +
          Math.sin(fx * g.f2 * 6.283 + 1.3) * 0.3;
        v += (Math.random() - 0.5) * g.nz;
        a[i] = v;
      }
      prof = a;
    };

    const shouldAnimate = () => {
      if (reduce || document.hidden || !activeRef.current) return false;
      const tel = telRef.current;
      if (!tel.on) return false;
      const wave = tel[`${traceKey}Wave` as const];
      return !(wave && wave.length > 1);
    };

    const draw = () => {
      const tel = telRef.current;
      const dpr = applyCanvasSize();
      const w = size.w;
      const h = size.h;
      x.setTransform(dpr, 0, 0, dpr, 0, 0);
      x.clearRect(0, 0, w, h);

      const step = 7;
      const n = Math.max(4, Math.floor(w / step) + 1);
      const mid = h / 2;
      const maxA = h / 2 - 3;

      // v13:点划中心线(机械制图 centerline;25/75 基准线已删,线条减法)
      x.setLineDash([9, 4, 2, 4]);
      x.strokeStyle = "rgba(214,213,205,0.08)";
      x.lineWidth = 1;
      x.beginPath();
      x.moveTo(0, mid);
      x.lineTo(w, mid);
      x.stroke();
      x.setLineDash([]);

      const wave = tel[`${traceKey}Wave` as const];
      // 量程 0..-120 dBFS,与 srail 标注、dB 读数下限一致。
      const e = clamp01((tel[traceKey] + 120) / 120) * (tel.on ? 1 : 0.05);

      let pts: [number, number][];
      if (wave && wave.length > 1) {
        // 真实波形:后端给的是 [0,1] peak 包络。用每桶真实 peak 调制一条
        // 载波(过中心轴上下摆),得到「真实振幅 + 平滑示波」的曲线。
        const nW = wave.length;
        const ampF = (tel.on ? 1 : 0.06) * maxA;
        pts = [];
        for (let i = 0; i < nW; i++) {
          const carrier = Math.sin(i * 0.85 + phase);
          const xx = (i / (nW - 1)) * (w - 1);
          pts.push([xx, mid - wave[i] * carrier * ampF]);
        }
      } else {
        // 合成包络:固定结构 + 每 ~150ms 原地刷新噪声,幅度由 dBFS 驱动。
        if (!prof || lastN !== n) {
          lastN = n;
          buildProfile(n);
        }
        const amp = e * g.base * maxA;
        pts = [];
        for (let i = 0; i < n; i++) pts.push([i * step, mid - prof![i] * amp]);
      }

      x.beginPath();
      x.moveTo(pts[0][0], pts[0][1]);
      for (let i = 1; i < pts.length - 1; i++) {
        const xc = (pts[i][0] + pts[i + 1][0]) / 2;
        const yc = (pts[i][1] + pts[i + 1][1]) / 2;
        x.quadraticCurveTo(pts[i][0], pts[i][1], xc, yc);
      }
      x.lineTo(pts[pts.length - 1][0], pts[pts.length - 1][1]);
      x.strokeStyle = dimmedRef.current
        ? `rgba(${DIM_COL},${tel.on ? 0.6 : 0.4})`
        : `rgba(${g.col},${tel.on ? 0.95 : 0.45})`;
      x.lineWidth = g.lw;
      x.lineCap = "round";
      x.lineJoin = "round";
      x.stroke();
    };

    const frame = (now: number) => {
      raf = 0;
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      const animate = shouldAnimate();
      if (animate) {
        acc += dt;
        if (acc > 0.15) {
          acc = 0;
          prof = null; // 触发原地刷新
        }
      }
      draw();
      if (animate) {
        raf = requestAnimationFrame(frame);
      }
    };

    const schedule = () => {
      if (document.hidden || raf) return;
      raf = requestAnimationFrame(frame);
    };
    const stop = () => {
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
    };
    const onVisibility = () => {
      if (document.hidden) stop();
      else schedule();
    };
    const resizeObserver = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      if (box) {
        size.w = Math.max(1, box.width | 0);
        size.h = Math.max(1, box.height | 0);
      }
      prof = null;
      schedule();
    });
    resizeObserver.observe(canvas);
    document.addEventListener("visibilitychange", onVisibility);
    scheduleRef.current = schedule;
    schedule();
    return () => {
      stop();
      resizeObserver.disconnect();
      document.removeEventListener("visibilitychange", onVisibility);
      scheduleRef.current = () => {};
    };
  }, [traceKey, telRef, phase]);

  useEffect(() => {
    scheduleRef.current();
  }, [active, revision, dimmed]);

  return <canvas ref={canvasRef} data-w={traceKey} />;
}

// footer 微型电平条:绑 out_dbfs(真实活动指示,非装饰)。
const BAR_ENV = [0.45, 0.7, 1, 0.8, 0.55, 0.32];
export function FooterBars({
  telRef,
  active,
  revision,
}: {
  telRef: MutableRefObject<Telemetry>;
  active: boolean;
  revision: number;
}) {
  const wrapRef = useRef<HTMLSpanElement>(null);
  const activeRef = useRef(active);
  const scheduleRef = useRef<() => void>(() => {});

  activeRef.current = active;

  useEffect(() => {
    let raf = 0;
    let t = 0;
    let last = performance.now();
    const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const frame = (now: number) => {
      raf = 0;
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      if (!reduce && activeRef.current && !document.hidden) t += dt;
      const tel = telRef.current;
      const eo = clamp01((tel.out + 60) / 60) * (tel.on ? 1 : 0.18);
      const bars = wrapRef.current?.children;
      if (!bars) return;
      for (let i = 0; i < bars.length; i++) {
        const j = 0.65 + Math.sin(t * 9 + i * 1.25) * 0.35;
        const hgt = Math.min(12, 2 + eo * BAR_ENV[i] * j * 12);
        (bars[i] as HTMLElement).style.height = `${hgt.toFixed(1)}px`;
      }
      if (!reduce && activeRef.current && !document.hidden) {
        raf = requestAnimationFrame(frame);
      }
    };

    const schedule = () => {
      if (document.hidden || raf) return;
      raf = requestAnimationFrame(frame);
    };
    const stop = () => {
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
    };
    const onVisibility = () => {
      if (document.hidden) stop();
      else schedule();
    };
    document.addEventListener("visibilitychange", onVisibility);
    scheduleRef.current = schedule;
    schedule();
    return () => {
      stop();
      document.removeEventListener("visibilitychange", onVisibility);
      scheduleRef.current = () => {};
    };
  }, [telRef]);

  useEffect(() => {
    scheduleRef.current();
  }, [active, revision]);

  return (
    <span className="bg" ref={wrapRef}>
      {BAR_ENV.map((_, i) => (
        <i key={i} />
      ))}
    </span>
  );
}
