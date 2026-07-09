import {
  cloneElement,
  isValidElement,
  useRef,
  useState,
  type ReactElement,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

// 自绘悬浮提示:单色直角,匹配主题;短延迟弹出(比原生 title 快)。
// 全 app 统一的 tooltip 入口 —— 禁止再用原生 title=(样式突兀且各端不一致)。
//
// 弹层经 portal 挂到 body、position:fixed 定位:锚点在 overflow:hidden 的
// 截断容器(.dpath/.dpick/.cdetail)里也不会被裁剪。.window 无 transform,
// fixed 相对视口定位安全。
//
// 两种用法:
//   包裹模式(默认):<Hint text=...><span className="alabel">…</span></Hint>
//     —— 外加一层 span.hint(inline-flex)。适合标签类小元素(AdvancedPage 惯例)。
//   附着模式:<Hint text=... attach><button …/></Hint>
//     —— cloneElement 只挂 onMouseEnter/Leave,不加任何 DOM 包裹,零布局影响。
//     适合 flex/grid 里尺寸敏感的元素(截断按钮、路径、整行)。
// pos:默认向下弹;下方紧跟动态内容时传 "top" 往上弹,避免遮挡。
export function Hint({
  text,
  children,
  pos = "bottom",
  attach = false,
}: {
  text?: string;
  children: ReactNode;
  pos?: "top" | "bottom";
  attach?: boolean;
}) {
  const [box, setBox] = useState<{ x: number; y: number; up: boolean } | null>(
    null,
  );
  const timer = useRef<number | undefined>(undefined);
  if (!text) return <>{children}</>;

  const onEnter = (e: React.MouseEvent) => {
    const el = e.currentTarget as HTMLElement;
    timer.current = window.setTimeout(() => {
      const r = el.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) return; // 已卸载
      const up = pos === "top";
      // 水平钳制:max-width 260 + padding,别弹出视口右缘
      const x = Math.max(8, Math.min(r.left, window.innerWidth - 276));
      setBox({
        x,
        y: up ? window.innerHeight - r.top + 6 : r.bottom + 6,
        up,
      });
    }, 240);
  };
  const onLeave = () => {
    clearTimeout(timer.current);
    setBox(null);
  };

  const pop =
    box &&
    createPortal(
      <span
        className="hint-pop"
        style={
          box.up ? { left: box.x, bottom: box.y } : { left: box.x, top: box.y }
        }
      >
        {text}
      </span>,
      document.body,
    );

  if (attach && isValidElement(children)) {
    return (
      <>
        {cloneElement(children as ReactElement<Record<string, unknown>>, {
          onMouseEnter: onEnter,
          onMouseLeave: onLeave,
        })}
        {pop}
      </>
    );
  }
  return (
    <span className="hint" onMouseEnter={onEnter} onMouseLeave={onLeave}>
      {children}
      {pop}
    </span>
  );
}
