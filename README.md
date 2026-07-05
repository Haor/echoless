# Echoless — 外放场景的实时回声消除

**用音箱外放打语音,对面听不到你的回声。**

Echoless 把「系统正在播放的声音」作为参考信号,从麦克风采到的声音里实时
消掉音箱回声,再把干净人声送进一个虚拟麦克风 —— Discord / VRChat 等语音
应用把那个虚拟麦克风当作输入即可。

```
系统播放(far-end reference,loopback / Process Tap)
        ↘
麦克风(人声 + 音箱回声) → AEC 引擎 → 干净人声 → 虚拟麦克风 → 语音应用
```

- **平台**:Windows 10/11、macOS 14.4+、Linux(PipeWire/PulseAudio)
- **形态**:Tauri 桌面应用(`app/`)+ 同一后端的 CLI(`echoless`)
- **引擎**:AEC3(默认,WebRTC AEC3 的内化 Rust 移植,见 `aec3/`)、
  LocalVQE(神经后处理,试验)、NVAFX / RTX AEC(Windows + RTX 显卡)

## 安装

1. 从 GitHub Actions 构建产物取对应平台的包:macOS 是 `.dmg`,Windows 是
   NSIS 安装器,Linux 是 `.deb` / AppImage(见下方「构建」;暂无公开 release 页)。
2. **装一个虚拟声卡**(Echoless 不创建系统级虚拟麦克风,需要外部设备桥接):
   - Windows:[VB-CABLE](https://vb-audio.com/Cable/)。装完**重启**。
   - macOS:[BlackHole 2ch](https://github.com/ExistentialAudio/BlackHole)
     或 VB-CABLE MAC。
   - Linux:用 PipeWire/PulseAudio null-sink,无需驱动:
     ```bash
     pactl load-module module-null-sink sink_name=echoless_out sink_properties=device.description=Echoless-Output
     ```
     在 GNOME/KDE 声音设置与 Discord/VRChat 等通话 app 里,麦克风选择
     `Monitor of Echoless-Output`。
   - 应用内「MIC SETUP」向导会检测安装状态并逐步引导;Linux 流程简化为
     未创建 null-sink → 已检测到。
3. macOS 首次启动会请求**麦克风**与**系统音频录制**两个权限。

## 快速上手

1. **01 INPUT**:选你的物理麦克风。
2. **02 MODEL**:默认 AEC3 即可。LocalVQE 首次使用需在 Engine 页下载模型
   (~3-18 MB,来自 [HuggingFace](https://huggingface.co/LocalAI-io/LocalVQE));
   NVAFX 需要 Windows + RTX 显卡并按向导装 runtime。
3. **03 OUTPUT**:选虚拟声卡的写入端(Windows `CABLE Input`,mac
   `BlackHole 2ch`,Linux `Echoless-Output`)。
4. 开 **POWER**,然后在 Discord/VRChat 里把麦克风选成虚拟声卡的输入端
   (Windows `CABLE Output`,mac `BlackHole 2ch`,Linux
   `Monitor of Echoless-Output`)。
5. 效果不好时:Advanced 页跑一次 **RUN PROBE**(约 15 秒蜂鸣,自动校准
   near/far 对齐延迟;Linux 暂不支持 probe-delay 蜂鸣播放,该入口会隐藏);
   footer 的 VOL 滚轮调输出音量、点按静音。

电源 OFF = 直通模式:AEC 旁路但麦克风链路不断,语音应用不会失去输入。

## 故障排查

- **对面还是听到回声**:先跑 RUN PROBE;确认 02 的 REFERENCE 是系统音频
  (mac 用 Process Tap,需要系统音频录制权限)而不是 none。
- **状态条显示 NO REFERENCE**:系统当前没有在放声音属正常;持续如此则检查
  参考源选择与权限。
- **LocalVQE 报错 / 不可用**:Engine 页确认模型已下载;`v1.4` 需要新版
  runtime(包内自带,老构建升级后需重装)。
- **虚拟声卡检测不到**:Windows 装完 VB-CABLE 必须重启;mac 可按向导提示
  重启 CoreAudio(`sudo killall coreaudiod`);Linux 确认上面的 `pactl` null-sink
  已创建,并刷新设备。
- Windows 测试者完整流程见
  [`docs/windows-testing/WINDOWS_FRIEND_INSTALL_TEST_GUIDE.md`](docs/windows-testing/WINDOWS_FRIEND_INSTALL_TEST_GUIDE.md)。

## 构建(开发者)

```bash
# 后端 workspace(CLI + 引擎)
cargo build --release
cargo test --workspace
(cd aec3 && cargo test)          # 内化 AEC3 引擎子 workspace

# 桌面应用(需要 Node 22 + pnpm)
cd app
pnpm install
pnpm prepare:tauri-assets        # 构建 CLI sidecar + 打包 LocalVQE native runtime
pnpm tauri dev                   # 开发
pnpm tauri build                 # 打包(发布加 --require-localvqe-assets 缺库即 fail)
```

CLI 可独立使用:`echoless devices` / `run --config` / `probe-delay` /
`offline` / `doctor audio` / `nvafx doctor`,均有 `--json` 输出供集成。
示例配置见 `configs/example.toml`。

推送 `main` 触发 `.github/workflows/build.yml`:Windows/macOS 测试 + clippy +
LocalVQE 上游 regression + FFI smoke + 打包产物(dmg / NSIS)与 bundle smoke;
Linux job 构建 Rust workspace、准备 LocalVQE `.so` native runtime,并打包
`.deb` / AppImage。

## 仓库结构

| 路径 | 内容 |
|---|---|
| `aec3/` | 内化的 AEC3 引擎(独立子 workspace,含延迟魔改,见其 README) |
| `crates/` | `echoless-cli` / `-core` / `-processors` / `-audio-io` / `-paths` |
| `app/` | Tauri 桌面应用(React 前端 + Rust 壳) |
| `docs/` | 架构方案、前端交接、Windows 测试指南、审计记录 |
| `tools/macos-process-tap-poc/` | mac 系统音频采集 helper(Swift) |

分发约定:LocalVQE **native runtime 随包**,**模型走 HF 按需下载**
(`~/Library/Application Support/Echoless` / `%LOCALAPPDATA%\Echoless`);
NVAFX runtime 按需下载(~1 GB,不随包)。
