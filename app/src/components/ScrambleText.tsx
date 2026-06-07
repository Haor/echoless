import { useEffect, useRef } from "react";
import { animate, scrambleText } from "animejs";

// 字符 scramble 文本:text 变化(或 trigger 变化)时,用 anime.js 把内容
// 从乱码 ░▒▓ settle 到目标文本,避免硬切。首次挂载不动画。
export function ScrambleText({
  text,
  trigger,
  className,
}: {
  text: string;
  trigger?: unknown;
  className?: string;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const lastText = useRef<string | null>(null);
  const lastTrig = useRef<unknown>(undefined);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // 首次:直接写入,不动画(也避开 StrictMode 双调用的误触发)
    if (lastText.current === null) {
      el.textContent = text;
      lastText.current = text;
      lastTrig.current = trigger;
      return;
    }
    if (lastText.current === text && lastTrig.current === trigger) return;
    lastText.current = text;
    lastTrig.current = trigger;
    animate(el, {
      // scrambleText 作为 innerHTML 的目标值(anime.js v4 文本插件)
      innerHTML: scrambleText({
        text,
        from: "center",
        duration: 520,
        cursor: "░▒▓",
        ease: "inOut",
        override: false,
      }),
    } as never);
  }, [text, trigger]);

  return <span ref={ref} className={className} />;
}
