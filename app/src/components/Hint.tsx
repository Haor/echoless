import { useRef, useState, type ReactNode } from "react";

// 自绘悬浮提示:单色直角,匹配主题;短延迟弹出(比原生 title 快)。
// pos:默认向下弹(标签场景下方多为空白);下方紧跟动态内容(如 RUN PROBE
// 按钮下就是进度灯/结果)时传 "top" 往上弹,避免遮挡。
export function Hint({
  text,
  children,
  pos = "bottom",
}: {
  text?: string;
  children: ReactNode;
  pos?: "top" | "bottom";
}) {
  const [show, setShow] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  if (!text) return <>{children}</>;
  return (
    <span
      className="hint"
      onMouseEnter={() => {
        timer.current = window.setTimeout(() => setShow(true), 240);
      }}
      onMouseLeave={() => {
        clearTimeout(timer.current);
        setShow(false);
      }}
    >
      {children}
      {show && <span className={`hint-pop ${pos}`}>{text}</span>}
    </span>
  );
}
