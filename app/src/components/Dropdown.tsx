import { useEffect, useRef, useState } from "react";
import { ScrambleText } from "./ScrambleText";

export interface DropdownOption {
  value: string;
  label: string;
}

// 自绘下拉:单色直角,与 brutalist 主题统一(原生 <select> 弹层无法套主题)。
export function Dropdown({
  value,
  options,
  onChange,
  align = "left",
  compact = false,
  warn = false,
}: {
  value: string;
  options: DropdownOption[];
  onChange: (v: string) => void;
  align?: "left" | "right";
  compact?: boolean;
  warn?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const cur = options.find((o) => o.value === value);

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className={`dd ${compact ? "dd-sm" : ""}`} ref={rootRef}>
      <button
        className={`dd-trigger ${warn ? "warn" : ""}`}
        onClick={() => setOpen((v) => !v)}
        type="button"
      >
        <span className="lbl">
          <ScrambleText text={cur?.label ?? value} />
        </span>
        <span className="dn">▾</span>
      </button>
      {open && (
        <div className={`dd-panel ${align === "right" ? "right" : ""}`}>
          {options.length === 0 && <div className="dd-empty">no devices</div>}
          {options.map((o) => (
            <button
              key={o.value}
              type="button"
              className={`dd-opt ${o.value === value ? "sel" : ""}`}
              onClick={() => {
                onChange(o.value);
                setOpen(false);
              }}
            >
              <span className="mk">{o.value === value ? "■" : ""}</span>
              <span className="otext">{o.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
