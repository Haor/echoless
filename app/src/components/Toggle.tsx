// 极简方形滑块开关(ON 绿 / 右,OFF 暗 / 左)。无条纹、无文字、无 scramble。
export function Toggle({
  on,
  onToggle,
  disabled,
}: {
  on: boolean;
  onToggle: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className={`toggle ${on ? "on" : ""}`}
      disabled={disabled}
      onClick={onToggle}
      aria-pressed={on}
      aria-label={on ? "on" : "off"}
    >
      <span className="knob" />
    </button>
  );
}
