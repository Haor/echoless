import { useEffect, useRef } from "react";
import { animate, scrambleText, utils } from "animejs";
import { useI18n } from "../i18n";

type TextSink = { textContent: string | null };

export function writeSafeText(target: TextSink, text: string) {
  target.textContent = text;
}

// 字符 scramble 文本:text 变化(或 trigger 变化)时,用 anime.js 把内容
// 从乱码 ░▒▓ settle 到目标文本,避免硬切。首次挂载不动画。
export function ScrambleText({
  text,
  trigger,
  className,
  cursor = "░▒▓",
}: {
  text: string;
  trigger?: unknown;
  className?: string;
  // reveal 波前沿字符。短文本(如 POWER 的 ON/OFF)+ text 长期不变时,前沿
  // 残留没有后续 scramble 覆盖会持久可见 —— 这类场景传 false 关掉前沿。
  cursor?: string | false;
}) {
  const { lang } = useI18n();
  const ref = useRef<HTMLSpanElement>(null);
  const lastText = useRef<string | null>(null);
  const lastTrig = useRef<unknown>(undefined);
  const lastLang = useRef(lang);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // 首次:直接写入,不动画(也避开 StrictMode 双调用的误触发)
    if (lastText.current === null) {
      writeSafeText(el, text);
      lastText.current = text;
      lastTrig.current = trigger;
      lastLang.current = lang;
      return;
    }
    // 语言切换:文本因翻译而变,直接写入、不播 scramble。否则整屏 ScrambleText
    // 会齐刷刷重播,cursor 块字符逐帧改宽,标题栏斜纹/状态条随之抖动(用户反馈
    // 的「切语言细微跳动」)。真正的状态/数值/导航变化(语言不变)仍照常 scramble。
    if (lastLang.current !== lang) {
      lastLang.current = lang;
      lastText.current = text;
      lastTrig.current = trigger;
      writeSafeText(el, text);
      return;
    }
    if (lastText.current === text && lastTrig.current === trigger) return;
    lastText.current = text;
    lastTrig.current = trigger;
    // Let anime.js mutate only a plain object, then copy each frame as text.
    // Untrusted device names never enter the browser's HTML parser.
    const animationTarget: TextSink = { textContent: el.textContent ?? "" };
    animate(animationTarget, {
      textContent: scrambleText({
        text,
        from: "center",
        duration: 520,
        cursor,
        ease: "inOut",
        override: false,
      }),
      onUpdate: () => {
        writeSafeText(el, animationTarget.textContent ?? "");
      },
      onComplete: () => {
        writeSafeText(el, text);
      },
    } as never);
    return () => {
      // Settle interruptions to target text and stop the detached animation target.
      utils.remove(animationTarget);
      writeSafeText(el, text);
    };
  }, [text, trigger, cursor, lang]);

  return <span ref={ref} className={className} />;
}
