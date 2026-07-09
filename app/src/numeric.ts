// 遥测数值格式化的单一实现。
// null / undefined / NaN / Infinity 一律显示 "—"。
//
// 抽成共享模块的动因:此前 RuntimeSignalPanel 与 RuntimeStatusStrip 各写了一份
// `v === null ? "—" : v.toFixed(d)`,两处都只挡 null、漏了 undefined。后端某帧
// 遥测缺字段时值是 undefined(非 null)→ undefined.toFixed() 抛错 → 在无 Error
// Boundary 的树里卸载整个 app(黑屏)。单一实现 + == null + isFinite 根除该类。
export const dash = (v: number | null | undefined, d = 1): string =>
  v == null || !Number.isFinite(v) ? "—" : v.toFixed(d);
