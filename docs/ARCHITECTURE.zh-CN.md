# Echoless 架构

[English](ARCHITECTURE.md) | 简体中文

```
                    ┌────────────────────────── echoless run (CLI process) ─────────────────────────┐
mic (cpal) ─────────►  near ring ─► near_delay ─┐                                                    │
                    │                           ├─► 10 ms frame loop ─► processor chain ─► output   │
system audio ───────►  far ring ────────────────┘        │                (AEC engine)      (cpal)  │
 (per-OS capture)   │                                    ├─► status JSONL (stdout)                  │
                    │   stdin JSONL control ─────────────┘                                          │
                    └───────────────────────────────────────────────────────────────────────────────┘
                                            ▲                                   ▲
                       Tauri app spawns & supervises              virtual device (VB-CABLE /
                       (start/stop, hot controls, events)          BlackHole / PipeWire null sink)
```

桌面应用自身从不直接接触音频:它把 CLI 作为 sidecar 拉起,写入一份 TOML
配置,读取 JSONL 状态流,并通过 stdin 发送热控命令。GUI 能做的一切,都可以用
CLI 手动完成(见 [CLI.zh-CN.md](CLI.zh-CN.md))。

## 仓库结构

| Path | What it is |
|---|---|
| `crates/echoless-cli` | `echoless` 二进制:实时管线、设备 I/O(cpal)、延迟探测、doctor、NVAFX 安装器 |
| `crates/echoless-core` | 配置模型(TOML)、帧/链原语、平台默认值 |
| `crates/echoless-processors` | 统一 trait 之下的引擎实现:`aec3`、`localvqe`、`nvidia_afx_aec`、`speex`、`passthrough` |
| `crates/echoless-audio-io` | 离线与诊断共用的 WAV / 采样格式辅助工具 |
| `crates/echoless-paths` | 品牌数据目录解析(模型、下载) |
| `aec3/` | **Rust 实现的 WebRTC AEC3** —— 独立的 cargo workspace,BSD-3-Clause,详见下文 |
| `app/` | Tauri 桌面应用:React 前端(`app/src`)、Rust 外壳(`app/src-tauri`) |
| `tools/macos-process-tap-poc/` | 用于 macOS 系统音频采集的 Swift 辅助程序 |
| `configs/example.toml` | 带注释的管线配置示例 |

## 采集:各操作系统的参考源

far-end 参考必须*恰好等于扬声器正在播放的内容*。

- **Windows** —— 在 render endpoint 上做 WASAPI loopback。无需驱动,无需辅助程序。
- **macOS(14.4+)** —— 一个 Swift 辅助程序(`echoless-process-tap`)在所有进程
  (排除 Echoless 自身)之上创建 Core Audio **Process Tap**,并通过 stdout 向 CLI
  串流交错排列的 f32 PCM。该流以 16 字节的 `ELTP` 头(magic、version、采样率、
  声道数)开头;若 tap 的采样率与管线采样率不一致,CLI 会做线性重采样。该辅助
  程序还提供一个无弹窗的 TCC 权限预检,供 `doctor` 使用。
- **Linux(PipeWire/Pulse)** —— 取正在播放的 sink 的 monitor 源
  (`<sink>.monitor`)。无需驱动、无需辅助程序;通过 cpal 枚举。

near-end(mic)与 output 在所有平台上都是普通的 cpal 流;设备采样率会被透明地
适配到管线采样率。

## 管线

固定采样率(默认 48 kHz)、固定帧长(默认 10 ms)的循环:

1. 从 near ring 拉取一帧(经过 `near_delay_ms` 对齐之后 —— macOS 默认 25 ms
   以补偿 tap 的抢先量,其余平台为 0)。
2. 从 far ring 拉取相匹配的一帧。
3. 把两者送入 processor chain(通常是单个 AEC 引擎)。
4. 把处理后的帧推送到 output 设备。
5. 每约 100 ms,输出一行状态 JSON(电平、延迟、引擎指标)。

引擎在一份清单(`echoless processors --json`)中声明自身:kind、平台、带类型/
默认值的参数。GUI 依据该清单渲染其控件,因此新增一个引擎参数只是后端改动。

**热控 vs 重启。** 输出电平、near delay、AEC3 NS/AGC、LocalVQE 噪声门、bypass
以及诊断录制均可通过 stdin 在线生效。设备、引擎、采样率或模型的变更会重启
sidecar(GUI 会自动完成)。

**Bypass 保温。** "关机"会发送 `set_bypass true`:帧会跳过引擎(15 ms 交叉淡入
淡出),但采集/输出继续运行,引擎也保留其自适应状态,因此重新开机是即时且无
爆音的。

## AEC3(`aec3/`)

一份 WebRTC AEC3 的 Rust 移植(实现参考 WebRTC 音频处理模块,基于
[sonora](https://github.com/dignifiedquire/sonora) 的 Rust 移植,并在 `aacadf0`
定型)。作为独立的 cargo workspace 保留,以便其 700+ 个源自上游的测试能不加改动
地针对该移植运行。Echoless 特有的改动均由配置开关守护:

- **Delay hold** —— 一旦延迟估计器达到置信度,估计值即被固定,并抑制在参考静音
  期间的重新搜索。上游 AEC3 假设延迟会漂移(声学路径),而 loopback 参考具有稳定
  的延迟;实测表明在参考静音期间重新搜索会严重劣化长会话的回声衰减(关闭
  `delay_hold` 时长跑 9.6→5.6 dB;见
  `crates/echoless-processors/tests/echo_cancellation.rs`)。
- **Render activity gate**,以及一个显式的 `aec3_delay_blocks` runtime 指标。
- 由外部施加的 near-delay 偏置取代了负延迟处理。

## 引擎分发

| Engine | Runtime | Models |
|---|---|---|
| AEC3 | compiled in | — |
| LocalVQE | 原生库**随应用一同打包**(`liblocalvqe`) | 从 [Hugging Face](https://huggingface.co/LocalAI-io/LocalVQE) 下载 GGUF,逐文件锁定 SHA-256 |
| NVAFX | 从本仓库的 GitHub Releases 下载(通用 runtime + 按 GPU 架构划分的模型 zip,附 SHA-256 清单) | 同上 |

CI 锁定 LocalVQE 的源码修订版本(`LOCALVQE_REF`)并按操作系统构建原生库;
`app/scripts/prepare-tauri-assets.mjs` 将其暂存进 bundle。

## GUI ↔ CLI 契约

- **一次性命令**(`devices`、`doctor audio`、`processors`、`config validate`、
  `probe-delay`、`nvafx …`)以 `--json` 拉起,并从 stdout 解析。
- **`run`** 是长期存活的:stdout 上是 JSONL 状态(首行 `started`,并公布
  `supported_controls`),stderr 上是人类可读日志,stdin 上是控制命令(见
  [CLI.zh-CN.md](CLI.zh-CN.md#runtime-controls))。
- **`probe-delay`** 额外在 stderr 上输出进度 JSONL(`beep_train_start`,附蜂鸣
  节奏),以便 GUI 将其进度指示器与真实播放同步。
