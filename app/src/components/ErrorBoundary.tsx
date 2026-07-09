import { Component, type ErrorInfo, type ReactNode } from "react";

// React 渲染期异常的隔离墙。没有它,任意一处 render 抛错(例如某个遥测数值
// 为 undefined 时的 `.toFixed()`)都会卸载整棵组件树 —— #root 清空、整窗黑屏、
// 输入无响应。有了它,错误被限制在最近的边界内:顶层边界保证永不整屏黑死,
// 局部边界(如遥测面板)让故障只降级该子树、其余 UI 照常可用。
//
// 必须是 class 组件:getDerivedStateFromError / componentDidCatch 无 Hook 等价物。

interface Props {
  children: ReactNode;
  // 局部降级用:传一个静态占位(如遥测面板故障时的空槽)。
  // 不传则用顶层兜底 UI(错误提示 + 重试 / 重载)。
  fallback?: ReactNode;
  // 诊断标签,写进 console,便于定位是哪个边界捕获的。
  label?: string;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // 不吞异常:留到 console(DevTools / 日志可查),保留组件栈便于溯源。
    console.error(
      `[ErrorBoundary${this.props.label ? " " + this.props.label : ""}]`,
      error,
      info.componentStack,
    );
  }

  reset = () => this.setState({ error: null });

  render() {
    if (this.state.error) {
      if (this.props.fallback !== undefined) return this.props.fallback;
      // 顶层兜底:极简、不依赖任何应用状态或样式表,确保在最坏情况下仍可见。
      return (
        <div
          role="alert"
          style={{
            position: "fixed",
            inset: 0,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            gap: "14px",
            background: "#0a0a0a",
            color: "#e8e8e8",
            font: "13px/1.5 ui-monospace, monospace",
            letterSpacing: "0.02em",
            zIndex: 99999,
          }}
        >
          <div style={{ opacity: 0.7 }}>Interface error — the view was isolated to prevent a black screen.</div>
          <div style={{ display: "flex", gap: "10px" }}>
            <button
              type="button"
              onClick={this.reset}
              style={{
                padding: "6px 16px",
                background: "transparent",
                color: "#e8e8e8",
                border: "1px solid #444",
                cursor: "pointer",
                font: "inherit",
              }}
            >
              Retry
            </button>
            <button
              type="button"
              onClick={() => window.location.reload()}
              style={{
                padding: "6px 16px",
                background: "transparent",
                color: "#888",
                border: "1px solid #333",
                cursor: "pointer",
                font: "inherit",
              }}
            >
              Reload
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
