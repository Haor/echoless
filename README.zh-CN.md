# Echoless

[English](README.md) | 简体中文

**面向外放场景的实时声学回声消除(AEC)。**
Echoless 把「音箱正在播放的声音」从「麦克风采到的声音」里消掉,再经虚拟麦克风把
干净人声送给 Discord / 任意语音应用——这样对面永远不会听到自己的声音
(或你的游戏音)被回授回去。

```
远端参考 (far-end)   系统音频 loopback(音箱正在播放的系统声音)
近端采集 (near-end)  麦克风(你的人声 + 音箱回声 + 房间反射)
输出     (output)    去回声后的人声 → 虚拟麦克风 → 语音应用
```

无需特殊硬件:一只 USB 或内置麦克风、普通音箱、一个虚拟音频设备即可。

## 界面预览

<p align="center">
  <img width="800" alt="Echoless 主界面演示" src="https://github.com/user-attachments/assets/c4d846f9-9a7b-4b2d-91ab-945ab9e0ed26" />
</p>

## 特性

- **三套可热切换的 AEC 引擎** —— 运行中随时切换,按口味与硬件挑选
(见下方[引擎](#引擎))
- **系统音频参考,无需额外接线** —— WASAPI loopback(Windows)、
Core Audio Process Tap(macOS 14.4+)、PipeWire monitor(Linux)
- **延迟侦测** —— 播放一段短蜂鸣序列,实测你的「麦克风↔参考」真实延迟并自动应用
(互相关,~毫秒级精度)
- **关机 = bypass,不是静音** —— 麦克风通路永不中断;关掉 AEC 时人声原样穿透
- **诊断录制** —— 把 麦克风 / 参考 / 输出 三轨落成 WAV,便于排查
- **桌面应用 + 独立 CLI** —— Tauri 图形界面驱动的正是你可自行编排的 `echoless`
CLI([CLI 指南](docs/CLI.zh-CN.md))

## 引擎

### AEC3(默认)

来自 [WebRTC](https://webrtc.googlesource.com/src/) 音频处理模块的回声消除器
——与 Chrome、Google Meet 同源的算法族。自适应线性滤波 + 延迟估计 + 非线性残余
抑制,并可选噪声抑制 / AGC。CPU 占用低,48 kHz 原生。

Echoless 附带一份 AEC3 的 Rust 移植(在 [`aec3/`](aec3/),BSD-3-Clause),
针对外放场景做了小改:延迟一旦达到置信度就**保持**,而不是在静音期重新搜索
——这在 loopback 式通路的长会话中可测地提升了稳定性。

### LocalVQE(神经网络,实验性)

[LocalVQE](https://github.com/localai-org/LocalVQE) 由[Local AI](https://huggingface.co/LocalAI-io) 开发,是一族紧凑神经模型,
用于16 kHz 语音的回声消除、噪声抑制与去混响,可在普通 CPU 上实时运行。它是微软
**DeepVQE** 的流式、CPU 调优衍生版,参数量约为其十分之一。runtime 代码为
Apache-2.0,官方模型单独发布在
[Hugging Face](https://huggingface.co/LocalAI-io/LocalVQE)。该模型运行于 16 kHz,
Echoless 在其 48 kHz 管线两侧自动完成 48↔16 kHz 重采样,将模型透明地接入信号链。


| 模型          | 作用                       | 参数量   |
| ----------- | ------------------------ | ----- |
| v1.3 *(默认)* | AEC + 噪声抑制 + 去混响         | 4.8 M |
| v1.2        | AEC + NS + 去混响,CPU 开销约 ¼ | 1.3 M |
| v1.4-AEC    | 仅消回声 —— 保留人声、噪声与房间感      | 203 K |


推理 runtime 随应用一起分发;模型权重按需从 Hugging Face 下载。 

概览页的 NOISE 开关映射到模型选择:**开 = v1.3,关 = v1.4**(纯 AEC)。

### NVAFX / RTX AEC(Windows + RTX GPU)

来自 [NVIDIA Maxine](https://developer.nvidia.com/maxine) Audio Effects SDK 的
回声消除,在 RTX Tensor Core 上加速。在我们的测试中,它在压制大音量回声的同时对
人声的保留最好,但会残留一些回声——建议在其后再串一级降噪(如 NVIDIA Broadcast)。
需要 Windows 与 Turing 及更新的 RTX GPU;runtime(~1 GB)与各架构模型在首次设置时
下载。*AEC powered by NVIDIA Maxine.*

## 平台


| 系统              | 参考采集                   | 虚拟麦克风                                                          | 状态         |
| --------------- | ---------------------- | -------------------------------------------------------------- | ---------- |
| Windows 10 / 11 | WASAPI loopback        | [VB-CABLE](https://vb-audio.com/Cable/)                        | 已支持        |
| macOS 14.4+     | Core Audio Process Tap | [BlackHole 2ch](https://github.com/ExistentialAudio/BlackHole) | 已支持        |
| Linux           | monitor source         | `pactl` null sink——无需驱动                                        | 实验性,尚未真机验证 |


## 技术栈

- **核心 / CLI** —— Rust(cargo workspace:`echoless-core` / `echoless-audio-io` /
  `echoless-processors` / `echoless-cli`)。
- **音频 I/O** —— [cpal](https://github.com/RustAudio/cpal) 设备采集与播放、
  `ringbuf` 无锁环形缓冲、[rubato](https://github.com/HEnquist/rubato) 重采样;
  参考采集走平台原生 API(WASAPI loopback / Core Audio Process Tap /
  PipeWire monitor)。
- **AEC 引擎** —— AEC3(独立 Rust workspace,`aec3/`)、LocalVQE(C + ggml,
  经 `libloading` FFI 动态加载)、NVAFX(NVIDIA Maxine Audio Effects SDK)。
- **桌面应用** —— [Tauri v2](https://tauri.app)(Rust 后端)+ React 18 +
  TypeScript,Vite 构建 / Vitest 测试。
- **macOS 系统音频** —— Swift 编写的 Process Tap helper
  (`tools/macos-process-tap-poc/`)。

## 安装

从 [Releases](https://github.com/Haor/echoless/releases) 下载对应平台的安装器
(`.dmg` / `.exe` / `.deb` / `.AppImage`),或[从源码构建](#从源码构建)。

> **macOS**:首次打开若提示「已损坏 / 无法打开」,在终端执行以下命令后重试:
>
> ```bash
> sudo xattr -rd com.apple.quarantine /Applications/Echoless.app
> ```

你还需要一个虚拟音频设备(见上表)。应用的 **MIC SETUP** 向导会检测它是否存在,
并引导你完成安装。

## 快速上手

1. 安装虚拟音频设备([VB-CABLE](https://vb-audio.com/Cable/) /
  [BlackHole](https://github.com/ExistentialAudio/BlackHole);Linux 上只需一条
   `pactl load-module module-null-sink …` 命令——向导会给出)。
2. 在 Echoless 里:**INPUT** = 你的麦克风,**OUTPUT** = 虚拟设备。
  参考默认取系统音频。
3. 打开 **POWER**。macOS 上按提示授予系统音频录制权限。
4. 在你的语音应用里,把虚拟设备选作麦克风
  (`CABLE Output` / `BlackHole 2ch` / `Monitor of Echoless-Output`)。
5. 若仍有回声,在 Advanced 页点 **RUN PROBE**(约 15 秒蜂鸣)实测并应用你的
  确切设备延迟。

**关闭 POWER 是 bypass**:跳过 AEC 处理,麦克风信号原样穿透,语音应用不会失去输入。

底部 **VOL** 是输出增益:悬停用滚轮调节,以 dB 计——50 为单位增益(0 dB,原样
送出),范围自 0(静音)至约 +9.5 dB(≈3×);单击可静音 / 恢复。用它补偿人声电平,
使对端听感适中。

## CLI

Echoless 提供独立的 `echoless` CLI(图形界面即构建于其上)——`devices`、`run`、
`probe-delay`、`offline`(WAV 进 / WAV 出)、`doctor`,全部支持 `--json` 便于脚本化:

```bash
echoless devices --json
echoless run --mic default --reference system --output "CABLE Input"
echoless offline --mic mic.wav --reference ref.wav --out clean.wav --chain aec3
```

完整命令参考、runtime 控制协议与配置格式见 **[docs/CLI.zh-CN.md](docs/CLI.zh-CN.md)**;
架构说明见 **[docs/ARCHITECTURE.zh-CN.md](docs/ARCHITECTURE.zh-CN.md)**。

## 从源码构建

前置:Rust(stable)、带 Corepack 的 Node 22,macOS 上还需 Xcode CLT + Swift
(用于 Process Tap helper)。应用 workspace 经 `app/package.json` 锁定 pnpm 版本;
先 `corepack enable` 让 Node 使用该 pnpm。`app/pnpm-workspace.yaml` 设置了
`minimumReleaseAge: 10080`(7 天):npm 依赖须发布满 7 天才允许安装,以降低新版本
投毒类供应链攻击的暴露窗口。

```bash
# CLI
cargo build --release                    # target/release/echoless

# macOS 系统音频 helper
tools/macos-process-tap-poc/build.sh

# 桌面应用(开发)
cd app && corepack enable && pnpm install && pnpm tauri dev

# 桌面应用(打包)
cd app && pnpm tauri build
```

`cargo test --workspace` 跑测试套件;AEC3 引擎是独立 workspace
(`cd aec3 && cargo test`)。

## 致谢与许可

- **Echoless** 采用 MIT 许可([LICENSE](LICENSE))。
- **AEC3**(`aec3/`)的实现参考 [WebRTC 项目](https://webrtc.org)的音频处理模块,
基于 [sonora](https://github.com/dignifiedquire/sonora) 的 Rust 移植——
BSD-3-Clause([aec3/LICENSE](aec3/LICENSE))。
- **LocalVQE** 的 runtime 代码与模型为 Apache-2.0,© Local AI,基于 DeepVQE 与
GTCRN 的研究工作(学术引用信息见其
[仓库](https://github.com/localai-org/LocalVQE)的 `CITATION.cff`)。不用于紧急
或安全关键场景(见[模型卡](https://huggingface.co/LocalAI-io/LocalVQE))。
- **NVAFX** 使用 NVIDIA Maxine Audio Effects SDK,遵循
[NVIDIA SDK License](https://developer.nvidia.com/downloads/maxine-sdk-license);
重分发的 runtime / 模型包仅供 Echoless 安装使用。NVIDIA 与 Maxine 是 NVIDIA
Corporation 的商标。
- 虚拟音频致谢:[VB-CABLE](https://vb-audio.com/Cable/) 与
[BlackHole](https://github.com/ExistentialAudio/BlackHole)。

完整的第三方许可全文(WebRTC BSD-3、LocalVQE Apache-2.0 + NOTICE、NVIDIA Maxine
SDK)汇总在 [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md)。
