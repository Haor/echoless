// 单色线稿图标。行尾功能图标须有语义(见 Design.md §2.6)。

export function AppIcon() {
  // Level-Bars / E:三条递减电平横杠 = 字母 E + 音频电平。
  return (
    <svg
      className="appicon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="square"
    >
      <path d="M5 7 H17" />
      <path d="M5 12 H21" />
      <path d="M5 17 H13" />
    </svg>
  );
}

export function CapMin() {
  return (
    <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1">
      <path d="M2 6 H10" />
    </svg>
  );
}
export function CapMax() {
  return (
    <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1">
      <rect x="2.5" y="2.5" width="7" height="7" />
    </svg>
  );
}
export function CapClose() {
  return (
    <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1">
      <path d="M2.5 2.5 L9.5 9.5 M9.5 2.5 L2.5 9.5" />
    </svg>
  );
}

// INPUT:麦克风(近端采集设备)。替换原 level-bars,语义更准。
export function IcoInput() {
  return (
    <svg
      viewBox="0 0 22 18"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <rect x="8.5" y="1.5" width="5" height="9" rx="2.5" />
      <path d="M5.5 8 a5.5 5.5 0 0 0 11 0" />
      <path d="M11 13.5 V16 M8 16 H14" />
    </svg>
  );
}

// MODEL:DSP 处理块(斜纹方块)。保留。
export function IcoModel() {
  return (
    <svg viewBox="0 0 22 18" fill="none" stroke="currentColor" strokeWidth="1.3">
      <rect x="2" y="1" width="16" height="16" />
      <path d="M2 11 L9 1 M2 16 L17 2 M8 16 L17 8 M14 16 L17 13" />
    </svg>
  );
}

// OUTPUT:信号送出到端点(箭头 → 竖杠)。替换原准星,语义为「路由到输出设备」。
export function IcoOutput() {
  return (
    <svg
      viewBox="0 0 22 18"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M2 9 H13" />
      <path d="M9.5 5 L13.5 9 L9.5 13" />
      <path d="M18 3 V15" />
    </svg>
  );
}

// NOISE:噪声场(点阵)。保留。
export function IcoNoise() {
  return (
    <svg viewBox="0 0 22 18" fill="currentColor">
      {[3, 9, 15].map((y) =>
        [4, 11, 18].map((x) => (
          <circle key={`${x}-${y}`} cx={x} cy={y} r="1.4" />
        )),
      )}
    </svg>
  );
}
