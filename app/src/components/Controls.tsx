import { useState } from "react";

// 选项少 → 一排按钮(不做下拉)。
export function SegButtons<T extends string>({
  value,
  options,
  onChange,
  disabled = false,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  disabled?: boolean;
}) {
  return (
    <div
      className={`segg ${disabled ? "dim" : ""}`}
      aria-disabled={disabled || undefined}
    >
      {options.map((o) => (
        <button
          type="button"
          key={o.value}
          className={`b ${o.value === value ? "active" : ""}`}
          disabled={disabled}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

// number / string / path 输入:本地编辑,blur 或 Enter 提交。空 = null(生成 TOML 时省略)。
export function Field({
  value,
  numeric,
  placeholder,
  onCommit,
  wide,
  min,
  max,
  integer,
}: {
  value: unknown;
  numeric: boolean;
  placeholder: string;
  onCommit: (v: unknown) => void;
  wide?: boolean;
  min?: number;
  max?: number;
  integer?: boolean;
}) {
  const valueText = value == null ? "" : String(value);
  const [draft, setDraft] = useState({ source: valueText, text: valueText });
  const txt = draft.source === valueText ? draft.text : valueText;
  const commit = () => {
    const s = txt.trim();
    if (s === "") return onCommit(null);
    if (numeric) {
      let n = Number(s);
      if (!Number.isFinite(n)) return onCommit(null);
      if (integer) n = Math.round(n);
      if (min != null) n = Math.max(min, n);
      if (max != null) n = Math.min(max, n);
      return onCommit(n);
    }
    onCommit(s);
  };
  return (
    <input
      className={`afield ${wide ? "wide" : ""}`}
      value={txt}
      placeholder={placeholder}
      aria-label={placeholder}
      inputMode={numeric ? (integer ? "numeric" : "decimal") : "text"}
      min={min}
      max={max}
      step={numeric && integer ? 1 : undefined}
      spellCheck={false}
      onChange={(e) => setDraft({ source: valueText, text: e.target.value })}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
      }}
    />
  );
}
