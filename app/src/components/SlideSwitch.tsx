import { ScrambleText } from "./ScrambleText";

// 物理滑动开关:主体方块在条纹轨道里左右滑动 + 标签 scramble。
// 首页主开关与 Diagnostics 录制共用。
export function SlideSwitch({
  on,
  onToggle,
  disabled,
  small,
  onLabel = "ON",
  offLabel = "OFF",
}: {
  on: boolean;
  onToggle: () => void;
  disabled?: boolean;
  small?: boolean;
  onLabel?: string;
  offLabel?: string;
}) {
  return (
    <button
      type="button"
      className={`power ${on ? "on" : "off"} ${small ? "sm" : ""}`}
      disabled={disabled}
      onClick={onToggle}
    >
      <span className="slider">
        <ScrambleText text={on ? onLabel : offLabel} />
      </span>
    </button>
  );
}
