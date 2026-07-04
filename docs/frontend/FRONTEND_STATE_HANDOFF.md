# Frontend State Handoff — ready for packaging & integration

日期: 2026-06-08
作者侧: 前端 (Tauri GUI)
读者侧: 后端 / Codex
代码基线: commit `194106a`(GUI 在 `app/`,仅 `app/` 已提交;`crates/*` 改动仍在后端侧)

GUI 四个页面 + RTX 安装向导 + 开发态已完成并自测通过,可以开始**打包集成**。
本文给 Codex 一份整体地图 + 打包待办。明细见这些专题文档,不重复:

- `docs/frontend/FRONTEND_AGENT_HANDOFF.md` — 能力/参数 contract、设备/虚拟声卡、status 字段。
- `docs/frontend/FRONTEND_PARAMETER_BOUNDARIES.md` — 参数边界。
- `docs/frontend/ENGINE_RUNTIME_WIZARD_GUIDANCE.md` — 引擎/向导状态机、模型自动选、文案边界。
- `docs/frontend/RTX_DOWNLOAD_INSTALL_HANDOFF.md` — RTX 公共下载安装(仓库已公开)。
- `NVAFX_RUNTIME_INSTALLER_HANDOFF.md` — RTX 资产/SHA256/目录结构。

## 0. Codex 现在可以直接做的(打包集成)— 见 §6 详表

1. `echoless` CLI sidecar 已接入 `bundle.externalBin`;`pnpm prepare:tauri-assets` 会生成
   `src-tauri/binaries/echoless-<target-triple>{.exe}`。
2. LocalVQE assets 探针已扩展为 model + native readiness;模型和 native runtime 统一下载到
   `<brand data root>/localvqe/`。
3. **`nvafx download-install`** CLI 子命令已实现(仓库已公开,见 RTX 专题文档 §4)。
4. NVAFX runtime **不进 bundle**(运行时按需下载 ~1GB)。

## 1. 技术栈与位置

- `app/` = Tauri v2 + React 18 + TS + Vite + pnpm。
- `app/src-tauri/` = Rust app 层。`Cargo.toml` 用空 `[workspace]` 与上层 cargo workspace 解耦。
- 插件:`tauri-plugin-decorum`(macOS 红绿灯内嵌到自绘 40px 标题栏)、`tauri-plugin-dialog`(文件/目录选择)。
- 窗口(程序化创建,见 `lib.rs` setup):初始 1000×600,**高度锁定 600**(min/max 同 600),宽度 900–1480 可调。
  macOS = Overlay 标题栏 + 隐藏标题 + 红绿灯 inset(16,13);Windows/Linux = 无装饰 + 自绘 caption 按钮。
- 设计语言:单色 brutalist 工业规格单(`app/src/styles.css`)。

## 2. 架构:GUI ↔ sidecar CLI(只走 JSON/JSONL)

GUI 不含任何 AEC 逻辑,全部 shell `echoless` CLI(`app/src-tauri/src/lib.rs`):

- 一次性 JSON 命令 → 解析 stdout JSON。
- `run --status-json` → stdout 是 JSONL,逐行解析后经 Tauri event 推给前端。
- 人类日志走 stderr → `echoless://log` event。

CLI bin 解析(`echoless_bin()`):
1. 环境变量 `ECHOLESS_BIN`;
2. dev 回退 `../../target/release/echoless`。
**打包后第 2 条不存在 → 必须由 sidecar 提供(§6.1)。**

## 3. IPC 面(全部 Tauri command → CLI 子命令 / 返回契约)

| Tauri command | 参数 | shell 的 CLI | 返回 |
|---|---|---|---|
| `get_platform` | — | (cfg) | `"windows"\|"macos"\|"linux"` |
| `list_devices` | — | `devices --json` | DeviceList |
| `list_processors` | — | `processors --json` | ProcessorManifest |
| `doctor_audio` | — | `doctor audio --json` | DoctorAudio |
| `request_system_audio` | — | `doctor audio --request-system-audio --json` | DoctorAudio |
| `nvafx_doctor` | `runtime_dir?` | `nvafx doctor [--runtime-dir D] --json` | `{ok, report}` |
| `nvafx_install` | `common_zip, model_zip, runtime_dir?` | `nvafx install ...` → `nvafx doctor --json` | `{ok, report}` |
| `nvafx_download_install` | `runtime_dir?` | `nvafx download-install --json`(前端已接,**待后端子命令**) | `{ok, report}` |
| `validate_config` | `toml_text` | `config validate --config <temp> --json` | `{ok, errors[]}` |
| `start_run` | `toml_text, stats_interval_ms` | spawn `run --config <temp> --status-json --stats-interval-ms N` | 流式 event |
| `start_diagnostics` | `record_dir, max_seconds?` | write JSONL to run stdin | event |
| `stop_diagnostics` | — | write JSONL to run stdin | event |
| `set_output_level` | `level: 0..100` | write `{"cmd":"set_output_level","level":N}` to run stdin | `output_level_changed` event |
| `set_bypass` | `enabled: boolean` | write `{"cmd":"set_bypass","enabled":B}` to run stdin | `bypass_changed` event |
| `stop_run` | — | kill child | — |
| `open_url` | `url` | OS 打开浏览器 | — |
| `open_path` | `path` | OS 打开文件管理器(不存在则建目录) | — |
| `default_diag_dir` | — | (temp dir,无 CLI) | string |

