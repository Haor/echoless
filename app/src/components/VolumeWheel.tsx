import { useEffect, useRef, useState } from "react";
import { animate, scrambleText, utils } from "animejs";
import { outputLevelToGain } from "../api";

// 输出音量(最终送给虚拟麦克风的人声)。刻度 0-100:0=静音 / 50=原声(0dB) / 100=3x(+9.542dB)。
// 鼠标悬停 + 滚轮调节。暂为前端 UI,后端对输出样本乘 output_gain 后即生效。
export const VOL_MIN = 0;
export const VOL_MAX = 100;
export const VOL_UNITY = 50;
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
  const ref = useRef<HTMLSpanElement>(null);
  const vRef = useRef(volume);
  vRef.current = volume;
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const acc = useRef(0); // 累积滚动量,到阈值才走一格
  const [hover, setHover] = useState(false);
  const dbRef = useRef<HTMLSpanElement>(null);
  const shown = useRef(false); // dB 是否已浮现(区分「首次出现」与「滚动更新」)

  // 非 passive 原生监听,才能 preventDefault 阻止页面跟着滚。
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      acc.current += e.deltaY;
      if (Math.abs(acc.current) < SCROLL_THRESHOLD) return;
      const dir = acc.current < 0 ? 1 : -1; // 向上滚 = 增大
      acc.current = 0; // 每格只走 1,封顶速度
      const next = Math.max(VOL_MIN, Math.min(VOL_MAX, vRef.current + dir));
      if (next !== vRef.current) onChangeRef.current(next);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // dB 显示:悬停首次浮现 → 字符动画 scramble 一次;之后随音量直接更新数值(不再每步动画);
  // 移开光标 → 清空消失。
  useEffect(() => {
    const el = dbRef.current;
    if (!el) return;
    if (!hover) {
      utils.remove(el); // 停掉在飞的 scramble,否则它继续写 innerHTML、移开不立即收起
      el.textContent = "";
      shown.current = false;
      return;
    }
    const text = ` · ${dbLabel(volume)}`;
    if (!shown.current) {
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
      utils.remove(el); // 滚动中:取消未播完的浮现动画,直接显示数值动态变化
      el.textContent = text;
    }
  }, [hover, volume]);

  return (
    <span
      ref={ref}
      className="vol"
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      VOL {volume}
      {/* 悬停浮现「 · +5.4 dB」,点分隔同其它参数 */}
      <span ref={dbRef} className="voldb" />
    </span>
  );
}
