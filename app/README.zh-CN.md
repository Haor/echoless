# Echoless — Desktop GUI (Tauri)

[English](README.md) | 简体中文

Echoless 控制电器主界面的 Tauri v2 + React/TS 实现。视觉/交互定稿真理源:
`AEC/Design/overview.html` + `AEC/Design/Design.md`。

## 架构

```
React/TS UI  ──invoke──▶  Tauri (Rust, src-tauri/src/*)
   ▲                          │ spawn sidecar
   └── echoless://status ◀────┴── echoless CLI (--status-json JSONL)
```

Rust 侧按职责拆模块(`src-tauri/src/`):`lib.rs` 仅入口 + setup;
`sidecar.rs`(run 生命周期/热命令)、`bin_resolve.rs`(二进制定位)、
`proc.rs`、`localvqe.rs`、`nvafx.rs`、`platform.rs`、`device_watch.rs`、
`tray.rs`、`commands.rs`(`#[tauri::command]` 薄封装)。

- 只消费 JSON / JSONL 契约(`types.ts` 镜像后端形状),不解析人类日志(走 stderr → `echoless://log`)。
- 一次性命令:`list_devices` / `list_processors` / `validate_config`。
- 实时:`start_run`(`sidecar.rs`)spawn `echoless run --status-json --stats-interval-ms 80`,逐行解析后经 `echoless://status` 事件推前端;`stop_run` 关 stdin→限时→kill(优雅停机);退出(关窗/Cmd+Q/ExitRequested)自动回收子进程。

## echoless 二进制定位

`src-tauri/src/bin_resolve.rs::echoless_bin()`:
1. 环境变量 `ECHOLESS_BIN`(打包后由 sidecar 资源注入)
2. dev 回退:`../../target/release/echoless`(即本仓库 `cargo build --release` 产物)

dev 前先在仓库根构建 CLI:

```bash
cd ..            # echoless/
cargo build --release -p echoless-cli
```

## 跑起来

```bash
pnpm install
pnpm tauri dev          # 开发(热重载前端 + Rust)
pnpm tauri build        # 打包
```

## 平台标题栏(Design.md §5.1)

程序化建窗(`lib.rs` setup),平台镜像:

- **macOS**:`TitleBarStyle::Overlay` + `hidden_title`,保留系统红绿灯(OS 绘制,左上);`set_traffic_lights_inset(16,13)` 把红绿灯居中到 40px 标题栏。
- **Windows / Linux**:`decorations(false)` + `shadow(true)`,自绘 caption 按钮(右上 `─ □ ✕`,close hover 红)。

平台由 `get_platform` 命令返回,前端切 `.window.mac` / `.window.win`。

窗口以 `visible(false)` 创建,前端首屏就绪(`booted`,字体+首批数据就位,
1.2s 硬封顶)后经 core window show 权限亮窗——根除 WebView 初始化白闪;
Rust 侧另有 5s 兜底防前端崩溃导致窗口不出现。

## 当前边界

- **真实波形**:后端已在 status event 输出 `mic_wave/ref_wave/out_wave`(64 桶 peak 包络),`Scope.tsx` 直接绘制;无波形字段时才回退合成包络。
- **sidecar 打包**:`tauri.conf.json` 已声明 `externalBin` 和 bundle resources(含 `licenses/` 三方许可);本地打包前仍需先构建 release CLI 并运行 `pnpm prepare:tauri-assets`。
- **虚拟声卡引导**:Mic Setup 已接入 `doctor audio --json` 检测与平台提示;驱动安装仍由用户完成。
- **Advanced / Diagnostics**:页面已可用;新增诊断字段时同步更新 `types.ts` 与页面显示。
- **LocalVQE / RTX**:LocalVQE 模型下载/选择与 RTX runtime 安装向导已接入,但仍依赖对应平台原生资产可用。