Events:
- `echoless://status` — `started` 事件(backend/sr/frame/session_dir)+ 此后每条 `status`(dBFS、波形、队列、drop、latency、diverged、diagnostics 录制态…)。启用 diagnostics 时还会收到 `diagnostics_started` / `diagnostics_stopping` / `diagnostics_done` / `control_error`;实时音量控制会收到 `output_level_changed`,OFF/穿透控制会收到 `bypass_changed`。字段见 `FRONTEND_AGENT_HANDOFF.md`。
- `echoless://exit` — sidecar 退出。
- `echoless://log` — stderr 行。

TS 镜像:`app/src/types.ts`;调用层:`app/src/api.ts`;配置 TOML 生成:`api.ts` 的 `buildConfigToml()`(顶层 mic/reference/output/sample_rate/frame_ms/reference_channels/near_delay_ms/output_level + 可选 `[diagnostics]` + `[[chain]] kind + params`)。

## 4. 已完成的页面/功能

- **Overview**:电源开关、状态框(诚实:无参考时显示 NO REFERENCE 而非绿色)、INPUT/MODEL/OUTPUT/NOISE 行、三路示波。
- **Engine**:AEC3 / LocalVQE / NVAFX 规格牌(ECHO+VOICE 双计量条;NVAFX ECHO 比 AEC3 低一格、VOICE 满格),就绪门槛,`SET UP RTX »` 入口。
- **Advanced**:manifest 驱动的全参数(按 `processors --json` 渲染),`requires` 门控隐藏,EN/中文语言切换。
- **Diagnostics**:Record 极简滑块、最长秒数、可点输出目录(文件夹选择器)、SESSION、Health 计数。
- **RTX Setup 向导**(接管式):doctor 8 态机 → 就绪阶梯 → 自适应动作卡(硬阻断/外部修复/本地 zip 安装/公共下载/ready)。
- **i18n**:全量 EN/中文(功能词翻译,技术术语保留英文)。
- **开发态(按 `~`)**:解开 NVAFX 平台/doctor 门槛 + 注入模拟 Windows RTX doctor,mac 上可走完整 RTX 流程(状态切换条 + 模拟安装→ready)。仅 dev,不影响生产。

## 5. 功能状态表

| 能力 | 状态 |
|---|---|
| 设备枚举 / 处理器枚举 / 配置校验 / run 流式 status | ✅ 接 CLI 已通 |
| AEC3 实时(默认主路径) | ✅ |
| macOS System Audio reference(Process Tap) | ✅ 后端最小集成;已传 `--exclude-pid` 排除 Echoless CLI 输出;app plist/helper env 已补;helper 仍需随包签名 |
| macOS near delay calibration | ✅ 后端原生 `probe-delay --json`;推荐值写入顶层 `near_delay_ms`;不需要 Python/独立脚本 |
| macOS System Audio permission request | ✅ `doctor audio --request-system-audio --json` |
| Diagnostics recording done event | ✅ 后端已发 `diagnostics_done`;前端可移除 timer 猜测 |
| Diagnostics start/stop IPC | ✅ `run --status-json` stdin JSONL |
| Runtime output level IPC | ✅ `{"cmd":"set_output_level","level":N}`;不重启 run,下一帧生效 |
| 虚拟声卡检测提示(doctor audio) | ✅ |
| 非 48k/16k 设备 I/O 重采样 | ✅ 第一版固定比率线性 SRC;`started.io_resampling` 可观测 |
| LocalVQE:选择 + 下载 `.gguf` + native readiness | ✅(bundled model/native 取决于构建环境产物) |
| NVAFX:doctor / 本地 zip 安装 / recheck / ready 选用 | ✅ 接 CLI |
| NVAFX:公共 release 下载安装 | ✅ 前端已接 shell;后端 `nvafx download-install --json` 已实现 |
| 打包 sidecar / bundled assets | ⛔ 待 Codex(§6) |

## 6. 打包集成待办(Codex)

### 6.1 echoless CLI 作为 sidecar
- 已配置 `bundle.externalBin = ["binaries/echoless"]`;构建前由
  `pnpm prepare:tauri-assets` 复制对应平台的 `echoless` 到 target-triple 文件名。
- macOS 还需要随包 `echoless-process-tap-poc` helper,或设置
  `ECHOLESS_PROCESS_TAP_HELPER` 指向固定 helper。`reference="system"` 依赖它。
  当前 app 层已在能定位 helper 时给 CLI 子进程注入该 env;`prepare:tauri-assets` 会把 helper 复制进
  `resources/helpers/`。
- `app/src-tauri/Info.plist` 已包含 `NSMicrophoneUsageDescription` 与
  `NSAudioCaptureUsageDescription`;签名后的 `.app` 才是系统音频录制权限的完整验证对象。
