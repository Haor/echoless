import { useCallback, useEffect, useRef } from "react";
import { animate, scrambleText, utils } from "animejs";
import { outputLevelToGain } from "../api";

// 输出音量(最终送给虚拟麦克风的人声)。刻度 0-100:0=静音 / 50=原声(0dB) / 100=3x(+9.542dB)。
// 鼠标悬停 + 滚轮调节。暂为前端 UI,后端对输出样本乘 output_gain 后即生效。
const VOL_MIN = 0;
const VOL_MAX = 100;
// 滚动放慢:累积 deltaY 到阈值才走 1 格(触控板连发也不会飞)。
const SCROLL_THRESHOLD = 120;

// 悬停时旁边浮现的当前 dB(字符动画),随音量实时变。
function dbLabel(volume: number): string {
  const g = outputLevelToGain(volume);
  if (g <= 0) return "mute";
  const db = 20 * Math.log10(g);
  return `${db >= 0 ? "+" : ""}${db.toFixed(1)} dB`;
}

export function VolumeWheel({
  volume,
  onChange,
}: {
  volume: number;
  onChange: (v: number) => void;
}) {
  const vRef = useRef(volume);
  vRef.current = volume;
  const acc = useRef(0); // 累积滚动量,到阈值才走一格
  const hoverRef = useRef(false);
  const dbRef = useRef<HTMLSpanElement>(null);
  const shown = useRef(false); // dB 是否已浮现(区分「首次出现」与「滚动更新」)

  const showDb = useCallback((animated: boolean) => {
    const el = dbRef.current;
    if (!el) return;
    const text = ` · ${dbLabel(vRef.current)}`;
    if (animated && !shown.current) {
      shown.current = true;
      animate(el, {
        innerHTML: scrambleText({
          text,
          from: "center",
          duration: 480,
          cursor: "░▒▓",
          ease: "inOut",
          override: false,
        }),
      } as never);
    } else {
      utils.remove(el);
      el.textContent = text;
    }
  }, []);

  const hideDb = useCallback(() => {
    const el = dbRef.current;
    if (!el) return;
    utils.remove(el); // 停掉在飞的 scramble,否则它继续写 innerHTML、移开不立即收起
    el.textContent = "";
    shown.current = false;
  }, []);

  useEffect(() => {
    if (hoverRef.current) showDb(false);
  }, [volume, showDb]);

  return (
    <span
      className="vol"
      onMouseEnter={() => {
        hoverRef.current = true;
        showDb(true);
      }}
      onMouseLeave={() => {
        hoverRef.current = false;
        hideDb();
      }}
      onWheel={(e) => {
        e.preventDefault();
        acc.current += e.deltaY;
        if (Math.abs(acc.current) < SCROLL_THRESHOLD) return;
        const dir = acc.current < 0 ? 1 : -1; // 向上滚 = 增大
        acc.current = 0; // 每格只走 1,封顶速度
        const next = Math.max(VOL_MIN, Math.min(VOL_MAX, vRef.current + dir));
        if (next !== vRef.current) onChange(next);
      }}
    >
      VOL {volume}
      {/* 悬停浮现「 · +5.4 dB」,点分隔同其它参数 */}
      <span ref={dbRef} className="voldb" />
    </span>
  );
}
