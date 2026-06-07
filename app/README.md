# Echoless — Desktop GUI (Tauri)

Echoless 控制电器主界面的 Tauri v2 + React/TS 实现。视觉/交互定稿真理源:
`AEC/Design/overview.html` + `AEC/Design/Design.md`。

## 架构

```
React/TS UI  ──invoke──▶  Tauri (Rust, src-tauri/src/lib.rs)
   ▲                          │ spawn sidecar
   └── echoless://status ◀────┴── echoless CLI (--status-json JSONL)
```

- 只消费 JSON / JSONL 契约(`types.ts` 镜像后端形状),不解析人类日志(走 stderr → `echoless://log`)。
- 一次性命令:`list_devices` / `list_processors` / `validate_config`。
- 实时:`start_run` spawn `echoless run --status-json --stats-interval-ms 80`,逐行解析后经 `echoless://status` 事件推前端;`stop_run` kill 子进程;关窗自动 kill。

## echoless 二进制定位

`src-tauri/src/lib.rs::echoless_bin()`:
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

- **macOS**:`TitleBarStyle::Overlay` + `hidden_title`,保留系统红绿灯(OS 绘制,左上);标题栏左侧预留 82px。
- **Windows / Linux**:`decorations(false)` + `shadow(true)`,自绘 caption 按钮(右上 `─ □ ✕`,close hover 红)。

平台由 `get_platform` 命令返回,前端切 `.window.mac` / `.window.win`。

## 已知 TODO

- **真实波形**:目前三路示波是 dBFS 包络驱动的合成曲线(`Scope.tsx`)。后端若在 status event 加入 `mic_wave/ref_wave/out_wave`(降采样 peak/RMS 数组),`Scope` 已前向兼容,会自动改画真实波形。
- **macOS 红绿灯垂直居中**:当前用 Overlay 默认位置。需要 inset 时接 `tauri-plugin-decorum` 的 `set_traffic_lights_inset` 或 trafficlights-positioner。
- **sidecar 打包**:dev 走相对路径;`tauri build` 需把 `echoless` 配成 `externalBin` 并由 `ECHOLESS_BIN` 注入路径。
- **虚拟声卡引导**:等后端 `doctor audio --json`(检测 VB-CABLE/BlackHole 安装态)。
- **Advanced / Diagnostics**:页面待设计(footer 入口已占位)。
- LocalVQE / RTX 选中后需各自必填参数(model 路径等),当前仅 AEC3 全链路打通。