- dev 环境必须继续优先使用显式 `ECHOLESS_BIN` 或固定 `../../target/release/echoless`。不要在 app 启动期扫描资源目录后 `set_var("ECHOLESS_BIN", ...)`;
  这会把 Tauri app 自己误识别成 CLI,导致 `devices --json` 等调用递归拉起窗口。
- 生产打包走 Tauri 官方 sidecar 固定文件名 / target-triple 规则。
- 校验:打包产物里 `devices --json` / `run` 能跑(当前 dev 用 `ECHOLESS_BIN` 或 `target/release/echoless`)。

### 6.2 LocalVQE HF assets
- bundle 不再包含 LocalVQE 模型或 native runtime;首次使用从 HF 下载到品牌数据目录。
- `localvqe_assets()` 已返回 downloaded models、`native_ready`、`library_path`、`native_dir`、
  `native_files`、`cli_path`、`process_tap_helper_path`。
- 前端会在模型存在且 `native_ready=true` 时把 LocalVQE 显示为 READY。

### 6.3 NVAFX runtime
- **不进 bundle**。运行时由向导下载(见 RTX 专题文档 §4 的 `nvafx download-install`)。
- runtime 默认根目录 `%LOCALAPPDATA%\Echoless\nvafx\2.1.0`,doctor/install 已就绪。

### 6.4 其它
- icons 已在 `app/src-tauri/icons/`,CSP 已设,`identifier=app.echoless.desktop`。
- capabilities(`default.json`)含 window 控制 + `dialog:allow-open`;若新增能力记得补权限。
- `tauri.conf.json` 目前 `bundle.targets="all"`,并已配置 `externalBin` / `resources`。

## 7. 构建 / 运行

```bash
cd app
pnpm install
pnpm build                 # tsc + vite,产出 dist/
pnpm prepare:tauri-assets  # 生成 sidecar/resources;tauri dev/build 也会自动跑
pnpm tauri dev
pnpm tauri build
```

自测:前端 `tsc`/`vite` + app 层 `cargo check` 均 0 error;各页面/向导状态用 Playwright(注入 `__TAURI_INTERNALS__` mock)逐态验证;mac 真机验过 AEC3 实时链路。

## 8. 不可回退的约束

- GUI 只消费 JSON/JSONL;不内置 AEC 逻辑。
- 默认 backend `aec3`;48k/10ms/mono 默认;NS/AGC 默认关。
- 不暴露 `.trtpkg`,RTX 模型按 GPU 架构自动选;命名 `NVIDIA AFX / RTX AEC SDK`(非 Broadcast)。
- 就绪判定唯一真源:`nvafx doctor --json` 的 `ok`/`checks`;配置应用前先 `config validate`。
- 窗口高度锁 600、宽度区间内可调;长文本截断不外溢(桌面 app 无滚动条)。

## 9. Windows tray preference contract

Rust/Tauri side exposes `set_tray_prefs` for the frontend preference UI:

| Tauri command | Frontend invoke payload | Effect |
|---|---|---|
| `set_tray_prefs` | `{ minimizeToTray: boolean, closeToTray: boolean }` | Syncs Windows-only tray behavior preferences into Rust state. |

Frontend persistence key: `echoless.trayPrefs.v1`.

Persisted JSON shape:

```json
{
  "minimizeToTray": false,
  "closeToTray": false
}
```

Startup contract: read `echoless.trayPrefs.v1` in the frontend startup effect and invoke `set_tray_prefs`; invoke it again whenever either preference changes. Rust defaults to `false/false`, so behavior remains the existing direct-close path until the frontend pushes persisted preferences. Non-Windows platforms ignore these preferences and keep the current window behavior.

## 10. OFF passthrough / bypass contract

Backend status: implemented in P8-D1 on `phase-2/off-bypass`.

Runtime control:

```json
{"cmd":"set_bypass","enabled":true}
```

Success event on `echoless://status` JSONL:

```json
{"type":"bypass_changed","bypassed":true}
```

Status JSON now always includes:

```json
{"type":"status","bypassed":false}
```

Startup config accepts top-level TOML:

```toml
bypass = true
```

Default is `false`. When `bypassed=true`, realtime output is the raw mic signal, not AEC/NS output and not `near_delay` delayed; the existing `output_level` gain/soft limiter still applies. The processor chain stays warm by default so switching back to processed output does not require a fresh AEC convergence window. ON/OFF transitions crossfade over 15ms.

Tauri Rust exposes direct command `set_bypass(enabled: bool)` and also keeps generic `send_run_control`. P1/frontend should add its own `api.ts` wrapper and stop using `stop_run` for the user-facing OFF state; reserve `stop_run` for actually terminating the run sidecar.

## 下一会话建议
- 用真实 CI LocalVQE 产物 / Windows 机器跑 `pnpm prepare:tauri-assets --require-localvqe-assets`
  后做 Tauri bundle 端到端联调。
- 联调:Windows RTX 机器全链路(doctor → download-install → run);mac 验 AEC3/LocalVQE 打包产物。
