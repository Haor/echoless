// 单色线稿图标。行尾功能图标须有语义;v6 起为工业蓝图线稿风
// (设计稿 overview.html v6 重绘,bh_037/060 母题),SVG 原样移植。

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

// INPUT:电容麦克风蓝图(capsule 线框 + 定位十字 + 侧刻度)。
export function IcoInput() {
  return (
    <svg
      viewBox="0 0 22 18"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.1"
    >
      <rect x="8.5" y="1.5" width="5" height="8" />
      <path d="M11 3.5 V7.5 M9 5.5 H13" />
      <path d="M11 9.5 V13.5 M7.5 15.5 H14.5" />
      <path d="M5.5 5.5 V9 M16.5 5.5 V9" />
    </svg>
  );
}

// MODEL:DSP 处理块蓝图(方框 + 信号引脚 + 滤波斜纹)。
export function IcoModel() {
  return (
    <svg
      viewBox="0 0 22 18"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.1"
    >
      <rect x="6.5" y="4.5" width="9" height="9" />
      <path d="M1.5 9 H6.5 M15.5 9 H20.5" />
      <path d="M8.5 11.5 L11.5 6.5 M11 11.5 L14 6.5" />
      <path d="M11 1.5 V3 M11 15 V16.5" />
    </svg>
  );
}

// OUTPUT:路由到虚拟麦端口(箭头 → 端口圆 + 端口芯)。
export function IcoOutput() {
  return (
    <svg
      viewBox="0 0 22 18"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.1"
    >
      <circle cx="16.5" cy="9" r="3.2" />
      <path d="M1.5 9 H11.5 M9 6.5 L11.5 9 L9 11.5" />
      <rect x="16" y="8.5" width="1" height="1" fill="currentColor" stroke="none" />
    </svg>
  );
}

// NOISE:噪声点场 + 划除线(抑制)。
export function IcoNoise() {
  return (
    <svg
      viewBox="0 0 22 18"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.1"
    >
      <g fill="currentColor" stroke="none">
        <rect x="4" y="3" width="1.6" height="1.6" />
        <rect x="10" y="2.4" width="1.6" height="1.6" />
        <rect x="16" y="4" width="1.6" height="1.6" />
        <rect x="5" y="8" width="1.6" height="1.6" />
        <rect x="11" y="7.4" width="1.6" height="1.6" />
        <rect x="16.6" y="9" width="1.6" height="1.6" />
        <rect x="4" y="13" width="1.6" height="1.6" />
        <rect x="10.4" y="12.4" width="1.6" height="1.6" />
        <rect x="16" y="13.6" width="1.6" height="1.6" />
      </g>
      <path d="M2.5 15.5 L19.5 2.5" />
    </svg>
  );
}
