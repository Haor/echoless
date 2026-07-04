// 纯浏览器预览垫片(vite dev / headless 截图用)。
// 真 Tauri 环境已有 __TAURI_INTERNALS__,本文件零作用;
// 浏览器里把 invoke 全部置为 pending(不 resolve 不 reject),
// UI 停留在初始状态、不弹后端错误,布局/动效可正常核查。
type AnyObj = Record<string, unknown>;
const w = window as unknown as AnyObj;
// 真 Tauri 环境(dev 与打包产物)注入时机早于任何模块执行,此分支只会在纯浏览器进入。
if (!w.__TAURI_INTERNALS__) {
  w.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main", windowLabel: "main" },
    },
    plugins: {},
    transformCallback: (cb: unknown) => cb,
    invoke: () => new Promise(() => {}),
  };
}

export {};
