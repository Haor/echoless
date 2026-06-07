# Windows 外放音箱场景下的实时 AEC 工具 — 完整技术调研

> 目标平台:Windows 10 / Windows 11
> 目标场景:Discord / VRChat / 语音连麦时使用外放音箱,同时避免系统播放声音经麦克风回传给对方。
>
> 本文档由五份调研整理合并而成(深度架构调研、技术报告、Neural AEC 模型调研、NVIDIA Broadcast 真伪 AEC 分析、Maxine SDK 接入评估),已统一结构、去除重复,保留全部数据、代码、链接与结论。
>
> **2026-06-05 修正:** 已按 `research/reference_repos_exploration_report.md`(reference_repos/ 16 组仓库源码级深扒,经 codex 复核)对本文做 8 处修正,均带 `file:line` 证据。主要变更:① AEC3 内置 `NeuralResidualEchoEstimator` 深度集成接口(§3.3);② `sonora` 升至 4/5 classical AEC3 Rust 主线(§3.2/§4.1);③ Virtual-Audio-Driver 公开版降至 1/5(mic 恒静音,§7.5);④ 虚拟麦克风起点改 simpleaudiosample + Win11 AecApo 路线(§7.3/§7.4);⑤ drift ppm 闭环无 production 实现=创新空间(§6.6);⑥ AEC3 4ms block/band0/external delay 等源码纠正(§4.1);⑦ DATA_DISCONTINUITY 风险 + AECMOS_local 评测(§12/§13);⑧ project-raven/DTLN/TSPNN/NKF/EchoFree 逐项纠正(§3.3/§14)。
>
> **2026-06-07 产品决策更新:** 原生虚拟麦克风驱动不再作为 Echoless 路线图目标。本文关于自研虚拟麦、WaveRT/SysVAD/simpleaudiosample、AudioServerPlugin 的分析仍可作为历史调研与风险背景,但当前产品实现长期依赖 VB-Cable / BlackHole / Virtual Desktop Mic 等外部虚拟设备。当前没有证据表明 `cpal` 是瓶颈,因此不排期原生平台 I/O 重写。边界见 `docs/architecture/audio_io_scope.md`。

---

## 目录

1. 总体判断与最终推荐
2. 需求分析与核心挑战(含证据分级)
3. AEC 引擎全景调查(经典 / Neural / NVIDIA)
4. 核心引擎源码级分析(WebRTC AEC3 / SpeexDSP)
5. Windows 音频采集方案
6. 时钟漂移与延迟对齐
7. 处理后音频送入 Discord / VRChat(虚拟麦克风)
8. 外放场景特殊考量
9. 推荐架构方案
10. 技术路线对比
11. MVP 到成品路线图
12. 验证计划与实验设计
13. 风险清单
14. 参考资源清单

---

## 1. 总体判断与最终推荐

这个项目的本质是一个 **Windows 桌面级 speakerphone AEC 管线**,不是语音降噪工具。核心链路必须同时拿到两路实时音频:

```text
far-end reference = Windows render endpoint loopback,即音箱正在播放的系统声音
near-end capture  = USB 麦克风采到的「用户声音 + 音箱空气传播回声 + 房间反射」
output            = 去除 far-end 回声后的用户声音,作为系统麦克风输入给 Discord / VRChat
```

只要有正确的 far-end reference,人声、音乐、游戏音效在 AEC 眼里都只是「由扬声器播放后通过声学路径漏进麦克风的信号」。NVIDIA Broadcast / RTX Voice 这类只看麦克风单路信号的语音增强,会把视频对白、游戏语音、Discord 对方声音当成「人声」,这正是单端 NS / voice isolation 的边界。

### 1.1 工程最优路线(三句话)

> - **当前主路径**:`cpal` 实时 I/O + USB 麦克风 + far-end reference + WebRTC AEC3(sonora) + VB-Cable / VAC / 外部虚拟设备输出给 Discord。
> - **产品输出策略**:不自研虚拟麦克风驱动;Discord / VRChat 选择 VB-Cable / BlackHole / Virtual Desktop Mic 等外部虚拟输入。
> - **Windows 11** 可探索 APO / CAPX AEC 方案,但不应作为 Win10 / Win11 全覆盖产品的第一路径。

### 1.2 AEC 引擎结论

- **成品核心首选:WebRTC AEC3 / WebRTC AudioProcessing Module(证据等级 A)。** 开源、可商用、工程成熟度最高;源码含 delay buffer、echo path delay estimator、adaptive FIR filter、matched filter、residual echo estimator、suppression gain、reverb model、near-end detector、clockdrift detector 等完整组件。短板是构建链重(GN/Ninja/Clang)、内部 API 变动风险、外放场景需自己调 delay/filter tail/stereo/drift,以及需自己解决虚拟麦克风输出。
- **最快 MVP:SpeexDSP AEC(证据等级 A)。** C API 极简、vcpkg 可装、Windows 接入最轻;但基于 MDF/AUMDF,无独立 double-talk detector(靠 variable learning rate),外放音箱 + 音乐 + 人声对白 + 强双讲场景上限不如 AEC3。建议作 MVP 与对照基线,成品保留为 fallback。
- **Neural AEC 真正可落地的只有三个:LocalVQE、DTLN-aec、NVIDIA Maxine AEC。** 其余多为 benchmark / 论文复现 / 待跟踪。**最稳的成品不是「纯 neural AEC」,而是 `WebRTC AEC3 负责主消回声 + neural 模型(LocalVQE/DTLN)负责残余回声、非线性、去混响`。⭐ 重要更新(2026-06):AEC3 已内置可注入的 `NeuralResidualEchoEstimator` 深度集成接口,这套「AEC3 主消 + neural 残余」应走「深度集成(写一个子类)」而非「管线串联」,工程量与时序对齐成本大幅下降——详见 §3.3。**
- **classical AEC3 的 Rust 主线候选 `sonora`(自带 C ABI、绕开 GN/depot_tools)在 2026-06 复核中从 3/5 上调到 4/5,与 webrtc-audioprocessing CMake wrapper 并列**——详见 §3.2 / §4.1。
- **NVIDIA Broadcast App 没有 reference-based AEC**(只有单端 Noise Removal / Room Echo Removal / Studio Voice);真正的 AEC 在 **Maxine / AFX SDK** 里(`NVAFX_EFFECT_AEC`),但它只是引擎,不解决音频管线、drift、虚拟麦克风、分发,且有 RTX 硬件门槛,适合作「RTX 用户可选增强 / 效果上限对照」。

### 1.3 最终推荐落点

> **WebRTC AEC3(sonora) + `cpal` 实时 I/O + drift-aware 对齐 + 48 kHz stereo reference + 10 ms pipeline + 外部虚拟设备输出。**
>
> - SpeexDSP 用来抢验证速度并作 fallback;
> - LocalVQE / DTLN-aec 作为 neural residual suppressor 候选;
> - NVIDIA Maxine 作为 RTX 用户可选增强与效果对照;
> - Windows 11 APO / CAPX 作为后续系统级增强路线,不作为唯一主线;
> - 不把 NS、AI voice isolation、VAD、RNNoise、NVIDIA Broadcast 类工具当作核心 AEC。

---

## 2. 需求分析与核心挑战

### 2.1 真正困难的不是「能不能消人声」

只要有正确的 far-end reference,人声/音乐/游戏音效在 AEC 眼里都只是回声。真正困难在工程对齐、时钟、声学路径与系统集成。两份主调研的难点归纳如下(已合并):

| 难点 | 为什么难 | 对架构的影响 |
|---|---|---|
| 真 AEC 引擎需求 | 需要 far-end reference,不是普通 NS/VAD/AI voice isolation | NVIDIA Broadcast/RTX Voice、RNNoise、传统 NS 只能作后处理或对照 |
| Render / capture 延迟未知 | WASAPI loopback 到麦克风采集之间包含音频引擎缓冲、DAC、空气传播、ADC、USB 缓冲 | 必须有延迟估计、可调 offset、统计监控 |
| 两个独立设备时钟漂移 | 主板声卡 / HDMI / USB DAC 与 USB 麦克风通常不是同一采样时钟 | 必须做 drift tracking 和异步重采样,只做固定 delay alignment 不够 |
| 外放 echo path 长 | 桌面音箱 + 房间反射比耳机漏音复杂,路径随位置变化 | AEC filter tail 不能按耳机场景短配置 |
| 双讲 | 你说话时算法不能把你的声音当成回声去学习 | 需要 robust double-talk / update gating |
| 立体声参考 | 两只音箱到麦克风是两条不同声学路径 | 最好把 stereo render reference 喂给 AEC,而非粗暴 mono downmix |
| Windows 音频 I/O | loopback、mic capture、event callback、timestamp、device invalidation 都要稳 | 建议 raw WASAPI 或参考 Chromium/OBS 自研采集层 |
| 输出给 Discord | Discord / VRChat 只认系统录音设备 | 最终需要虚拟麦克风设备或等价系统级注入 |
| 系统设备变化 | 插耳机、切默认设备、采样率变化都会让 loopback 失效或重建 | 必须有设备通知和自动重连 |
| 外放场景复杂 | 回声能量高、非线性失真、房间混响、双讲更难 | WebRTC AEC3 明显优于老式轻量 AEC,但仍需物理布局与校准 |

### 2.2 证据分级口径

本文把证据按可靠性分级:

- **A 级**:源码、官方文档、头文件、构建脚本、官方 sample。
- **B 级**:维护者 issue/PR、官方论坛、工程博客。
- **C 级**:第三方博客、用户经验。
- **D 级**:没有直接证据的工程判断/推断。

### 2.3 WASAPI loopback 基本事实(A 级)

Windows 的 WASAPI loopback 明确是从 render endpoint 抓取正在播放的混音;系统没有硬件 Stereo Mix 也能通过 WASAPI loopback 捕获播放流,`AUDCLNT_STREAMFLAGS_LOOPBACK` 要在 render endpoint 上以 shared mode 初始化。WASAPI 包含 loopback 的主要理由之一就是支持 AEC。

参考:WASAPI loopback recording — <https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording>

---

## 3. AEC 引擎全景调查

### 3.1 评分口径

- 5 = 可直接作为产品核心
- 4 = 适合作为产品核心但接入成本高
- 3 = 可做 MVP 或重要参考
- 2 = 只适合实验 / 局部参考
- 1 = 不建议作为主路径

### 3.2 经典 / 开源 AEC 引擎全景

| 引擎 / 项目 | 算法与源码判断 | Windows 构建与依赖 | 许可 / 成本 | 可用于本项目 |
|---|---|---|---|---|
| **WebRTC AEC3 官方 C++** | 频域分块自适应 FIR、粗/精滤波、延迟估计、残余回声估计、抑制器、舒适噪声、多通道检测。源码完整、生产使用广泛。 | GN / Ninja / depot_tools;依赖 WebRTC common_audio、absl、rtc_base 等。可在 Windows 编译,工程链较重。 | BSD 风格 + PATENTS;开源。 | **4–5 / 5(成品核心)** |
| **SpeexDSP echo canceller** | MDF / AUMDF,频域分块滤波,变量学习率;源码注释明确没有显式 double-talk detector,但有 foreground / background two-path 机制。 | vcpkg 可装;C API 简洁;Windows 接入最轻。 | BSD-like;开源。 | **3 / 5 MVP,2 / 5 成品** |
| **NVIDIA Maxine Audio Effects AEC** | 公开 AEC API,far-end + near-end 双输入;算法闭源(neural)。 | 需 NVIDIA SDK、模型、CUDA / Tensor Core GPU;Windows 10 / 11 支持。 | 商业 / 闭源 SDK;分发受限。 | **2.5–3.5 / 5(可选后端)** |
| **Rust `sonora` / `sonora-aec3`** | WebRTC AudioProcessing **M145** 纯 Rust 移植,含 AEC3、NS、AGC2、HPF;**附带现成 `wap_*` C ABI(`sonora-ffi`,staticlib + cbindgen)**。源码核实(`sonora/README.md:57-71`):classical AEC3/APM 完整;`panic_guard` 防 panic 跨 FFI。**caveat**:Windows x64 CI 只覆盖普通 cargo build/test,2400+ C++ reference validation 跑在 Ubuntu/macOS(非 Windows);`sonora-aec3/src/residual_echo_estimator.rs:179` 明确 `NeuralResidualEchoEstimator is skipped (not ported)`。 | Cargo;Windows x86_64 SSE2 / AVX2;普通 CI 通过。 | BSD-3;开源。 | **4 / 5(classical AEC3 Rust 主线候选)** |
| **Rust `aec3` crate(aec3-rs)** | 纯 Rust WebRTC AEC3 移植,带图式运行时,API 标注 WIP;milestone 偏旧(用旧 main/shadow 命名,sonora 用新 refined/coarse),**无 FFI / 无 C ABI**。`discontinuity` 标志框架值得借鉴。 | Cargo;依赖 rustfft / crossbeam 等。 | MIT OR BSD-3-Clause;开源。 | **2.5 / 5(参考设计,不作主线)** |
| **get-wrecked/webrtc-audioprocessing** | 简化 APM 构建的第三方 CMake wrapper,工程便利层。 | 面向独立构建;需核对第三方维护状态。 | BSD-style。 | **4 / 5(参考或 fork)** |
| **PipeWire / PulseAudio echo-cancel** | 架构优秀,通常调用 WebRTC / Speex 后端;Linux 音频图模型适合参考。 | 非 Windows 原生方案;移植成本高。 | MIT / LGPL 等。 | **2 / 5 直接使用,5 / 5 架构参考** |
| **Project Raven** | 现成项目:Windows / macOS 抓系统音频和麦克风,做回声消除;很接近「参考实现」。⚠️ **README 自称 AEC3,但源码核实实为旧版 WebRTC APM(GStreamer `webrtcdsp` 插件 + 旧 `EchoCancellation` API),非真正 AEC3**。 | Node / Rust / GStreamer / CMake;Windows 预编译 lib 实际缺失(仅 macOS),需自行 build。 | 开源;目标是会议转写,非虚拟麦克风产品。 | **3.5 / 5 参考,2.5 / 5 直接 fork** |
| **Windows 11 AEC APO / CAPX** | 不是单独 AEC 引擎,而是系统级 APO 插入与 reference stream 框架。 | Win11 22000+,更适合驱动 / APO 包;Win10 不覆盖。 | 与驱动签名、HLK、INF 绑定。 | **3 / 5 Win11 专线,1 / 5 通用首选** |
| **Intel IPP Echo Canceller** | 历史有 subband/fullband/NLMS EC primitives;当前可核验的现代完整引擎资料不足。 | 商业 / 闭源生态;Windows 编译不是问题,管线需自行补齐。 | 商业 / oneAPI。 | **1–2 / 5(只做对照)** |
| **Superpowered** | 文档声称有 "Low-latency custom AEC",但公开资料未确认 far-end reference API / stereo / Windows sample。 | 商业 SDK。 | 商业。 | **1–3 / 5(需试用评估)** |
| **Switchboard Audio SDK** | 文档描述 AEC node(可用 Superpowered / Speex extension),公开桌面 Windows 方案与源码不足。 | 商业 SDK,偏移动 / 嵌入式描述。 | 商业。 | **2–3 / 5** |
| **PJSIP / PJMEDIA** | 抽象多个 EC backend(Speex / Simple / WebRTC / WebRTC AEC3);引入完整通信栈过重。 | PJSIP build system,支持 Windows。 | GPL / 商业。 | **2 / 5(只参考 backend 抽象)** |
| **Microsoft Speech MAS model-based AEC** | 支持 loopback/reference 的 model-based AEC,绑定 Azure Speech 路线。 | Speech SDK。 | 绑定 Azure。 | **不适合通用虚拟麦克风核心** |

#### 引擎能力对比(真 AEC / reference / stereo / 实时 / 集成)

| 引擎 / SDK | 真 AEC | render reference | stereo reference | 实时 | Windows 集成 | License | 结论 |
|---|---:|---:|---:|---:|---|---|---|
| WebRTC AEC3 | 是 | 是 | 支持多通道管线,外放收益需验证 | 是 | 中等偏难 | BSD-style | 成品首选 |
| SpeexDSP AEC | 是 | 是 | 有 multi-channel API | 是 | 容易 | BSD-like | MVP 首选 |
| PulseAudio WebRTC EC | 是 | 是 | 有多通道配置 | 是 | Linux audio server | LGPL | 架构参考 |
| PipeWire echo-cancel | 是 | 是 | 配置支持多通道 | 是 | Linux-first | MIT mostly | 架构参考 |
| NVIDIA Maxine AFX AEC | 是 | 是 | 文档主述 near/far,stereo 需 SDK 验证 | 是 | Win10/11 + RTX | NVIDIA SDK | 可选高级后端 |
| Superpowered AEC | 文档声称是 | 需验证 | 未确认 | 是 | 文档列 Windows | 商业 | 需评估 |
| Switchboard AEC node | 是,依赖扩展 | 是 | 未确认 | 是 | SDK 需验证 | 商业 | 需评估 |
| Intel IPP EC | 历史 primitives 是 | 是,需自建管线 | 未确认 | 是 | Windows C/C++ | oneAPI | 当前成品引擎未确认 |
| Microsoft APO AEC | 是 | 系统提供 additional input | 取决于 APO 实现 | 是 | Win11 driver/APO | 驱动分发 | 系统级路线 |
| RNNoise / RTX Voice NS | 否或不完整 | 通常没有 far-end | 不适用 | 是 | 容易/中等 | 多样 | 只能后处理或对照 |

#### 逐项定位说明

- **WebRTC AEC3(4–5/5):** 最值得作为产品核心的开源引擎。短板不是算法,而是:① 构建链重;② 内部 API 变动风险(老的 `webrtc::Config` builder 已变为 `AudioProcessing::Config` / `ApplyConfig`);③ 开放音箱场景需额外调 delay / filter tail / stereo / drift;④ 需自己解决虚拟麦克风输出。源码细节见第 4 章。
- **SpeexDSP(3/5 MVP):** 最快拿到可运行 MVP 的 AEC。缺点是算法年代较老,double-talk / 残余回声 / 非线性处理不如 AEC3。源码细节见第 4 章。
- **Rust `sonora`(4/5,classical AEC3 主线候选)与 `aec3` crate(2.5/5,参考设计):** `sonora` 是 WebRTC APM/AEC3 的纯 Rust **M145** 移植,**自带 `sonora-ffi` 现成 `wap_*` C ABI**(staticlib + cbindgen,`sonora/crates/sonora-ffi/src/functions.rs:22-52`),`panic_guard.rs` 用 `catch_unwind` 三层防 panic 跨 FFI。这条路线能**绕开 depot_tools/GN/Ninja**,直接用 cargo 拿到 classical AEC3,显著降低 §4.1「构建链重」短板——因此从早期 3/5 上调到 **4/5**,作为 `WebRtcAec3Engine` 的 Rust 主线实现,与 webrtc-audioprocessing CMake wrapper(见 §4.1)**并列**。`adaptive_fir_filter.rs` 保留 `H(t+1)=H(t)+G(t)*conj(X(t))` 频域分块自适应 FIR 与 SIMD backend。**两个关键 caveat(降低盲目押注风险):** ① Windows x64 CI **只覆盖普通 cargo build/test**,2400+ C++ reference validation 跑在 Ubuntu/macOS,**Windows 数值一致性/长时稳定性需自测**(见 §13.4);② `sonora-aec3/src/residual_echo_estimator.rs:179` 明确**未 port 2025 `NeuralResidualEchoEstimator`**——若目标包含官方 neural REE 路径(见 §3.3),sonora 仍需补 port 或回退 official C++。`aec3-rs` milestone 偏旧 + 无 FFI,只作参考设计(graph DAG + discontinuity 标志框架),不作主线。
- **PipeWire / PulseAudio echo-cancel(直接 2/5,架构 5/5):** 证明了推荐架构的正确形态——**虚拟输入/输出节点 + reference/capture 双流 + ring buffer + AEC 处理线程**。一个重要证据:PulseAudio WebRTC wrapper 中 `drift` 默认 false、`pa_webrtc_ec_set_drift` 为空实现,说明桌面外放 AEC 必须在应用层显式处理 clock drift。详见第 6 章。
- **Project Raven(参考 3.5/5,fork 2.5/5):** 最接近需求的开源项目,已把「Windows WASAPI system audio + mic + 回声消除」跑通。`src/native/` 有三个子模块:`windows/`(Rust + windows crate,WASAPI loopback `eRender`+`AUDCLNT_STREAMFLAGS_LOOPBACK` 与 mic `eCapture` 采集,rubato 重采样到 16 kHz mono Int16)、`aec/`(**实际运行的 AEC**:Node-API addon 内嵌 GStreamer pipeline + `webrtcdsp` 插件)、`webrtc-aec/`(另一套直连 C API wrapper)。C API 形态干净:`raven_aec_create(int sample_rate)`、`raven_aec_process_render(...)`、`raven_aec_process_capture(...)`、`raven_aec_set_stream_delay(...)`、`raven_aec_get_stats(...)`、`raven_aec_reset(...)`;`RavenAecStats` 含 ERL、ERLE、delay_ms、diverged;render/capture 按 160 samples = 10 ms Int16 mono 帧处理,默认 stream_delay 50 ms(可设,典型 40–150 ms)。
  - ⚠️ **源码核实(2026-06,本地 clone 阅读)与 README/早期描述的出入:**
    1. **不是 AEC3。** `webrtc-aec/README.md` 自称「AEC3 / Based on WebRTC M124」,但 `webrtc-aec/src/aec_api.cpp` 实际用旧版 WebRTC APM 接口(`echo_cancellation()->set_suppression_level(kHighSuppression)`、`AnalyzeReverseStream`/`ProcessStream`、`webrtc::Config`、`webrtc::AudioFrame`)——AEC1/AECM 时代 API,真正 AEC3 已移除这些;Electron 实际走的是 `aec/` 的 GStreamer `webrtcdsp` 路径(底层 webrtc-audio-processing 库亦偏旧)。
    2. **采集是轮询非 event-driven**(`wasapi.rs` 用 `thread::sleep(20ms)` + `GetNextPacketSize` 轮询)。
    3. **无 QPC / device-position 时间戳,无 drift 补偿**:`GetBuffer` timestamp 传 `None`,自用 `SystemTime::now()` 当 PTS,对齐仅靠 system PTS + 引擎内部 delay。
    4. **stereo 被简单 downmix 成 mono**(各声道求平均),丢失左右 echo path。
    5. **16 kHz mono Int16**(ASR 向);无设备热插拔恢复、无 MMCSS、无虚拟麦克风输出;Windows 预编译 lib 缺失(仅 macOS);`webrtc-aec/build/` 误提交了构建产物。
  - **可复用**:C API 接口形态(可作 `IAecEngine` 蓝本)、`wasapi.rs` 的 loopback/mic 初始化与 `convert_to_f32`(16/32bit)骨架、rubato 重采样用法、10 ms frame pipeline、ERLE/delay/divergence 统计接口。
  - **不可直接照搬 / 必须自补**:真正的 AEC3(见第 4 章 + 本地 `reference_repos/webrtc-aec3-src/`)、event-driven 采集、QPC/device-position 精确对齐、clock drift 补偿、stereo reference、虚拟麦克风输出、设备恢复、MMCSS。
  - 本地副本:`reference_repos/project-raven/`;仓库:<https://github.com/Laxcorp-Research/project-raven>
- **Intel IPP(1–2/5):** 公开可核验资料不足,当前 ready-to-use AEC engine 未确认,仅作底层 optimized primitives 对照。
- **Superpowered(1–3/5):** 文档声称有低延迟 AEC,但关键 AEC API 细节(far-end reference、stereo、Windows sample、分发条款)未公开,值得联系试用,不能基于公开资料定为主路线。
- **Switchboard(2–3/5):** 商业 audio graph,文档有 AEC node,但 Windows 虚拟麦克风、外部 render reference、低延迟、授权均不透明。
- **PJSIP / PJMEDIA(2/5):** 抽象多 AEC backend(含 WebRTC AEC3),但引入完整通信栈过重,只参考其 backend abstraction。

### 3.3 Neural AEC 引擎

按「能不能真的拿来做 Windows 外放连麦工具」筛选后,**可用 neural AEC 模型其实很少**。大部分论文只给思路、demo 音频或评测表,没有权重、没有 streaming runtime、没有 C/C++ API。

#### 筛选标准

"真实可用" = 同时满足以下多数条件:

| 条件 | 说明 |
|---|---|
| 有公开权重或可调用 SDK | 不能只是论文 |
| 输入含 mic + far-end/reference | 不能只是单端降噪/去混响 |
| 有推理代码 | Python 离线脚本也算,但 C/C++/ONNX/TFLite/GGML 更好 |
| 能 streaming 或 frame-wise | 不是整段音频离线生成 |
| 延迟有望 <50 ms | Discord/VRChat 实时可用 |
| 能嵌入 Windows | ONNX Runtime / TFLite / GGML / 商业 DLL / 可移植 C++ |
| 许可证可接受 | MIT / Apache / BSD / 商业 SDK 条款明确 |

**关键现实:几乎所有公开 neural AEC 都是 16 kHz mono。** 在 48 kHz stereo speaker + USB mic 场景下需要预处理:

```text
48 kHz stereo render reference
        ├─ stereo → mono 或更复杂的 stereo reference fusion
        ├─ 48 kHz → 16 kHz
        ▼
neural AEC model
        ▲
48 kHz mic → 16 kHz mono
        ▼
16 kHz clean speech → 48 kHz upsample → virtual mic
```

这会牺牲宽带语音质量,也降低 stereo speaker echo path 可分辨性。因此 neural AEC 更适合做 `WebRTC AEC3 / classical AEC → neural residual echo suppressor / neural postfilter`,而非一开始就完全替代 AEC3。

#### 三档分级

| 档位 | 模型 / SDK | 判断 |
|---|---|---|
| **一线可试** | LocalVQE、DTLN-aec、NVIDIA Maxine AEC | 有模型/SDK,能跑、接近实时,值得进原型 |
| **基准 / 研究可用** | Microsoft AEC Challenge DEC baseline、NKF-AEC、Deep Echo Path Modeling | 有代码/权重或 ONNX,但更像 benchmark / research baseline |
| **暂不适合产品核心** | TSPNN、EchoFree、FADI-AEC、LLaSE-G1、各类 diffusion / LLM speech enhancement | 论文强,但缺可部署 runtime、权重、实时性或专用 AEC 接口 |

#### 候选模型总表

| 模型 / SDK | 权重/API | Runtime | 输入 | 采样率 | 实时性 | Win 嵌入 | 可用度 |
|---|---|---|---|---:|---|---|---:|
| **LocalVQE** | GGUF / PyTorch | C++ GGML + C API | mic + ref | 16k mono | 16 ms hop,CPU 实时 | 中 | **4/5** |
| **DTLN-aec** | TFLite | TFLite / C wrapper | mic + lpb | 16k mono | 32ms window / **8 ms hop** | 低-中 | **4/5** |
| **NVIDIA Maxine AEC** | 商业 SDK | NVIDIA DLL | near + far | 16/48k float | 实时 | 中,高硬件/授权约束 | **3.5/5** |
| **Microsoft DEC baseline** | ONNX | ONNX Runtime | mic + lpb | 16k mono | 10 ms hop | 中 | **3/5** |
| **NKF-AEC** | PyTorch | PyTorch | ref + mic | 16k mono | 16 ms hop,需改造 | 中-高 | **3/5** |
| Deep Echo Path Modeling | checkpoint | PyTorch | far + mic | 未产品化 | 论文实时指标 | 高 | 2.5/5 |
| DeepVQE / deepvqe-ggml | 社区实现 | GGML / PyTorch | mic + ref | 16k | 可实时 | 中 | 2.5/5 |
| TSPNN | ⚠️ 仓库 ONNX 实为 **AECMOS 评测器**非 AEC 模型 | eval harness | — | — | — | 中(评测) | 2/5(评测复用,非 AEC 模型) |
| EchoFree | ⚠️ 本地副本**只有 README,代码/模型未释出** | 不可用 | — | — | — | 不可用 | **弃用** |
| FADI-AEC | 论文 | 无可用权重 | mic + ref | 研究用 | 扩散,声称快 | 当前不可用 | 1.5/5 |
| LLaSE-G1 | HF 模型 | 大模型式 SE | 多任务 | 非低延迟 | 不适合实时桌面 AEC | 高 | 1.5/5 |

#### 一线可试模型

**LocalVQE(4/5,最推荐的 neural 试验核心)** — 面向实时的 neural voice quality enhancement,联合做 **AEC + noise suppression + dereverberation**。有 C++ GGML 推理、PyTorch 参考实现、公开 GGUF 权重、C API、OBS plugin 目录。
- 面向 16 kHz speech,commodity CPU real-time;v1.3 ≈ 4.8M 参数 / 约 19 MB F32,v1.2 ≈ 1.3M 参数 / 约 5 MB F32;256-sample hop = 16 ms 算法延迟。
- C API:`localvqe_process_f32()` / `localvqe_process_s16()`(16 kHz mono mic + 16 kHz mono ref),`localvqe_process_frame_f32()` / `localvqe_process_frame_s16()`(单 hop streaming);暴露 sample rate、hop length、FFT size、reset、residual-echo noise gate 配置。
- CMake 选项:`LOCALVQE_BUILD_SHARED`(shared lib + C API)、`LOCALVQE_CUDA`、`LOCALVQE_VULKAN`;bench target 能自动下载 HF 模型与 mic/ref WAV。只做 DLL/C API 可避开 libsndfile;构建 WAV CLI 则用 vcpkg 装 libsndfile。Windows 构建:
  ```bat
  git clone --recursive https://github.com/localai-org/LocalVQE.git
  cd LocalVQE
  cmake -S ggml -B ggml\build -A x64 -DCMAKE_BUILD_TYPE=Release -DLOCALVQE_BUILD_SHARED=ON
  cmake --build ggml\build --config Release
  ```
- 实时性:Ryzen 9 7900 4 线程,v1.3 p50≈3.21 ms / p99≈3.42 ms,16 ms hop 实时倍率≈4.97×;v1.2 p50≈1.65 ms / p99≈2.91 ms。移动端 Ryzen 7 6800U,v1.2 4 线程 p50≈2.11 ms / p99≈2.77 ms。
- 质量(ICASSP 2022 AEC Challenge blind,800 clip):v1.3 far-end single-talk ERLE≈50.9 dB、double-talk ERLE≈8.5 dB;v1.2 far-end single-talk≈45.7 dB、double-talk≈8.4 dB;另报 AECMOS echo/degradation 与 DNSMOS。**含义:far-end only 消得很狠(治"视频对白漏回去");double-talk ERLE 低是正常现象(输出保留近端语音),需看听感与 near-end degradation。**
- 短板:不支持 stereo ref(公开 API 为 mono)、不支持 48 kHz(当前 16 kHz)、项目较新,Windows 构建/声学泛化/near-end 失真需实测。
- 仓库:<https://github.com/localai-org/LocalVQE>;C API header:<https://raw.githubusercontent.com/localai-org/LocalVQE/main/ggml/localvqe_api.h>;CMake:<https://raw.githubusercontent.com/localai-org/LocalVQE/main/ggml/CMakeLists.txt>

**DTLN-aec(4/5,最成熟、最易试音的 baseline)** — Nils Westhausen / Oldenburg 的 Dual-Signal Transformation LSTM Network AEC,提交 Microsoft AEC Challenge 获第 3 名,MIT 许可。
- 三档模型:`dtln_aec_128`(128 units / 1.8M)、`dtln_aec_256`(256 / 3.9M)、`dtln_aec_512`(512 / 10.4M,提交版本)。
- `run_aec.py` 暴露输入输出形态:输入须 16 kHz 单声道;mic 文件 `*_mic.wav`、loopback/far-end 文件 `*_lpb.wav`;`block_len=512`、`block_shift=128`(16 kHz 下 = 8 ms hop);两个 TFLite interpreter(part1:mic magnitude + loopback magnitude + LSTM state → mask;part2:估计 block + loopback buffer + state → time-domain block),overlap-add 写回。可 frame-wise/streaming 化。
- Windows 嵌入:第三方 C wrapper `RogerTeng/DTLN_AEC` 提供 Windows x64 / macOS 预编译 TensorFlow Lite v2.5.2 + VS2019 工程,默认 hardcode `dtln_aec_128`,可用 `TfLiteModelCreateFromFile()` 替换 `TfLiteModelCreate()` 加载模型文件。
- 离线运行:
  ```bat
  git clone https://github.com/breizhn/DTLN-aec.git
  cd DTLN-aec
  python -m pip install -r requirements.txt
  python run_aec.py -i path\to\input_folder -o path\to\output_folder -m .\pretrained_models\dtln_aec_512
  ```
- 短板:2020/2021 时代架构、16 kHz mono、对 stereo 外放/房间变化无显式处理、需随产品分发 TFLite、官方模型较老。
- 仓库:<https://github.com/breizhn/DTLN-aec>;Win C wrapper:<https://github.com/RogerTeng/DTLN_AEC>

**NVIDIA Maxine AEC(3.5/5)** — 详见 §3.4。

#### baseline / research 模型

- **Microsoft AEC Challenge DEC baseline(3/5):** `baseline/icassp2022` 含 `dec-baseline-model-icassp2022.onnx`、`enhance.py`、`requirements.txt`,输入目录需 `_mic.wav` 与 `_lpb.wav`。`DECModel` 用 ONNX Runtime,16 kHz,`window_length=0.02`、`hop_fraction=0.5`(20 ms window / 10 ms hop);对 mic 与 far-end 分别 rFFT 取 magnitude/log-power 特征拼接,ONNX 输出 mask + 两个 hidden states,用 mic phase 重建。requirements 很旧(`onnxruntime==1.7.0`、`numpy==1.19.2`、`librosa==0.8.0`)。优点:ONNX Runtime Windows 集成舒服、模型公开、frame-wise recurrent mask 结构清楚,适合自动化 benchmark baseline。缺点:只是 challenge baseline 非最佳模型、Python 离线脚本、需自己重写 STFT/feature/hidden state、16 kHz mono、无 C API。Windows C++ 实时接入需自实现 `DecState{ session; h01[322]; h02[322]; mic_stft; far_stft; ola; }` + `Process10ms16k(mic_160, far_160, out_160)`。
  - 仓库:<https://github.com/microsoft/AEC-Challenge/tree/main/baseline/icassp2022>
- **NKF-AEC(3/5,偏乐观→实为参考):** Neural Kalman Filtering,`python nkf.py -x ref.wav -y mic.wav -o res.wav`。**明确是 linear acoustic echo canceller**,delay 大时需先做 time delay compensation,采样率 16 kHz。⚠️ **2026-06 核实**:权重 `src/nkf_epoch70.pt` **仅 28KB**(候选池最小,小到可塞进 hot path 作 SpeexDSP MVP 的零成本增量),但**无 train 脚本**,自家重训成本高于 DTLN——「3/5 算法参考」对可用性偏乐观;真正可直接复用的是其 **GCC-PHAT 延迟估计 30 行 numpy**(`src/utils.py:5-38`,可作 §4.1 external delay 模式的独立 sanity check)。核心是 complex GRU + complex dense 输出 Kalman gain;`torch.stft(n_fft=1024, hop_length=256, win_length=1024)`,对 far-end STFT 构造长度 L=4 向量,逐帧估计 echo,输出 `s_hat = istft(y - echo_hat)`。优点:参数极少、可解释、near-end degradation 可能低、echo path reconvergence 可能强于传统 Kalman。短板:线性 AEC 对音箱非线性失真/削波/复杂残余不够、仍需 residual suppressor、需 delay compensation、complex model 转 ONNX/C++ 不顺滑、无现成 Windows DLL。**适合作 classical AEC 替代或算法参考,不适合作 neural residual suppressor。**
  - 仓库:<https://github.com/fjiang9/NKF-AEC>
- **Deep Echo Path Modeling(2.5/5):** Interspeech 2024,含 `Network`、`infer.py`、`model.ckpt`。用深度学习在 T-F domain 预测 echo path 再相减:`S_hat[t,f] = D[t,f] - Σ_k H_hat[k,t,f] * X[t-k,f]`。保留「估计 echo path 再相减」物理结构,不易把 far-end 人声误当 near-end speech。问题:无产品级 streaming C/C++ runtime、repo 小社区验证少、需读 `infer.py` 确认 causal/lookahead/state、Windows 集成需 PyTorch→ONNX/TorchScript、真实外放双音箱场景未知。
  - 仓库:<https://github.com/ZhaoF-i/Deep-echo-path-modeling-for-acoustic-echo-cancellation>
- **DeepVQE / deepvqe-ggml(2.5/5):** 原始 DeepVQE(Microsoft 论文,联合 AEC/NS/dereverb)**没有官方权重、参考实现或 streaming runtime**;LocalVQE 正是受其启发后重建并发布了 GGML runtime 与权重的版本。`richiejp/deepvqe-ggml` 可作结构参考,产品接入优先级低于 LocalVQE。→ **不要追原始 DeepVQE,优先 LocalVQE。**

#### 暂不建议作为产品核心

- **TSPNN(2/5,评测复用而非 AEC 模型):** Interspeech 2023 两阶段渐进网络;⚠️ **2026-06 本地核实纠正早期判断**:仓库里拿到的 ONNX 是 **AECMOS 评测器**(`TSPNN/eval/eval.py`),**不是 TSPNN AEC 模型本身**——「无可下载 checkpoint」的结论方向反了,实际拿到的是一份现成评测 harness。AEC 模型仅有结构无权重。仓库:<https://github.com/enhancer12/TSPNN>
- **EchoFree(弃用):** 论文称超轻量(278K 参数 / 30 MMACs)、ICASSP 2023 AEC Challenge blind 接近轻量 SOTA;⚠️ **2026-06 本地核实:工作区副本只有 README,代码/模型未释出**,按工作区证据从「占位跟踪」直接**弃用**。仓库:<https://github.com/StellanLi/EchoFree>;论文:<https://arxiv.org/html/2508.06271v1>
- **FADI-AEC / diffusion AEC(1.5/5):** diffusion-based,FADI-AEC 通过每帧只运行一次 score model 降复杂度,面向 edge;但无公开权重/C/C++ runtime/streaming SDK。扩散模型在实时链路最怕非确定性伪影、near-end 音色变化、GPU/CPU jitter、长尾延迟、与 Discord 编码器叠加的 artifacts。研究方向,非工程方案。论文:<https://arxiv.org/html/2401.04283v1>
- **LLaSE-G1(1.5/5):** 统一 speech enhancement 大模型(支持 NS、TSE、PLC、AEC、Speech Separation);非低延迟设计、推理链复杂、无明确 10–20 ms streaming AEC C/C++ runtime,延迟/音色稳定性风险高。不适合实时桌面 AEC 核心。模型:<https://huggingface.co/ASLP-lab/LLaSE-G1>

#### Neural AEC 与 WebRTC AEC3 的关系

纯 neural AEC 不一定比 AEC3 更稳:

| 问题 | Classical AEC3 | Neural AEC |
|---|---|---|
| reference-based echo subtraction | 强 | 取决于模型结构 |
| echo path 自适应 | 强 | 训练泛化或隐式建模 |
| stereo reference | AEC3 更接近可处理 | 公开模型多数 mono |
| double-talk | AEC3 工程成熟 | 可能更自然,也可能吞字 |
| 非线性音箱失真 | AEC3 较弱 | neural 可能更强 |
| residual echo | 需要 suppressor | neural 可能更强 |
| 可解释和调参 | 较好 | 较差 |
| 48 kHz fullband | 支持 | 公开模型多为 16 kHz |
| Windows 产品嵌入 | C++ 可控 | 取决于 runtime |

因此更稳的成品架构:`WebRTC AEC3(线性 echo path、自适应、delay、double-talk 稳定性) + Neural postfilter/residual suppressor(视频人声残余、音乐残余、房间混响、非线性失真)`。若想做 neural-first 版本,LocalVQE 是目前最值得试的。

#### ⭐ 关键更新:AEC3 已把 neural 残余抑制从「串联」升级为「深度集成接口」

> 源码核实(2026-06,本地 `reference_repos/webrtc-aec3-src/`)修正本文早期「neural 只能作 AEC3 之外串联后处理」的判断——这是本轮探索最大的架构级认知更新。

当前官方 AEC3 源码已内置可注入的 **`NeuralResidualEchoEstimator` 抽象基类**(TFLite 实现),它**不是**串在 AEC3 之外的独立模块,而是 `ResidualEchoEstimator` 内部的一个 hook,**直接共享 AEC3 的全部中间状态**(`dominant_nearend / S2_linear / Y2 / E2`)。证据:

- 抽象接口与官方 tuning:`api-audio/neural_residual_echo_estimator.h:26-65`、`aec3/neural_residual_echo_estimator/neural_residual_echo_estimator_impl.cc:569-589`(官方给了 production `AdjustConfig`)。
- **「始终运行」铁律**:即便输出被忽略也要每 block 喂数据保持 LSTM state 一致(`aec3/residual_echo_estimator.cc:214-216`)——任何 stateful neural 后端不能按需启停。
- **双 mask + DTD 二值选择**:`dominant_nearend=true` 用 unbounded mask(保近端),false 用 bounded(狠压残余)(`neural_residual_echo_estimator_impl.cc:541-566`)。**DTD 是 neural REE 的 conditioning input,不应让 neural 独立判 DTD。**
- neural 激活时关 coarse filter + 切 suppressor tuning(`aec3/echo_remover.cc:431,513-520`)。
- **工程坑**:仓库不含模型;frame 硬锁 256 样本(16ms@16k),只跑 band0;TFLite `ModelRunner` 强制 `SetNumThreads(1)`(`neural_residual_echo_estimator_impl.cc:411-415`),直接反驳「多线程 p50 3.21ms」类标称,production 须重新 benchmark。

**对架构决策的影响:** 「AEC3 主消 + LocalVQE/DTLN 残余」的工程量从「写独立 neural 模块 + 自己做时序对齐」降到「写一个 `NeuralResidualEchoEstimator` 子类」(如 `LocalVqeNeuralReeAdapter : public NeuralResidualEchoEstimator`),接口与时序对齐成本显著下降。但**前提是走 official C++ 路线**;若走 `sonora`(见 §3.2),当前 `sonora-aec3` 未 port 此接口,需先补 port。这条更新影响 §9.4 引擎抽象(IAecEngine 应把 neural REE 作为 AEC 引擎内部可选项,而非管线后一级)。完整逐条证据见 `research/reference_repos_exploration_report.md` §3.5 / §4.1。

#### Neural AEC 工程风险

1. **公开模型大多只支持 mono reference** — 你的场景左右音箱 echo path 不同,简单 `ref_mono = 0.5*L + 0.5*R` 在 L/R 差异大时丢空间信息。三种方案:stereo downmix(低复杂度,MVP 可用)/ 两路分别跑模型再融合(中,未必有效,模型没这样训练)/ **AEC3 先处理 stereo reference,neural 只做 residual(中,最稳)**。
2. **16 kHz 影响 Discord/VRChat 音质** — 多数 neural AEC 只输出 16 kHz;upsample / neural super-resolution / EQ 只补高频,不是真正恢复原始 48 kHz。
3. **近端人声损伤** — 训练分布外时可能:你说话音色变薄、开头音被吞、double-talk 断续、唱歌/笑声/喊叫被当 echo/noise、残余 echo 转 musical noise。不能只看 ERLE,需 AECMOS / DNSMOS / 主观 AB。
4. **Reference 对齐仍是核心** — neural AEC 不是魔法,WASAPI loopback 与 USB mic 因 delay/drift 不对齐照样失效,尤其 DTLN/DEC 这类短上下文模型。

#### Neural 选型优先级

1. **LocalVQE** — 最强 neural 候选(有权重 / C++ GGML / C API / streaming frame API / CPU 实时 / 同时做 AEC+NS+dereverb)。v1.2(更小更快更温和)与 v1.3(消 echo 更强、double-talk degradation 指标更好,far-end-only residual 听感可能更粗糙)都测。
2. **DTLN-aec** — 成熟 TFLite baseline,快速听感对比,测试顺序 `128 → 256 → 512`(128 够则集成轻很多)。
3. **Microsoft DEC baseline** — ONNX Runtime 集成舒服,作自动化 evaluation harness 公开基准线,不建议产品化。
4. **NVIDIA Maxine AEC** — RTX 用户效果上限对照(详见 §3.4)。

### 3.4 NVIDIA 方案专题(Broadcast App vs Maxine/AFX SDK)

需要严格区分两件事:**用户级的 NVIDIA Broadcast App 没有 reference-based AEC**;**开发者级的 Maxine / AFX SDK 才有真正的 AEC**。

#### 3.4.1 NVIDIA Broadcast App 没有你要的 AEC

NVIDIA Broadcast 产品页公开的音频能力是 **Noise Removal、Room Echo Removal、Studio Voice**,描述的是键盘声、麦克风静电、风扇声、房间混响等背景问题,没有「拿扬声器播放参考信号消掉麦克风回声」的 AEC。

- Broadcast 1.2 引入的 **Room Echo Removal** 官方解释为处理「房间声学差导致你的声音听起来 echoey / reverb」,即**近端人声在房间里的混响/房间反射,不是 reference-based AEC**(原文:"removing the echoey sound of your voice in rooms with poor acoustics")。release notes 把 1.2.0 写成 "Room Echo Removal (beta): reduces room reverb from your audio",2.0.1 恢复 "Background Noise Removal" 与 "Room Echo Cancellation" 强度滑块;整个 release highlights 没把它称为 Acoustic Echo Cancellation。
- 更关键:Maxine/AFX SDK 文档把 **Room Echo Removal** 和 **Acoustic Echo Cancellation** 分成两个不同 effect。Room Echo Removal 在 SDK 里等价于 Dereverb / Room Echo Removal / Room Echo Cancellation,目标是消录音里的房间混响,支持 16k/48k 32-bit float。所以 Broadcast 里的 "Room Echo Removal / Cancellation" 准确归类是「单端 mic enhancement / dereverb」,不是 reference-based AEC。
- **虚拟 `Speaker (NVIDIA Broadcast)` 不是 AEC reference。** 官方设置指南要求在 Discord 选 NVIDIA Broadcast Microphone/Speaker/Camera,同时提醒 Windows 默认输出仍应设成真实耳机/音箱,不要把 NVIDIA Broadcast Speakers 设成系统默认(否则过滤所有系统声)。这说明 Speaker 更像「对 incoming audio 做单端增强的虚拟扬声器」,不是 mic AEC 的 far-end reference。

**为什么「视频男性对白过不掉」符合此判断:** 真 AEC 行为是 `系统播放对白 x → 音箱→空气→麦克风;mic y = 我的声音 s + 对白回声 e;AEC(x,y) → ≈ s`。若 Broadcast 只做 mic-only 的 NS/dereverb,它看到的只有「我的声音 + 音箱里的人声对白」,视频对白在声学上就是「另一个人声」,模型很容易当成有效 speech 保留,无法精确相减。这与观察一致:稳态噪声/键盘声/房间混响能压;外放视频对白/游戏语音/Discord 对方人声不稳定甚至直接通过。

| 问题 | 判断 |
|---|---|
| Broadcast 有 Noise Removal? | 有 |
| Broadcast 有 Room Echo Removal / Dereverb? | 有 |
| Broadcast 有 Studio Voice? | 有 |
| Broadcast 有面向用户的 reference-based AEC? | **公开资料看不到,基本判定没有** |
| Broadcast 能否稳定解决外放把视频/Discord/游戏语音回传? | **不应期待** |
| Maxine / AFX SDK 有真正 AEC? | **有** |
| Maxine AEC 能用于自研工具? | 技术上可评估,但要自做 WASAPI loopback reference、delay/drift 对齐、虚拟麦克风输出、SDK 分发 |

> **简单验证实验:** 音箱外放干净男声对白、麦克风正常位置、本人静音、Discord/OBS 录 `Microphone (NVIDIA Broadcast)`,分别测 A.只 Noise Removal / B.Room Echo Removal / C.Studio Voice / D.同时选 Speaker (NVIDIA Broadcast) 作 Discord 输出。若是真 AEC 且 reference 正确,near-end 静音时输出应接近静音;若仍能听见对白(只是音色变薄/断续),即单端增强而非 reference-based AEC。

参考:Broadcast App <https://www.nvidia.com/en-us/geforce/broadcasting/broadcast-app/>;1.2 更新 <https://www.nvidia.com/en-us/geforce/news/may-2021-nvidia-broadcast-update/>;release notes <https://www.nvidia.com/en-us/geforce/broadcasting/broadcast-app/release-notes/>;Room Echo 文档 <https://docs.nvidia.com/maxine/afx/latest/AboutTheEffects/AboutRoomEchoRemovalCancellation.html>;设置指南 <https://www.nvidia.com/en-us/geforce/guides/broadcast-app-setup-guide/>

#### 3.4.2 Maxine / AFX SDK 才有真 AEC

Maxine AFX SDK 有单独的 `aec` effect(`NVAFX_EFFECT_AEC = "aec"`),NVIDIA 文档定义其为 Acoustic Echo Cancellation,输入两路:

```text
near-end microphone signal y   (= 近端语音 s + far-end speaker echo e)
far-end speaker/reference signal x
```

输出 `s' = (s + e) - e`;只有 far-end echo、近端静音时输出应为静音。支持 16k / 48k、32-bit float。SDK README 列出支持的效果:Background Noise Suppression、Room Echo Cancellation / Dereverb、Dereverb + Denoiser、**Acoustic Echo Cancellation (AEC)**、Audio Super Resolution;要求带 Tensor Cores 的 NVIDIA GPU,Windows 10/11,模型/DLL 来自 NVIDIA installer。GitHub sample v1.3.0 发布于 2025-06-06。

参考:AEC 文档 <https://docs.nvidia.com/maxine/afx/latest/AboutTheEffects/AboutAcousticEchoCancellation.html>;AFX SDK <https://github.com/NVIDIA-Maxine/Maxine-AFX-SDK>

#### 3.4.3 能不能「直接用」的分层判断

| 层级 | 能否直接用 | 说明 |
|---|---|---|
| 离线跑 sample | 可以 | 下载 SDK + AEC feature/model,用 sample 跑 WAV |
| 写 C++ 原型调用 AEC | 可以 | API 直接:create effect → set model → load → 每帧 `NvAFX_Run()` |
| 接入实时 Windows 音频管线 | 可以,但要自己做大量工程 | WASAPI loopback、mic capture、delay/drift 对齐、虚拟麦克风**都不由 SDK 提供** |
| 面向普通用户一键分发 | 有明显阻碍 | RTX/Tensor Core 要求、驱动版本、模型/DLL 分发、许可、fallback |
| 替代 WebRTC AEC3 作唯一核心 | 不建议 | 用户覆盖面、闭源、硬件依赖、可调试性都弱于开源主线 |

#### 3.4.4 C/C++ API 与最小接入

```cpp
NvAFX_Handle aec = nullptr;
NvAFX_CreateEffect(NVAFX_EFFECT_AEC, &aec);
NvAFX_SetString(aec, NVAFX_PARAM_MODEL_PATH, model_path);
NvAFX_SetU32(aec, NVAFX_PARAM_NUM_SAMPLES_PER_INPUT_FRAME, frame_samples);
NvAFX_Load(aec);

const float* input[2];
float* output[1];
input[0] = near_end_mic_frame;  // mic: your voice + echo
input[1] = far_end_ref_frame;   // speaker/reference audio
output[0] = clean_mic_frame;
NvAFX_Run(aec, input, output, frame_samples, /*num_channels=*/2);

NvAFX_DestroyEffect(aec);
```

`NvAFX_Run()` 签名(AEC 需指定两个 channel:第一个 near-end,第二个 far-end;输入输出 buffer 在 CPU memory,GPU 传输由 SDK 内部处理):

```cpp
NvAFX_Status NvAFX_Run(
    NvAFX_Handle effect,
    const float** input,
    float** output,
    unsigned num_samples,
    unsigned num_channels);
```

SDK Windows 包分三部分:core package + 从 NGC 下载的 AI features + GitHub 上的 sample;需设 `AFX_SDK_ROOT`。最小流程:

```bat
:: 1. 安装/解压 Windows AFX SDK core package
set AFX_SDK_ROOT=C:\NVIDIA\AFX_SDK
:: 2. 下载 AEC feature/model
cd %AFX_SDK_ROOT%\features
set NGC_API_KEY=你的_NGC_API_KEY
powershell -ExecutionPolicy Bypass -File .\download_features.ps1 ^
  --gpu_architecture ampere --ngc-org nvidia --ngc-team maxine --effects aec-16k,aec-48k
:: 3. 跑 sample(按 GPU 架构 / aec / 16k|48k 采样率)
run_effect_demo.bat ampere aec 48k 48k
```

参考:安装/feature 下载 <https://docs.nvidia.com/maxine/afx/2.0.0/WindowsAFXSDK/InstallTheAFXSDK.html>;NvAFX_Run <https://docs.nvidia.com/maxine/afx/2.0.0/UseAFXInApps/LoadRunDestroyAnEffect.html>;sample 源码 <https://raw.githubusercontent.com/NVIDIA-Maxine/Maxine-AFX-SDK/main/samples/effects_demo/effects_demo.cpp>

#### 3.4.5 主要阻碍(产品化关键)

1. **硬件门槛(最大阻碍)** — 必须 NVIDIA Tensor Core GPU;Windows 要求 64-bit Win10、VS 2017+、CMake 3.9+、NVIDIA 驱动 572.61+。无 RTX / 无 Tensor Core / 驱动过旧 / AMD / Intel / 老 GTX 用户不可用 → **必须有 WebRTC AEC3 fallback**。
2. **非纯开源包** — GitHub 只是 API source + sample,DLL/模型/依赖来自 NVIDIA installer,影响 CI、自动构建、安装器、离线安装、版本锁定。
3. **NGC 下载麻烦** — 需 NGC API key、org/team、GPU 架构参数;消费级产品要决定:A.安装器内置 SDK DLL+model / B.首次运行下载 / C.要求用户自装(C 体验差,A/B 须核对许可)。
4. **分发许可** — SDK 仅授权用于含 NVIDIA GPU 的系统;桌面应用需按 Maxine branding guidelines 做归属标识。GitHub sample 是 MIT,但 DLL/模型/feature 受 NVIDIA SDK License 约束。上线前须确认:能否把 model 打包进 installer、能否静默安装 runtime、是否需展示 attribution、是否允许具体商业/开源分发模式、是否允许消费级桌面工具。
5. **不提供 Windows 音频管线** — loopback / mic capture / 默认设备切换 / 采样率转换 / delay 对齐 / clock drift 补偿 / 虚拟麦克风输出**全要自研**(正是项目最难部分)。
6. **Reference 对齐仍决定效果** — `x`/`y` 差几十毫秒且不补偿,AEC 明显变差;仍需 estimated_render_to_mic_delay_ms、clock_drift_ppm、dynamic resampling、render reference ring buffer、mic-clock-driven processing。
7. **输入格式限制** — 32-bit float、16k/48k(对本项目不算坏事,Discord/VRChat 本就建议 48k);但 sample 的 WAV reader 要求 mono,near/far 都按 mono 读。stereo 音箱简单 downmix `farend_mono = 0.5*L + 0.5*R` 可能丢左右 echo path 信息;是否支持 stereo far-end 需用 `NVAFX_PARAM_NUM_INPUT_CHANNELS` 查询,公开文档无 stereo 双参考产品级说明。稳妥起点 mono reference,但可能不如 AEC3 stereo 路线可控。
8. **GPU 调用不能放实时回调线程** — `NvAFX_Run()` 含 CPU↔GPU 拷贝,不要在 WASAPI capture callback/packet loop 里阻塞调用,否则 GPU 调度抖动造成采集 underrun 或输出断音。推荐:`WASAPI mic thread → lock-free ring`、`WASAPI loopback thread → timestamped render ring`、`processing thread → delay align + NvAFX_Run`、`virtual mic writer → output ring`。
9. **闭源黑盒,调试能力有限** — 可观测的主要是输入输出波形、`NvAFX_Status`、GPU/driver/model load 状态、主观听感、外部 ERLE/AECMOS/DNSMOS;异常时难定位是 delay / drift / 模型泛化 / reference 失配 / GPU 抖动 / 非线性失真哪一个。这是不建议作唯一核心的核心原因。
10. **训练目标可能偏会议 AEC** — 文档表述偏 conferencing;你的 reference 可能含 YouTube 男声、游戏音效、音乐、Discord 对方声音、系统提示音,模型对这些分布外声音可能产生伪影或残余,需实测。

参考:Windows 入门/硬件要求 <https://docs.nvidia.com/maxine/afx/2.0.0/WindowsAFXSDK/GetStartedOnWindows.html>;SDK License <https://developer.nvidia.com/downloads/maxine-sdk-license>;Run sample <https://docs.nvidia.com/maxine/afx/2.0.0/WindowsAFXSDK/RunTheSampleApplication.html>;AFX Functions <https://docs.nvidia.com/maxine/afx/latest/APIReference/AFXFunctions.html>

#### 3.4.6 接入定位

把 Maxine AEC 放成一个**可插拔 Engine Backend**(统一 `IAecEngine` 接口见 §9.4),而非改掉整个架构。实现 `WebRtcAec3Engine` / `SpeexAecEngine` / `NvidiaMaxineAecEngine` 三个 backend,运行时选择:

```text
if 兼容 NVIDIA Tensor Core GPU + SDK runtime/model available:
    allow user to choose "NVIDIA Maxine AEC"
else:
    use WebRTC AEC3
```

#### 3.4.7 Maxine 专项验证步骤

- **离线:** 录 `mic_48k_mono.wav`(USB mic:你的声音 + 音箱回声)+ `farend_48k_mono.wav`(WASAPI loopback 抓的系统播放,已对齐/降 mono),跑 `run_effects_demo.bat -g <arch> -e aec -isr 48k -osr 48k` 或自写最小 `NvAFX_Run()`(sample config 用 `input_wav` + `input_farend_wav`)。重点:far-end only 输出是否近静音、double-talk 是否吞字、music/game 残余、delay sweep ±200 ms 容忍范围。
- **实时原型:** `WASAPI loopback 48k stereo → downmix mono → timestamped render ring → delay/drift aligner → far_end_mono_48k`;`USB mic 48k mono → near_end_mono_48k`;`NvAFX_Run(aec) → VB-Cable Input → Discord 选 VB-Cable Output`。注意:即使 Maxine 很强,没有 delay/drift aligner 也不代表可用。
- **产品化:** 驱动过旧时错误提示是否清楚、RTX 20/30/40/50 兼容性、laptop dGPU/Optimus 能否选对 GPU、睡眠/唤醒后 NvAFX handle 是否失效、GPU 满载游戏时 AEC 是否爆音、SDK/model 能否合法随 installer 分发。

**Maxine 最终判断:** 能直接用于原型(有真 AEC、16k/48k float、C API、Windows sample、model 下载路径);不能直接解决产品问题(只是引擎,系统音频采集/对齐/漂移补偿/虚拟麦克风仍需自做)。适合定位:`默认核心 WebRTC AEC3 + RTX 用户可选 NVIDIA Maxine AEC + 兜底/调试 SpeexDSP`。

---

## 4. 核心引擎源码级分析

### 4.1 WebRTC AEC3

#### API 与调用模型

AEC3 核心外部形态:

```cpp
aec.AnalyzeRender(render_10ms);   // 系统播放参考写入内部队列
aec.ProcessCapture(mic_10ms);     // 取出对应 render 数据处理麦克风帧
```

`AnalyzeRender()` 将 render 数据写入 queue,`ProcessCapture()` 处理 capture 同时检查 sample rate、声道数、delay、capture output 等;构造函数按 full-band sample rate、render/capture 声道数初始化队列、block processor 与重采样/分频对象。接口按 **10 ms 帧**工作,内部切成 **64-sample block**。这天然契合 Windows:WASAPI loopback 和 USB mic 采集本就是两条异步流,只需把两路对齐到 10 ms frame 再依次调用。

源码:<https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/echo_canceller3.cc>;头文件 <https://chromium.googlesource.com/external/webrtc/+/master/modules/audio_processing/aec3/echo_canceller3.h>

#### 核心算法

自适应滤波器是 **frequency-domain partitioned adaptive FIR**,`adaptive_fir_filter.cc` 中的频域更新形式:

```text
H(t+1) = H(t) + G(t) * conj(X(t))
```

按 CPU feature 选择 C / SSE2 / AVX2 / NEON 路径,Windows x64 走 SSE2 / AVX2。源码:<https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/adaptive_fir_filter.cc>

AEC3 不是孤立滤波器。`echo_remover.cc` 把 subtractor、suppression gain、comfort noise generator、suppression filter、render signal analyzer、residual echo estimator、AEC state 组合起来,在 echo path 变化、render gain 变化、filter 输出分支选择、残余回声估计与抑制之间切换。源码:<https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/echo_remover.cc>

残余回声估计不只依赖线性滤波误差,还用线性估计、非线性 echo path gain、render buffer echo-generating power、soft noise gate、混响模型,源码保留 neural residual echo estimator hook。源码:<https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/residual_echo_estimator.cc>

#### 模块路径与算法细节(A 级证据)

相关源码路径:`echo_canceller3.h/.cc`、`aec_state.cc`、`echo_path_delay_estimator.*`、`adaptive_fir_filter*`、`matched_filter*`、`render_delay_buffer*`、`residual_echo_estimator.*`、`suppression_gain.*`、`suppression_filter.*`、`comfort_noise_generator.*`、`dominant_nearend_detector.*`、`subband_nearend_detector.*`、`reverb_model.*`、`reverb_decay_estimator.*`、`clockdrift_detector.*`、`delay_estimate.*`、`matched_filter_lag_aggregator.*`、`audio_processing_impl.cc`、`BUILD.gn`。

| 项 | 源码证据 | 判断 |
|---|---|---|
| 帧长 | 头文件要求 10 ms 段,内部切 64-sample block(`kBlockSize=64`、`kBlockSizeMs=4`) | 适合 48 kHz / 10 ms 管线 |
| 自适应滤波 | `adaptive_fir_filter*`、`matched_filter*`、`render_delay_buffer*`、`echo_remover*` | 频域/block 化 AEC3 系统 |
| delay estimation | `echo_path_delay_estimator.*`、`delay_estimate.*`、`matched_filter_lag_aggregator.*` | 内置 delay 估计,但应用层仍需 reference buffer 对齐 |
| 双讲 / near-end | `dominant_nearend_detector.*`、`subband_nearend_detector.*`、`AecState` dominant near-end / usable linear estimate | 比 Speex 完整 |
| 残留回声 | `residual_echo_estimator.*`、`suppression_gain.*`、`suppression_filter.*`、`comfort_noise_generator.*` | 有 residual echo suppression / NLP 类处理 |
| reverb | `reverb_model.*`、`reverb_decay_estimator.*` | 对房间尾音有模型 |
| drift | `clockdrift_detector.*`、config `echo_removal_control.has_clock_drift` | 不等于完整应用层 drift compensation |
| stereo / multi-channel | APM `multi_channel_capture` / `multi_channel_render` 配置;`AudioProcessingImpl` 按 pipeline 配置决定是否 downmix | 支持多通道管线,本项目需实测 |

关键模块:`AecState`(跟踪 filter convergence、delay、ERL/ERLE、saturation、dominant near-end)、`EchoCanceller3Config`(filter、delay、suppressor、echo removal control 等)、`AudioProcessingImpl`(整合 AEC、NS、AGC、HPF、transient suppression)。

#### 源码级细节纠正(2026-06 本地 `webrtc-aec3-src/` 核实)

> 这些是早期基于在线文档的描述与本地源码的出入,工程接入前必须按这里为准。

| 纠正项 | 源码证据 | 工程含义 |
|---|---|---|
| **内部 block 是 4ms 不是 10ms** | `aec3/aec3_common.h:35`(`kBlockSize=64`、`kBlockSizeMs=4`) | 10ms 是 API 外部帧,内部按 4ms block 跑;延迟/对齐预算按 4ms 粒度算 |
| **48k 输入下主自适应滤波只跑 band0(0–8kHz)** | `aec3/aec3_common.h:57-58` | 高频两 band 走共享 gain 而非独立建模——**外放音箱齿音/共振的高频残余 echo,在现成模型/参考方案中仍缺强证据,是产品差异化点** |
| **`SetAudioBufferDelay` 内部按 16kHz round** | `aec3/render_delay_buffer.cc:338-342` | 传入 delay 必须是 **4ms 倍数**,否则被静默截断 |
| **`use_external_delay_estimator` 切换后内部 matched_filter 完全 bypass** | `aec3/render_delay_buffer.cc:351,375-384`;config `echo_canceller3_config.h:55` | 外放场景应启用 external delay(把 WASAPI 时间戳接进来,也是 neural REE 拿前瞻 render 的前提),但**失去内部兜底,必须外面再跑一份独立 GCC-PHAT sanity check**(NKF 的 GCC-PHAT 30 行可复用,见 §3.3) |
| **`matched_filter` 最大 lag ≈ 152ms** | `aec3/matched_filter*` | 这是内部 delay search 上限;更长 echo path 须靠应用层粗对齐先拉进窗口 |
| **`clockdrift_detector` 是 stub,只分类不补偿** | `aec3/clockdrift_detector.*` | 只输出 None/Probable/Verified 三档分类,**不做任何重采样补偿**——drift 闭环必须应用层做(见 §6) |
| **filter tail 无硬上限** | `EchoCanceller3Config::Validate()` 只 `FloorLimit(1)` | `length_blocks` 可上调到 64 blocks(≈256ms)做外放长 tail 试验;sonora 48k full pipeline benchmark ≈ 13.3µs/10ms 帧(`sonora/BENCHMARKS.md`),有调大空间 |

#### Echo tail length

默认 refined / coarse filter `length_blocks = 13`,按 16 kHz 低频带:

```text
13 * 64 samples / 16000 Hz ≈ 52 ms
```

这对耳机漏音或近距离扬声器可能够用,但对桌面两只外放音箱 + 房间反射偏短。**外放方案应把 filter length 做成可调,在 CPU 预算允许时测试 128–256 ms 量级有效 tail。** AEC3 config 还暴露 delay、filter、ERLE、residual echo、suppressor、dominant near-end、stereo detection 等参数组。源码:<https://webrtc.googlesource.com/src/+/master/api/audio/echo_canceller3_config.h>、<https://webrtc.googlesource.com/src/+/master/modules/audio_processing/aec3/aec3_common.h>

#### 双讲与近端保护

不是简单传统 DTD 函数,而是散布在 update gain、AEC state、dominant near-end detection、suppressor、residual echo estimator 等路径。`refined_filter_update_gain.cc` 里 update gain 受 render power、filter 输出、ERL、poor excitation、saturation、调用计数等控制,避免不适合学习时过度更新。比 SpeexDSP 更适合「对方说话 + 我也插话」场景。源码:<https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/refined_filter_update_gain.cc>

#### 立体声处理

源码有 multichannel content detection 与 proper stereo detection 分支;render/capture 声道数进入构造与 reinit 流程。对两只外放音箱,应保留 stereo render reference,让 AEC 区分左右声道不同声学路径,而非过早 downmix 成 mono。

#### Windows 构建

官方构建需 depot_tools、GN、Ninja。AEC3 在 `modules/audio_processing/aec3/BUILD.gn` 是 `rtc_library("aec3")`,源码列表含 delay estimator、adaptive filter、residual echo estimator、suppressor、clockdrift detector 等;BUILD 为 AVX2 分支设置 Windows `/arch:AVX2`。

```bat
git clone https://chromium.googlesource.com/chromium/tools/depot_tools.git C:\src\depot_tools
set PATH=C:\src\depot_tools;%PATH%
set DEPOT_TOOLS_WIN_TOOLCHAIN=0

mkdir C:\src\webrtc-checkout
cd /d C:\src\webrtc-checkout
fetch --nohooks webrtc
gclient sync

cd src
gn gen out\aec3_win_x64 --args="target_os=""win"" target_cpu=""x64"" is_debug=false is_component_build=false rtc_include_tests=false rtc_build_examples=false treat_warnings_as_errors=false"
ninja -C out\aec3_win_x64 modules/audio_processing/aec3:aec3
ninja -C out\aec3_win_x64 webrtc
```

工程上更建议封装自己的小 C API:

```c
typedef struct MyAec MyAec;
MyAec* my_aec_create(int sample_rate, int render_channels, int capture_channels);
void my_aec_analyze_render(MyAec* aec, const float* render_interleaved, int frames);
void my_aec_process_capture(MyAec* aec, const float* capture_mono, float* output_mono, int frames);
void my_aec_set_delay_ms(MyAec* aec, int delay_ms);
void my_aec_get_stats(MyAec* aec, MyAecStats* stats);
void my_aec_destroy(MyAec* aec);
```

源码/文档:BUILD.gn <https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/BUILD.gn>;Native build <https://webrtc.github.io/webrtc-org/native-code/development/>

#### 工程经验指标与结论

Switchboard 的 AEC3 文章给出经验指标:ERLE 20–40 dB 通常不错,低于 10 dB 往往说明 reference / 延迟 / 设备路径有问题,正常收敛通常在 1–2 秒量级——可作为测试 dashboard 起点。参考:<https://switchboard.audio/hub/how-webrtc-aec3-works/>

短板不是算法,而是:① 构建链重;② 内部 API 变动风险(讨论中老 `webrtc::Config` builder 已变,需用 `AudioProcessing::Config` / `ApplyConfig`);③ 官方 AEC3 target 不是给第三方稳定 ABI 用的小库;④ 外放场景仍要自己解决 delay / drift / stereo / tail;⑤ 输出到 Discord 仍需虚拟设备。**可用度:4–5 / 5(成品核心)。** 建议复用整个 APM 或 AEC3 + 必要 support modules,避免只摘单个 filter。

### 4.2 SpeexDSP Echo Canceller

#### API

```c
SpeexEchoState *speex_echo_state_init(int frame_size, int filter_length);
SpeexEchoState *speex_echo_state_init_mc(int frame_size, int filter_length, int nb_mic, int nb_speakers);
void speex_echo_cancellation(SpeexEchoState *st, const spx_int16_t *rec, const spx_int16_t *play, spx_int16_t *out);
// 另有 speex_echo_playback() / speex_echo_capture() 分离 API、speex_decorrelate_*
```

文档建议 frame size 约 10–20 ms,filter length 通常 100–500 ms;提供多麦克风/多扬声器初始化。本项目参数起点:

```cpp
constexpr int sample_rate   = 48000;
constexpr int frame_size    = 480;     // 10 ms
constexpr int filter_length = 14400;   // 300 ms
constexpr int nb_mic        = 1;
constexpr int nb_speakers   = 2;
SpeexEchoState* st = speex_echo_state_init_mc(frame_size, filter_length, nb_mic, nb_speakers);
int sr = sample_rate;
speex_echo_ctl(st, SPEEX_ECHO_SET_SAMPLING_RATE, &sr);
```

头文件:<https://github.com/xiph/speexdsp/blob/master/include/speex/speex_echo.h>

#### 核心算法

`mdf.c` 注释说明实现的是 MDF / AUMDF 变体,引用 Valin 2007 的 variable learning rate;同处明确**没有显式 double-talk detection**,依赖学习率和双路径机制。源码有 `TWO_PATH` foreground / background filter 逻辑,用于困难信号和双讲下避免滤波器劣化。核心路径含 FFT、频域乘法、foreground filter、梯度计算、权重更新、AUMDF 约束;多声道初始化中 `M` = 扬声器数、`C` = 麦克风数、`K` = 分块数,`K = (filter_length + frame_size - 1) / frame_size`。源码:<https://github.com/xiph/speexdsp/blob/master/libspeexdsp/mdf.c>(donut-release 镜像:<https://android.googlesource.com/platform/external/speex/+/donut-release/libspeex/mdf.c>)

#### 算法细节(A 级)

| 项 | 源码 / 文档证据 | 判断 |
|---|---|---|
| 帧长 | 头文件建议 10–20 ms | 可用 48 kHz / 10 ms = 480 samples |
| echo tail | 头文件建议 100–500 ms;manual 给小房间 100 ms 例子,并说 tail ≈ 房间混响时间的 1/3 | 外放建议从 200–400 ms 试起 |
| 自适应滤波 | `mdf.c` 注释 MDF + AUMDF | 频域 MDF 类 AEC |
| 双讲 | `mdf.c` 注释:robust double-talk 依赖 variable learning rate,无显式 DTD | 外放双讲风险明显 |
| residual suppression | manual 建议用 preprocessor 做 residual echo suppression | MVP 可启用,注意语音失真 |
| stereo reference | `speex_echo_state_init_mc` 支持 nb_mic / nb_speakers | 可测 stereo speaker reference |
| drift | API 无完整 drift compensation | 应用层必须处理 |

manual:<https://www.speex.org/docs/manual/speex-manual/node7.html>

#### 构建

最大优势是 Windows 接入简单,vcpkg 有现成 port(SpeexDSP 1.2.1 为 2022-06-17 发布;仓库有 `win32` 目录与 `README.win32`,C 代码可 MSVC/MinGW 构建;vcpkg CMake imported target 细节曾有 issue,需工程验证):

```bat
git clone https://github.com/microsoft/vcpkg C:\src\vcpkg
C:\src\vcpkg\bootstrap-vcpkg.bat
C:\src\vcpkg\vcpkg install speexdsp:x64-windows

cmake -S . -B build -A x64 ^
  -DCMAKE_TOOLCHAIN_FILE=C:\src\vcpkg\scripts\buildsystems\vcpkg.cmake ^
  -DCMAKE_BUILD_TYPE=Release
cmake --build build --config Release
```

参考:<https://www.speex.org/>、<https://github.com/xiph/speexdsp>、vcpkg issue <https://github.com/microsoft/vcpkg/issues/37412>

#### 结论

最快拿到可运行 MVP 的 AEC;缺点是算法年代较老,double-talk / 残余回声 / 非线性处理不如 AEC3,对「音箱播放男性对白 + 我也说话」上限有限。**可用度:3 / 5 做 MVP,2 / 5 做最终产品(成品保留为 fallback)。**

---

## 5. Windows 音频采集方案

### 5.1 WASAPI loopback 是首选 reference 获取方式

对本场景,默认应抓**整个默认播放设备的 loopback mix**:YouTube、游戏、视频播放器、Discord 对方声音都可能从音箱出来并漏进麦克风,AEC reference 必须覆盖这些。WASAPI 包含 loopback 的主要理由之一就是支持 AEC。

loopback 特性与性质(A 级):

| 问题 | 结论 |
|---|---|
| 初始化 | 在 render endpoint 上初始化 capture,加 `AUDCLNT_STREAMFLAGS_LOOPBACK` |
| shared / exclusive | loopback 只能 shared mode |
| 采集范围 | 采到整个 render endpoint 的混音,而非默认单个 app |
| 数据格式 | loopback 数据是 device format |
| event-driven | Windows 10 1703 之后 loopback capture 支持 event-driven |
| 硬件 loopback pin | 有硬件 pin 时用硬件,否则 audio engine 复制输出到 loopback buffer |
| 静音时 | 可出现 silent packet,需按 flags 处理 |
| 默认设备切换 | stream 可能失效,需监听并重建 |
| Discord 单独 reference | Windows 10 build 20348+ 有 process loopback sample,可 include/exclude process tree;但对 AEC,endpoint mix 更可靠 |

`IAudioCaptureClient::GetBuffer` 返回 packet、device position、QPC timestamp,并通过 flags 报告 silence、data discontinuity、timestamp error——是 delay alignment、drift 估计、glitch 诊断的关键输入。

参考:loopback recording <https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording>;GetBuffer <https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer>;Application Loopback sample(build 20348+)<https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/>

**单 app reference 是否必要:** 默认目标是「音箱实际播放了什么就消什么」,因此 **endpoint loopback mix 是主 reference**。单 app reference 更适合「只消 Discord 对方声音,不消游戏声」的特殊模式;但若游戏/视频/音乐也从音箱播放并漏入麦克风,不放进 reference 会降低 AEC 效果。

#### 推荐采集格式

| 流 | 推荐格式 | 说明 |
|---|---|---|
| Render loopback | 48 kHz, stereo, float32, 5–10 ms quantum | 保留 stereo reference |
| Mic capture | 48 kHz, mono, float32, 5–10 ms quantum | USB mic 若 44.1 kHz / 16-bit,立刻转换 |
| AEC processing | 48 kHz, 10 ms frames | WebRTC 支持 16 / 32 / 48 kHz full-band |
| Output virtual mic | 48 kHz, mono, float32 或 int16 | Discord / VRChat 兼容性最好 |

**不要为了 ASR 把整个链路降到 16 kHz。** Project Raven 降 16 kHz 是因为面向 Deepgram / 转写(详见 §3.2);Discord / VRChat 场景保留 48 kHz 更合理。

### 5.2 USB 麦克风采集

推荐 WASAPI shared-mode event-driven capture:48 kHz mono 优先(若 USB mic 提供 stereo,可在 AEC 前 downmix 或保留 stereo 实验);用 `IAudioClient` / `IAudioCaptureClient`;读 QPC timestamp 与 device position;flags 中 discontinuity / timestamp error 进 diagnostics;buffer 不追求极限小,MVP 用 10–20 ms processing quantum,产品目标压到 5–10 ms WASAPI buffer。Microsoft capture shared event-driven sample 展示了枚举 capture devices、初始化 shared event-driven stream、设 event handle、由 audio engine signal 触发读数据。参考:<https://learn.microsoft.com/en-us/windows/win32/coreaudio/capturesharedeventdriven>

### 5.3 延迟与低延迟模式

Windows 10 之后 audio engine 支持更低 latency;默认应用一般走约 10 ms buffer,`IAudioClient3` 可查询 shared-mode engine period 并请求更小周期,但实际由驱动支持决定。

| 指标 | 可实现性 |
|---|---|
| <50 ms added latency | 可实现,尤其 10 ms frame + 2–3 个 buffer |
| <20 ms added latency | 可以追求,但依赖 endpoint period、虚拟设备、AEC 内部 buffering、resampler |
| 低于 10 ms | 不建议作为桌面外放 AEC 目标,会牺牲稳定性 |

AEC 通常以 10 ms 为外部处理单位,AEC3 内部再拆更小 block;更低 I/O period 可降排队延迟,但不能无限降低 AEC 收敛和延迟估计需求。参考:<https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio>

### 5.4 Chromium / OBS 作为 WASAPI 参考实现

**Chromium**(`audio/win/audio_low_latency_input_win.cc`)是低延迟实时音频模板:audio thread 中启动 capture client 和 loopback render client;用 MMCSS "Pro Audio" 并设 critical 优先级;处理 endpoint loopback render event;QPC timestamp 错误时构造 fake timestamp;处理 `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY`、`AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR`、silent flag;做 glitch 统计。参考:<https://chromium.googlesource.com/chromium/src/media/+/master/audio/win/audio_low_latency_input_win.cc>

**OBS**(`plugins/win-wasapi/`)对设备切换和真实桌面场景更接近本项目:`WASAPISource::ProcessCaptureData` 调 `GetNextPacketSize` / `GetBuffer` / `ReleaseBuffer`;处理 timestamp error、`AUDCLNT_E_DEVICE_INVALIDATED`;有 reconnect thread;有 loopback silent workaround;Windows 10 1703 后用 RT Work Queue。**OBS 是 GPLv2+,产品代码直接复制会触发 license 风险,只学习设计、不搬运文件(clean-room 重写)。** 参考:<https://github.com/obsproject/obs-studio/tree/master/plugins/win-wasapi>、<https://github.com/obsproject/obs-studio/blob/master/plugins/win-wasapi/win-wasapi.cpp>

**OBS 是本地参考集合里 WASAPI 采集鲁棒性最完整的来源,以下具体技巧应逐条 clean-room 复刻(2026-06 源码核实):**

| 技巧 | 源码证据(`obs-studio/plugins/win-wasapi/`) | 为什么必须做 |
|---|---|---|
| **七 Event 协作状态机 + 线程长期存活分两阶段** | `win-wasapi.cpp:217-225,1066-1180` | 先等 init 成功才进主循环,比「init 失败退线程再重启」代码量小一个数量级 |
| **SILENT 双路径(两个独立技巧缺一不可)** | capture 端指针重定向到自家 silence vec(`:1003-1012`)+ loopback 端用独立 `IAudioRenderClient` 预写一帧零样本防 reference 流冻结(`:724-764`) | 静音时 packet 标 SILENT,reference 流冻结会让 AEC 失去对齐基准 |
| **DEVICE_INVALIDATED 静默白名单** | `:974, :988`(`GetNextPacketSize` 与 `GetBuffer` 两处都查) | 拔出时驱动高频返回此错误,不加白名单会刷爆磁盘日志 |
| **`while(true)` 排空多 packet** | `:966-1043` | 单次 event 唤醒常对应多 packet 堆积,只取一个会累积延迟 |
| **`IMMNotificationClient` + role 区分** | `wasapi-notify.cpp:48-55` | mic 用 `eCommunications`,loopback 用 `eConsole` |
| **时间戳分两路** | `:1022-1027,1333,1339` | mic 默认 `useDeviceTiming=false`(便宜 USB mic 的 device ts 抖)用 `QPC_now − frames/sr`;loopback 用 device timing |

**OBS 没做、需我们自补的硬 gap(见 §13 风险):** `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY` 完整处理、默认设备 format/channel 运行时变化(OBS `wasapi-notify.cpp:46` 的 `OnPropertyValueChanged` 直接 `return S_OK`,改 stereo→7.1 会让 AEC 崩)、reconnect 指数退避(OBS 固定 3s)、IAudioClient3 5ms period underrun 恢复、Modern Standby 唤醒恢复。**MMCSS 取舍:** OBS 用 `"Audio"`(非 `"Pro Audio"`)+ 500ms 大缓冲偏稳定保守;我们 processing 线程上 `"Pro Audio"`、capture 线程 `"Audio"` 可能足够,别无脑全 Pro Audio。

**反例:`project-raven` 采集是「不要这么写」清单** —— 轮询式(`thread::sleep(20ms)` + `GetNextPacketSize`)、无 QPC/device-position 时间戳(`GetBuffer` 传 `None`,用 `SystemTime::now()` 当 PTS)、stereo 被平均 downmix 成 mono、无设备热插拔恢复、无 MMCSS、无 SILENT/DISCONTINUITY 处理。详见 §3.2 与 `reference_repos_exploration_report.md` §3.1。

OBS 的 `wasapi-output.c`(audio monitoring)展示如何用 WASAPI render client、resampler、delay buffer、padding 控制输出,对处理后音频写入 VB-Cable 或未来虚拟 endpoint 的用户态 writer 有参考价值(不是 AEC 架构)。参考:<https://github.com/obsproject/obs-studio/raw/refs/heads/master/libobs/audio-monitoring/win32/wasapi-output.c>

### 5.5 音频 I/O 库取舍

| 库 | 优点 | 缺点 | 结论 |
|---|---|---|---|
| raw WASAPI | 控制 timestamp、event、flags、device recovery | 开发量最大 | 成品推荐 |
| miniaudio | 单头文件,支持 playback/capture/duplex/WASAPI-only loopback callback | 深度 timestamp/drift 控制需确认 | MVP 可试 |
| NAudio | .NET 快速做 loopback demo | C++ 成品不适合,低延迟控制弱 | 只做原型 |
| PortAudio | 跨平台成熟 | WASAPI loopback/endpoint 细节不如 raw WASAPI 可控 | 不推荐主线 |
| RtAudio | C++ 简洁 | loopback / timestamp 细节需确认 | 不推荐主线 |
| JUCE | 设备和 UI 方便 | 增加依赖,WASAPI loopback/AEC reference 仍需补 | GUI 可用,核心 I/O 不首选 |

miniaudio 文档说明支持 playback、capture、full-duplex 和 WASAPI-only loopback,callback API,适合快速验证,但产品化仍需确认能否暴露足够 timestamp 和设备状态。参考:<https://miniaud.io/docs/manual/index.html>

---

## 6. 时钟漂移与延迟对齐

### 6.1 为什么独立设备时钟漂移会破坏 AEC

典型组合:`render device`(主板声卡 / HDMI / USB DAC / 显示器音频)与 `capture device`(USB 电容麦克风)通常不是同一 hardware clock domain。即使都标称 48 kHz,实际可能是:

```text
render: 48000.7 Hz
mic:    47999.3 Hz
```

一分钟后样本差可能达数十到上百 samples。AEC 自适应滤波器需要 render reference 与 mic echo 在时间上稳定对齐;reference 慢慢漂移会让 delay estimator 不断追赶,收敛和 residual suppression 变差。表现:

- 刚启动几秒/30 秒有效,随后回声逐渐漏出。
- 视频对白和音乐残留呈现「水声」「抽吸」「相位感」。
- 双讲时用户语音被压缩或变薄。
- reference ring buffer 水位单向增长或下降。
- delay estimator 不断跳;filter 不稳或 ERLE 下降。
- 固定 delay alignment 看似正确,但长时间稳定性差。

### 6.2 推荐策略:以 mic capture clock 为主时钟

处理线程每 10 ms 从 mic ring buffer 取一帧作 processing tick,按 mic timestamp 去 render reference ring buffer 找对应 reference;render reference 以**可变 fractional read position** 读取,必要时做 adaptive resampling。

```text
mic frame timestamp Tm
        ↓
target render timestamp Tr = Tm - estimated_echo_delay
        ↓
render ring buffer fractional read
        ↓
resample render reference to exactly 480 samples / 10 ms in mic clock
```

### 6.3 delay alignment 三层

| 层 | 方法 | 用途 |
|---|---|---|
| 粗对齐 | WASAPI QPC timestamp + fixed device latency estimate | 启动时快速进入可用状态 |
| 信号相关 | far-end 与 mic echo 的 cross-correlation / GCC-PHAT / AEC3 delay metric | 校正真实 acoustic delay |
| AEC 内部 | AEC3 delay estimator / matched filter | 细化 echo path delay |

AEC3 的 `echo_path_delay_estimator`、`matched_filter`、`delay_estimate`、`AecState` filter delay 更适合 AEC 内部状态,不应替代应用层 reference buffer 查找。

### 6.4 drift compensation 方案对比

| 方案 | 原理 | 优点 | 缺点 | 推荐 |
|---|---|---|---|---|
| 不处理 | 固定 delay | 简单 | 长时间失效 | 不推荐 |
| sample slip | 偶尔丢/插一个 sample,配 crossfade | 实现快 | 可能 click 或轻微相位突变 | MVP fallback |
| ring buffer level control | 让 reference 读指针跟踪目标水位 | 简单稳健 | 不能解决所有 fractional drift | MVP 可用 |
| adaptive resampling | 估计 ppm drift,动态调整 render reference 重采样比 | 最适合 AEC | 实现复杂 | 成品推荐 |
| timestamp-only | 完全相信 WASAPI timestamp | 实现简单 | timestamp error、driver bug 影响 | 只能辅助 |

成品建议:

```text
drift_ppm      = low_pass( observed_render_rate_vs_mic_rate )
resample_ratio = 1.0 + drift_ppm / 1_000_000
render_ref_out = fractional_resampler(render_ring, resample_ratio)
```

稳定策略:只对 render reference resample,不改 mic capture;drift estimator 做低通避免抖动;drift correction 每 1–5 秒缓慢更新;QPC timestamp error 或 discontinuity 出现时冻结 drift estimator;render ring buffer 目标水位保持 80–150 ms,外放 AEC 另设 300–500 ms history 供 delay search。

### 6.5 Windows 时钟与设备通知 API

`IAudioClock::GetPosition()` 可拿 device position 和相关 QPC 位置;设备变化和 session disconnect 通过 `IMMNotificationClient` / `IAudioSessionEvents` 监听。推荐每个 WASAPI packet 记录:QPC 时间、endpoint device frame position、packet frames、stream kind;处理线程以 mic clock 为主时钟;render reference 通过 timestamped ring buffer 按 mic frame 时间取样;引入动态 fractional resampler 微调 render reference 拉伸/压缩;delay estimator 给粗 offset,drift estimator 负责长期微调。参考:<https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclock-getposition>

### 6.6 参考项目与重要证据

| 项目 | 可参考点 |
|---|---|
| Chromium WASAPI | timestamp 修正、glitch 统计、实时线程(见 §5.4) |
| OBS WASAPI | device invalidated、reconnect、silent loopback workaround(见 §5.4) |
| PulseAudio / PipeWire echo-cancel | source/sink/reference graph 与 WebRTC wrapper(见 §9.1) |
| Synchronous Audio Router | 把 endpoint 同步到 ASIO physical interface 以缓解 drift,但项目状态不适合直接复用(见 §7.5) |
| Q-SYS AEC docs | room AEC tail length、ERLE、RES、reference routing 经验(见 §8) |

**关键证据(2026-06 本地源码核实,修正早期表述):** 早期本文写「PulseAudio 把 drift 留给应用层」,更准确的事实是:**PulseAudio 既没做应用层 fractional resample、也没做内部 `set_drift`,它只做 drop-based resync;真正的 ppm 级闭环 fractional resampling 在当前本地参考集合中未见任何 production 实现——这正是我们的技术创新空间。** 四处死代码/注释互证「drift 必须应用层做且无现成实现」:

| 证据 | 文件:行 | 说明 |
|---|---|---|
| PulseAudio `set_drift` 空 stub | `pulseaudio/src/modules/echo-cancel/webrtc.cc:369` | wrapper 的 drift 回调是空实现 |
| PulseAudio rate 计算被注释掉 | `pulseaudio/src/modules/echo-cancel/module-echo-cancel.c:369` | 应用层 rate 补偿代码被注释,实际走 drop-based resync |
| PipeWire 强制关闭 drift | `pipewire/spa/plugins/aec/aec-webrtc.cpp:268` | 演进后的 PipeWire 直接删掉 `set_drift`(对 PulseAudio 经验的投票) |
| AEC3 `clockdrift_detector` 只分类 | `aec3/clockdrift_detector.*` | 只输出 None/Probable/Verified,不做补偿 |

**但 PulseAudio 的 `calc_diff` 延迟对齐公式 + 工程参数表 + `apply_diff_time` 可 clean-room 复刻**(`pulseaudio/src/modules/echo-cancel/module-echo-cancel.c:138-178,299-335,678-704`):5ms 容差立即跳、1s watchdog、+10 帧 safety、resampler 群延迟要算进 stream_delay(`:426,464`)。**成品三段式 = PulseAudio 公式 + rubato 实现 + AEC3 `use_external_delay_estimator`;mic 主时钟铁律——fractional resampler 只放 RenderLoopback→Processing,mic 路径绝不重采样**(`:393` 只动 sink)。完整推导见 `reference_repos_exploration_report.md` §3.3 / 维度 2。参考:<https://github.com/pulseaudio/pulseaudio/blob/master/src/modules/echo-cancel/webrtc.cc>

---

## 7. 处理后音频送入 Discord / VRChat(虚拟麦克风)

Discord 和 VRChat 都允许用户在设置中选择录音输入设备,因此 AEC 工具最稳妥的输出形态是创建一个 Windows capture endpoint 让应用当成麦克风。参考:Discord <https://support.discord.com/hc/en-us/articles/214925018-Where-d-my-Audio-Input-go-Various-Voice-Issues>;VRChat <https://help.vrchat.com/hc/en-us/articles/360062659053-I-want-to-change-where-my-audio-is-coming-from>

### 7.1 方案总览

| 输出路径 | Discord/VRChat 识别为麦克风 | 开发难度 | 分发难度 | 延迟 | 结论 |
|---|---:|---:|---:|---:|---|
| VB-Cable | 是 | 极低 | 中,用户需装驱动 | 中低 | MVP 推荐 |
| Virtual Audio Cable | 是 | 极低 | 中,商业软件 | 低到中 | 测试/用户自备 |
| Voicemeeter | 是 | 低 | 中,配置复杂 | 中 | 不适合作主 UX |
| TVirtAudio SDK | 是 | 中 | 中,商业授权 | 文档称 low latency | 成品候选 |
| SysVAD / SimpleAudioSample 改造 | 是 | 高 | 高,需签名/安装器 | 可低 | 成品自研路线 |
| VirtualDrivers fork | 是 | 中高 | 高,签名/质量风险 | README 称 no-latency | 候选,需验证 |
| APO capture path | 可透明作用于麦克风 path | 很高 | 很高 | 低 | Win11 高级路线 |
| OBS Virtual Camera 类路线 | 音频不等同虚拟麦克风 | 中 | 中 | 中 | 不推荐主线 |
| VST + Equalizer APO | 可做处理但 reference/virtual mic 不完整 | 中 | 中 | 不确定 | 只做实验 |

### 7.2 VB-Cable / Virtual Audio Cable / Voicemeeter(MVP 最现实)

**VB-Cable** 是 Windows 虚拟音频设备:送到 "CABLE Input" 的音频出现在 "CABLE Output" 录音端;支持 XP 到 Win11、WDM/KS/MME/DX/WASAPI,安装需管理员权限和重启。MVP 连接方式:

```text
你的工具 WASAPI render → VB-Cable Input
Discord / VRChat microphone → VB-Cable Output
Windows 默认播放设备 → 仍然是用户的真实音箱
WASAPI loopback reference → 抓真实音箱对应 render endpoint
```

**注意:不要让用户把系统默认播放设备改成 VB-Cable**,否则音箱不响、reference 也会错。工具应显式打开 VB-Cable render endpoint 写入处理后的麦克风音频。参考:<https://vb-audio.com/Cable/>

**Virtual Audio Cable** 也可用,商业授权更明确(trial/lite/full,可建多 cable,支持较低 event period 和 clock/position registers),适合测试不建议要求普通用户购买。参考:<https://vac.muzychenko.net/en/download.htm>

**Voicemeeter** 是功能完整虚拟混音器,但对普通用户配置复杂,作 MVP 路由可用、成品不建议依赖(donationware,专业用需 license)。参考:<https://voicemeeter.com/>

| 项 | 判断 |
|---|---|
| 开发成本 | 最低 |
| 用户体验 | 中等,需装第三方驱动、在 Discord 里选设备 |
| 延迟 | 可接受,取决于 cable buffer |
| 分发 | 受第三方许可限制 |
| 是否适合成品 | 不理想,适合 MVP / 内测 |

### 7.3 自研虚拟音频设备(成品最正统)

Windows 官方 **SimpleAudioSample** 是更小、更纯的官方样例(virtual speaker + mic,基础 WaveRT 驱动结构);**SysVAD** 更大(虚拟 audio endpoint、WaveRT、APO、offload、keyword spotter)。MSVAD 已归档。⚠️ **2026-06 核实修正起点选择:虚拟麦克风的最佳起点是 `simpleaudiosample` 而非 `sysvad`**——前者更纯、注入点单一(核心 = `Source/Main/minwavertstream.cpp:1392-1421` 的 `WriteBytes`,样例只写正弦波,真实注入通道要自研),比 sysvad 易裁剪、比 Virtual-Audio-Driver 公开版完整。

**关键设计(从 Windows-driver-samples 核实,直接决定能否与 Discord/VRChat 共存):**
- **WaveRT 零拷贝**:`AllocatePagesForMdl + MapAllocatedPages(MmCached)` 让 driver 与 user-mode 共享同一物理页,**不需要 user-kernel ring 双拷贝**(`simpleaudiosample/.../minwavertstream.cpp:499-525`)。
- **声明 `AUDIO_EFFECT_TYPE_ACOUSTIC_ECHO_CANCELLATION`**:让 Discord/VRChat 看到「此 endpoint 已自带 AEC」从而关掉自己的 AEC,避免双 AEC 互打(`sysvad/EndpointsCommon/minwavert.cpp:1924-1959`)。
- **绝不声明 `MIC_ARRAY_GEOMETRY`**:否则 Windows 自动插入 Voice Capture DSP 与我们互打(反例见 §7.5 Virtual-Audio-Driver)。
- **`EndpointFormFactor=Headset` + friendly name 含 "Communications"** 让 Discord 优先列出;**三段式 componentized INF**(base+extension+APO,Win10 1809+ 强制)套 sysvad 模板,保留 `PETrust=true`/`DRMLevel=1300`;**QPC↔device position 用整数余数累加**(`hnsElapsedTimeCarryForward`)不用浮点乘除防长跑漂移(`minwavertstream.cpp:1302-1388`)。

成品可基于其做虚拟 capture endpoint:

```text
User-mode AEC service
    │ shared memory / named pipe / IOCTL / custom endpoint bridge
    ▼
Kernel streaming / WaveRT virtual capture endpoint
    ▼
Windows Audio Engine
    ▼
Discord / VRChat
```

难点:内核驱动/audio miniport 质量要求高;驱动断流时输出 silence 而非阻塞 app capture;采样率/channel format negotiation 需完整处理。

**驱动签名**(分发最大成本):Windows 10 1607 之后新 kernel-mode driver 通常需通过 Dev Portal 签名;需 EV code signing certificate、Microsoft Hardware Dev Center dashboard、attestation signing 或 HLK/WHQL、ADK、CAB、Partner Center 提交流程、安装器/UAC/卸载/升级、Windows 版本兼容测试。参考:SysVAD <https://github.com/microsoft/Windows-driver-samples/blob/main/audio/sysvad/README.md>;SimpleAudioSample <https://github.com/microsoft/Windows-driver-samples/blob/main/audio/simpleaudiosample/README.md>;驱动签名策略 <https://learn.microsoft.com/en-us/windows-hardware/drivers/install/kernel-mode-code-signing-policy--windows-vista-and-later->;attestation <https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-attestation>

| 项 | 判断 |
|---|---|
| 开发成本 | 高 |
| 维护成本 | 高 |
| 用户体验 | 最好,一次安装后就是普通麦克风 |
| 延迟 | 可做到最好 |
| 分发 | 驱动签名 / 安装是主要门槛 |
| 是否适合成品 | 是,但应在 AEC 效果验证后再做 |

### 7.4 Windows APO / CAPX(系统级注入)

Windows APO 是 in-process COM 实时音频处理对象,可作 SFX/MFX/EFX 插入 endpoint audio graph。实现需 `IAudioProcessingObjectRT::APOProcess()`,受实时约束:不能阻塞、不能分页内存、不在实时路径做危险调用、不引入显著延迟。微软文档把 AEC、AGC 等列为 APO 可实现的系统效果,custom APO 通过 driver package / INF 安装,运行在 real-time audio processing path。

**Windows 11 AEC APO / CAPX 框架**更适合 AEC:AEC APO 可作 capture MFX,声明 `IApoAcousticEchoCancellation`,接收 render reference 作为 auxiliary input;render device 变化时 reference stream 会切换并含 format 和 timestamp。微软文档明确 Windows 10 时代 APO 只有单输入/单输出,厂商通常靠私有驱动通道或在 `audiodg.exe` 内用 WASAPI loopback,存在 deadlock、power、音量通知风险;Windows 11 新框架正是为 AEC reference stream 设计。

**⚠️ 2026-06 核实新增:本地 `Windows-driver-samples/audio/sysvad/APO/AecApo` 就是这条路线的官方模板,应升级为 Win11 主路径之一(与用户态虚拟麦克风并行),而非仅「高级路线」。** 走 APO 时,**OS audio engine 会把 render endpoint 的同步 loopback 作为 aux input 自动喂给 APO**(`AecApo/AecApoMfx.cpp:610-671,720-738`),从而把一部分延迟估计/对齐/设备恢复问题交给系统接口处理——这是被低估的价值点。**但它只是 API 模板不是可落地实现**:算法体为空、16k mono 硬编码、仅 COMMUNICATIONS 模式、audiodg 沙箱约束(无锁无分页),且 `AcceptInput` 样例里有明显 `inputId`/`dwInputId` typo。实时 AEC 算法与注入策略仍需自研。

现实限制:更偏 OEM / IHV / 驱动包集成;Win10 不覆盖;需 INF / APO / 驱动部署;绑定具体 capture endpoint 或自研虚拟 endpoint 更现实——不是「给任意 USB 麦克风装一个全局插件」。

| 项 | 判断 |
|---|---|
| 技术优雅度 / Win11 支持 | 高 / 好 |
| Win10 支持 | 差 |
| 任意 USB mic 兼容 | 不理想 |
| 成品主路径 | 不建议作为第一版 |
| 后续高级路线 | 值得探索 |

参考:APO 架构 <https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/audio-processing-object-architecture>;APO 实现 <https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/implementing-audio-processing-objects>;Windows 11 APO APIs <https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/windows-11-apis-for-audio-processing-objects>

### 7.5 VirtualDrivers / Synchronous Audio Router / 商业 SDK

- **VirtualDrivers / Virtual-Audio-Driver(降权至「仅 INF/工程模板」):** 派生自 SysVAD/SimpleAudioSample(MIT + MS-PL),README 称支持 Windows 10 1903+/11、x64/ARM64,latest signed release 2025-07-14。⚠️ **2026-06 源码核实推翻早期 3/5 判断**:公开版 mic 数据路径 `WriteBytes` 实为 `RtlZeroMemory`(`Virtual-Audio-Driver/Source/Main/minwavertstream.cpp:1392-1421`)——**虚拟麦克风永远输出静音,实际可用度为 0**;README 所称「custom builds 可用 named pipes/shared memory」经全仓 grep **证伪(属付费定制内容)**。**结论:绝对避免 fork 公开版做业务**,只剩 INF 三段式 componentized 模板、ARM64 工程、build 链路的模板价值。⚠️ 另注:`Source/Filters/micarraytopo.cpp:373-419` 声明了 `MIC_ARRAY_GEOMETRY`,会让 Windows 自动插入 Voice Capture DSP(AEC/NS/BF)与我们互打——**反例,我们的驱动绝不能这么声明**。参考:<https://github.com/VirtualDrivers/Virtual-Audio-Driver>
- **Synchronous Audio Router(1–2/5):** 开源 Windows 音频驱动,目标把应用音频路由到 DAW/ASIO 并提供同步虚拟音频流(用虚拟 driver、WaveRT、动态 endpoint,同步以缓解 drift/underrun);但依赖 ASIO/DAW,README 说明当前 Windows 10 Secure Boot 不支持,GPL。适合研究虚拟音频驱动和同步思路,**不适合作 fork 基础或普通用户成品**。参考:<https://github.com/eiz/SynchronousAudioRouter>
- **TVirtAudio SDK(4/5 成品候选):** 商业虚拟音频 SDK,明确支持创建 virtual input device,通过 private API 管理设备和传输音频。可缩短虚拟驱动开发与签名风险,但要确认授权、成本、低延迟、用户态 feed API。参考:<https://vac.muzychenko.net/en/sdk.htm>

### 7.6 应用级方案

Discord / VRChat 是既有应用,除非提供「把外部 PCM stream 注入为麦克风」的官方接口,否则通用工具必须走 Windows capture endpoint。Discord SDK 适合游戏/应用集成,不适合替换用户桌面 Discord 的麦克风输入。OBS 没有通用 AEC,也不能直接给 Discord 提供虚拟麦克风;其 `wasapi-output.c` 对 `VirtualMicWriter`/监控输出有参考价值(见 §5.4)。

### 7.7 硬件 AEC(基线对照)

常见形态:USB speakerphone、会议麦克风阵列、DSP mixer/conferencing processor、带 DSP AEC 的 USB audio interface、房间音频系统(如 Q-SYS)。Q-SYS AEC 文档展示成熟硬件 AEC 关注的变量:reference signal、tail length、ERLE、residual echo suppression、noise reduction、room impulse response;给出 100/200/300/400 ms tail length 级别,ERLE >20 dB 通常较好、<10 dB 可能说明 tail length 或回声路径问题。Yamaha YVC-200 这类 USB/Bluetooth speakerphone 含 adaptive echo canceller、full-duplex、AGC、NR。

硬件 AEC 是很好的基线,但通常要求声音从同一硬件系统播放/采集;对「两只已有有源音箱 + USB 电容麦克风 + 任意 Discord/VRChat」的软件工具目标,不是通用替代方案。参考:Q-SYS <https://q-syshelp.qsc.com/Content/Schematic_Library/acoustic_echo_canceler_simd.htm>;Yamaha YVC-200 <https://sg.yamaha.com/en/business/audio/products/speakerphones/yvc-200/>

---

## 8. 外放场景特殊考量

### 8.1 外放 AEC 与耳机漏音 AEC 的差异

| 因素 | 耳机漏音 | 外放音箱 |
|---|---|---|
| echo path | 短、能量低 | 长、能量高 |
| 房间反射 | 弱 | 强,多路径 |
| 非线性 | 较少 | 音箱失真、桌面反射、麦克风饱和 |
| 双讲 | 相对可控 | 用户讲话 + 视频人声同时存在很常见 |
| 移动影响 | 小 | 麦克风/音箱/用户移动会改变路径 |
| tail length | 短 | 可能需要 200–400 ms 甚至更长 |

### 8.2 Echo tail 明显更长

两只桌面音箱带来:直达声、桌面反射、墙面/显示器反射、低频驻波、扬声器非线性失真。WebRTC AEC3 默认 filter length 约 52 ms,需在外放场景做更长 tail 测试;SpeexDSP 文档建议 100–500 ms,更贴近开放房间工程直觉(但算法上限较低)。硬件 AEC 文档把 room impulse response 与 tail length 直接关联,常见 100–400 ms。

### 8.3 Stereo reference 不应随便 downmix

两只音箱到麦克风是两个独立 echo path:

```text
left speaker  → room path hL(t) → mic
right speaker → room path hR(t) → mic
mic echo = left_ref * hL + right_ref * hR
```

若过早 downmix `mono_ref = (L + R) / 2`,当 L/R 内容/相位/空间定位不同时:AEC filter 需用一个 reference 解释两个路径,收敛变慢;音乐和游戏 stereo 残留更多;相位反相关内容可能被错误建模。建议:AEC3 成品 wrapper 保留 stereo render pipeline;SpeexDSP MVP 同时测 mono downmix 与 `speex_echo_state_init_mc(... nb_speakers=2)`;若 AEC3 多通道成本过高,MVP 可先 mono,但实验必须比较 stereo vs mono。(neural 模型的 stereo 处理三方案见 §3.3 工程风险。)

### 8.4 非线性回声无法靠线性滤波完全消除

音箱音量过大、低频失真、桌面共振、麦克风削波都会让「render reference → mic echo」不再线性。线性自适应滤波会留残余,后级 residual echo suppressor 必须介入,但会增加对近端人声的损伤风险。成品需要:mic clipping detector、render gain detector、residual echo stats、speaker volume warning、「保守 / 强力」两档 suppressor preset。

### 8.5 麦克风位置变化会导致重新收敛

麦克风被移动、音箱角度改变、用户身体遮挡都会改变 echo path。AEC 应能检测 echo path change 并 reset / reconverge。WebRTC AEC3 处理路径中有 echo path variability、filter reset / reconfiguration 逻辑;产品层应暴露状态,例如「正在重新校准」。

### 8.6 房间混响与 dereverb

AEC 可处理线性 echo path 一部分,残留混响和非线性失真通常需 residual echo suppression。强 dereverb 太激进会让用户语音变薄、金属感增强。分层:

| 阶段 | 处理 |
|---|---|
| MVP | AEC + HPF + limiter;可选轻量 residual suppression |
| 稳定版 | AEC3 residual echo suppression + NS/AGC 可调 |
| 后续优化 | dereverb 作为可选后处理,不作为核心依赖 |

### 8.7 双讲场景

典型困难:用户说话同时视频里也有人说话;用户说话同时 Discord 对方也说话;游戏 NPC 台词 + 用户吐槽;音乐人声 + 用户说话。失败表现:用户声音被压低或断续、出现水声/抽吸/音色变薄、far-end 人声残留、双讲结束后 filter 需重新收敛。WebRTC AEC3 的 near-end detector、dominant near-end logic、residual echo estimator、suppression controller 比 SpeexDSP 更适合作成品核心;SpeexDSP 明确以 variable learning rate 处理 double-talk,无独立 DTD。

### 8.8 音量与物理布局(产品体验层)

软件 AEC 不是魔法。外放场景 UI 建议实时提示:音箱不要正对麦克风;麦克风增益不要过高;避免麦克风 clipping;音箱与麦克风尽量拉开距离;使用心形指向麦并让背面对着音箱;首次校准播放 test chirp / MLS / pink noise,估计 delay、ERLE、clipping、room tail。这些对 AEC 成败影响很大。

### 8.9 近年 neural AEC 可关注,但不适合第一主线

近年 AEC Challenge 使用大规模真实/合成数据评测复杂真实设备和环境中的 echo removal,说明神经 AEC 是活跃方向。但对 Windows 实时桌面工具有产品风险:需模型推理 runtime;CPU/GPU 占用不可控;double-talk 下可能产生语音伪影;数据集与用户实际音箱/麦克风差异大;与低延迟需求冲突。推荐把 neural residual suppressor 放后续实验,不作为 MVP 核心(neural 模型详细评估见 §3.3)。参考:AEC Challenge paper <https://arxiv.org/html/2309.12553v1>

---

## 9. 推荐架构方案

### 9.1 架构样板:PipeWire / PulseAudio echo-cancel

PipeWire `module-echo-cancel` 证明了推荐架构的正确形态:创建虚拟 echo-cancel capture source 和 playback sink,形成「四流」结构——麦克风输入、播放参考、处理后 source、实际 speaker playback(即 `mic → cancel → app`、`app → cancel → speaker`)。可选 `aec/libspa-aec-webrtc` 后端,设 `node.latency=1024/48000` 等低延迟参数。源码是典型实时音频图:固定采样率、最大缓冲时长、录音 ring buffer、播放 ring buffer、延迟播放 ring buffer、输出 ring buffer,运行路径调用 `spa_audio_aec_run()`。

PulseAudio WebRTC wrapper 默认 block size 10 ms,支持 HPF、NS、transient suppression、AGC、comfort noise;创建 APM,在 `pa_webrtc_ec_play` 调 `ProcessReverseStream`、在 `pa_webrtc_ec_record` 调 `ProcessStream`。

**结论:Windows 上要用 WASAPI 和虚拟音频驱动复刻这个模式——虚拟输入/输出节点 + reference/capture 双流 + ring buffer + AEC 处理线程。** 直接可用度 2/5,架构参考价值 5/5。参考:PipeWire echo-cancel <https://docs.pipewire.org/page_module_echo_cancel.html>;PipeWire `module-echo-cancel.c` <https://raw.githubusercontent.com/PipeWire/pipewire/master/src/modules/module-echo-cancel.c>;PipeWire AEC meson <https://github.com/PipeWire/pipewire/blob/master/spa/plugins/aec/meson.build>;PulseAudio wrapper <https://github.com/pulseaudio/pulseaudio/blob/master/src/modules/echo-cancel/webrtc.cc>;PulseAudio 改进博客 <https://arunraghavan.net/2016/05/improvements-to-pulseaudios-echo-cancellation/>

### 9.2 总体架构与模块职责

```text
Render Reference Capture → Render Reference Ring Buffer → Delay Alignment → Drift Compensation
Microphone Capture ───────────────────────────────────→ AEC Engine Wrapper → Post Processing
                                                          → Virtual Microphone Output → Discord / VRChat
```

| 模块 | 职责 |
|---|---|
| Render Reference Capture | WASAPI loopback 捕获默认/指定播放设备,保留 stereo 48 kHz,记录 QPC timestamp |
| Microphone Capture | WASAPI capture 捕获 USB mic,输出 48 kHz mono/stereo,记录 QPC timestamp |
| Delay Alignment | 根据 timestamp 与信号相关估计 render-to-mic delay |
| Drift Compensation | 估计 render/mic ppm drift,对 render reference 做 adaptive resampling |
| AEC Engine Wrapper | 封装 WebRTC AEC3 / SpeexDSP / SDK,引擎替换统一接口 |
| Post Processing | HPF、limiter、可选 NS/AGC/residual suppression/dereverb |
| Virtual Microphone Output | 把清理后的 48 kHz mono 写入 VB-Cable 或虚拟驱动 |
| Device Manager | 枚举设备、监听 default device、断连恢复 |
| Diagnostics / Calibration | delay、drift、ERLE、glitch、buffer level、AEC 收敛状态、日志与测试音 |

### 9.3 信号流设计

```text
┌─────────────────────────────────────────────────────────────────┐
│ Windows 默认播放设备 / 用户选择的音箱                            │
│ 48 kHz stereo float32, shared mode, event driven                 │
└───────────────┬─────────────────────────────────────────────────┘
                │ WASAPI loopback, AUDCLNT_STREAMFLAGS_LOOPBACK
                ▼
┌──────────────────────────┐
│ RenderReferenceCapture   │ 5–10 ms packet / QPC + device position
└───────────────┬──────────┘
                ▼
┌──────────────────────────┐
│ TimestampedRenderRing    │ 1–2 s capacity / stereo preserved
└───────────────┬──────────┘
                ▼
┌──────────────────────────┐
│ DelayDriftAligner        │ offset search / async resampler / stereo frame extraction
└───────────────┬──────────┘
                │ render 10 ms
                ▼
          ┌──────────────┐
          │ WebRTC AEC3  │◄─────────────────────────────┐
          │ AnalyzeRender│                              │
          │ ProcessCapture                              │
          └──────┬───────┘                              │
                 │ clean mic                             │
                 ▼                                       │
┌──────────────────────────┐                              │
│ PostProcessor            │ HPF / optional NS / AGC / limiter / level meter
└───────────────┬──────────┘                              │
                ▼                                         │
┌──────────────────────────┐                              │
│ OutputRing               │ 48 kHz mono, 10 ms          │
└───────────────┬──────────┘                              │
                ▼                                         │
┌──────────────────────────┐                              │
│ VirtualMicWriter         │ VB-Cable MVP / SysVAD product
└───────────────┬──────────┘                              │
                ▼                                         │
┌──────────────────────────┐                              │
│ Discord / VRChat         │ input = AEC Microphone      │
└──────────────────────────┘                              │
                                                          │
┌──────────────────────────┐                              │
│ USB Microphone Capture   │──────────────────────────────┘
│ 48 kHz mono float32 / 5–10 ms packet / QPC + device position
└──────────────────────────┘
```

#### 格式与采样率建议

| 环节 | 采样率 | 声道 | 格式 | 帧长 | 预期延迟 |
|---|---:|---:|---|---:|---:|
| WASAPI loopback | 48 kHz | 2 | float32(device mix) | 5–10 ms | 5–20 ms |
| USB mic capture | 48 kHz | 1 | float32 | 5–10 ms | 5–20 ms |
| Render ring | 48 kHz | 2 | float32 | packetized | 1000–2000 ms capacity(非实际延迟) |
| AEC3 | 48 kHz | render 2 / capture 1 | float32 | 10 ms API frame | 算法处理 <10 ms |
| SpeexDSP | 48 kHz | mono 或 multichannel | int16/float wrapper | 10 ms | — |
| Postprocess | 48 kHz | 1 | float32 | 10 ms | <1–3 ms |
| Virtual mic output | 48 kHz | 1 | float32 / int16 | 10 ms | 10–30 ms |

#### 重采样位置

| 情况 | 处理 |
|---|---|
| render endpoint 不是 48 kHz | loopback 后重采样到 48 kHz |
| mic 不是 48 kHz | capture 后重采样到 48 kHz |
| render/mic 长期漂移 | render reference adaptive resampling 到 mic clock |
| virtual mic endpoint 要求 44.1 kHz | post processing 后重采样输出;不改 AEC 内部 48 kHz |

#### 延迟预算

```text
Mic capture buffer     5–10 ms
Processing queue       0–10 ms
AEC / postprocess      <5 ms on normal desktop CPU
Virtual mic buffer     10–20 ms
Total                  ~25–45 ms(理想)
```

| 环节 | MVP | 成品目标 |
|---|---:|---:|
| WASAPI loopback/capture packetization | 10–20 ms | 5–10 ms |
| alignment + processing frame | 10 ms | 10 ms |
| virtual mic output buffer | 20–40 ms | 10–20 ms |
| app input buffer | 不可控,约 10–30 ms | 不可控 |
| 总体 | 50–100 ms 常见 | 30–60 ms 较现实 |

用户给出的理想 <20 ms 很激进。对 Windows shared-mode + virtual mic + external AEC,**<50 ms 是更现实的成品目标**;<20 ms 只有在 endpoint buffer、虚拟驱动、app input 都配合时才可能接近。

### 9.4 关键数据结构、模块接口与引擎抽象

```cpp
enum class SampleType { Float32, Int16 };

struct AudioFormat {
    uint32_t sample_rate;
    uint16_t channels;
    SampleType sample_type;
    uint32_t frames_per_buffer;
};

struct AudioBlock {        // 只读
    AudioFormat format;
    uint64_t qpc_ns;
    uint64_t device_frame_position;
    uint32_t frames;
    const float* interleaved;
};

struct MutableAudioBlock { // 可写
    AudioFormat format;
    uint64_t qpc_ns;
    uint64_t device_frame_position;
    uint32_t frames;
    float* interleaved;
};

struct AecStats {
    float erl_db;
    float erle_db;
    float residual_echo_likelihood;
    int estimated_delay_ms;
    float drift_ppm;
    bool filter_diverged;
    bool echo_path_changed;
    bool mic_clipped;
};

struct AecConfig {
    uint32_t sample_rate = 48000;
    uint16_t render_channels = 2;
    uint16_t capture_channels = 1;
    uint32_t frame_ms = 10;
    uint32_t initial_delay_ms = 80;
    uint32_t max_tail_ms = 256;
    bool enable_high_pass_filter = true;
    bool enable_agc = false;
    bool enable_noise_suppression = false;
};

class AudioCapture {
public:
    virtual ~AudioCapture() = default;
    virtual void Start(const std::wstring& device_id, const AudioFormat& desired_format) = 0;
    virtual bool Read(AudioBlock& out, uint32_t timeout_ms) = 0;
    virtual void Stop() = 0;
};

class RenderReferenceBuffer {
public:
    virtual ~RenderReferenceBuffer() = default;
    virtual void Push(const AudioBlock& block) = 0;
    virtual bool ReadAligned(uint64_t mic_qpc_ns, int delay_ms,
                             float* output_interleaved, uint32_t frames, uint16_t channels) = 0;
    virtual void Reset() = 0;
};

class DriftCompensator {
public:
    virtual ~DriftCompensator() = default;
    virtual double EstimatePpm(uint64_t render_device_frame, uint64_t render_qpc_ns,
                               uint64_t mic_device_frame, uint64_t mic_qpc_ns) = 0;
    virtual void ResampleRender(const float* input, uint32_t input_frames, uint16_t channels,
                                double ratio, float* output, uint32_t output_frames) = 0;
};

class AecProcessor {
public:
    virtual ~AecProcessor() = default;
    virtual void Configure(const AecConfig& config) = 0;
    virtual void AnalyzeRender(const float* render_interleaved, uint32_t frames, uint16_t channels) = 0;
    virtual void ProcessCapture(const float* mic_mono, float* clean_mono, uint32_t frames) = 0;
    virtual void SetStreamDelayMs(int delay_ms) = 0;
    virtual AecStats GetStats() const = 0;
    virtual void Reset() = 0;
};

class AudioOutput {
public:
    virtual ~AudioOutput() = default;
    virtual void Start(const std::wstring& output_device_id, const AudioFormat& format) = 0;
    virtual bool Write(const float* mono, uint32_t frames, uint64_t qpc_ns) = 0;
    virtual void Stop() = 0;
};

class Pipeline {
public:
    void Start(const PipelineConfig& config);
    void Stop();
    void OnDefaultRenderDeviceChanged(const std::wstring& device_id);
    void OnCaptureDeviceChanged(const std::wstring& device_id);
    AecStats GetStats() const;
};
```

**可插拔引擎后端**(用于在 WebRTC AEC3 / SpeexDSP / Maxine 之间运行时切换,见 §3.4.6):

```cpp
class IAecEngine {
public:
    virtual void Configure(const AecConfig&) = 0;
    virtual void Process(const float* near_end_mono_48k,
                         const float* far_end_mono_48k,
                         float* clean_mono_48k, int frames) = 0;
    virtual AecStats GetStats() const = 0;
    virtual void Reset() = 0;
};
// 实现:WebRtcAec3Engine / SpeexAecEngine / NvidiaMaxineAecEngine
```

统一 wrapper 接口边界:`Initialize(sample_rate=48000, render_channels=1/2, capture_channels=1, frame_size=480, config)` / `AnalyzeRender(render_frame_10ms, timestamp, drift_state)` / `ProcessCapture(capture_frame_10ms, timestamp) -> cleaned_frame_10ms` / `GetMetrics() -> ERLE, delay_ms, residual_echo, convergence, saturation, drift_ppm` / `Reset(reason)`。不同引擎映射:

| 引擎 | Render input | Capture input | Output | 注意 |
|---|---|---|---|---|
| WebRTC AEC3/APM | `ProcessReverseStream` / `AnalyzeRender` | `ProcessStream` / `ProcessCapture` | float | 推荐保留 APM metrics |
| SpeexDSP | `speex_echo_cancellation` 或 playback/capture API | same | int16/float wrapper | 需自行 residual suppression |
| NVIDIA NvAFX | far-end `x` | near-end `y` | float32 | GPU/SDK 状态管理 |
| Superpowered/Switchboard | SDK node input | SDK node input | SDK node output | API 需验证 |

可替换模块边界:

| 替换点 | 默认 | 可替换 | 数据边界 | 代价 |
|---|---|---|---|---|
| AEC | WebRTC AEC3 | SpeexDSP / NVIDIA / commercial SDK | 48 kHz 10 ms float32 mono/stereo | wrapper 复杂度 |
| Audio I/O | raw WASAPI | miniaudio / PortAudio / NAudio | timestamped packet → 10 ms frames | timestamp/control 可能下降 |
| Output | VB-Cable MVP | 自研 driver / TVirtAudio / APO | 48 kHz mono stream | 分发/签名 |
| Resampler | 自研 high-quality fractional | Speex resampler / libsamplerate / WebRTC | 48 kHz float32 | CPU/latency |
| Post processing | AEC3 built-in HPF/NS/AGC optional | RNNoise / NvAFX NS / dereverb | mono 48 kHz | 语音失真风险 |
| Diagnostics | 自研 UI | ETW/perfetto/log file | metrics snapshots | 工程量 |

### 9.5 线程模型

```text
┌────────────────────────────┐
│ Control / UI Thread        │ device enumeration / IMMNotificationClient / config / stream restart
└────────────┬───────────────┘ atomics / commands
             ▼
┌────────────────────────────┐
│ RenderLoopbackThread (MMCSS)│ IAudioCaptureClient + LOOPBACK / timestamp packet / push SPSC ring
└────────────┬───────────────┘ lock-free SPSC
             ▼
┌────────────────────────────┐
│ ProcessingThread (MMCSS)    │ wait mic frame / pull aligned render / drift resample / AEC3 / postprocess / push output ring
└────────────▲───────────────┘ lock-free SPSC
             │
┌────────────┴───────────────┐
│ MicCaptureThread (MMCSS)    │ IAudioCaptureClient / timestamp packet / push mic SPSC ring
└────────────┬───────────────┘
             ▼
┌────────────────────────────┐
│ VirtualMicThread (MMCSS)    │ WASAPI render to cable(MVP)/ driver bridge(product)/ underrun fill silence|CNG
└────────────────────────────┘
```

| 线程 | 优先级 | 可阻塞 | 动态分配 | 通信 | 说明 |
|---|---|---:|---:|---|---|
| Render loopback capture | MMCSS Pro Audio | 否 | 避免 | lock-free ring | 读 WASAPI packet,写 reference buffer |
| Microphone capture | MMCSS Pro Audio | 否 | 避免 | lock-free ring | 读 mic packet,写 capture buffer |
| Audio processing | MMCSS Pro Audio | 否 | 禁止热路径分配 | ring + atomic state | 10 ms tick,delay/drift/AEC/post |
| Virtual mic output | MMCSS Audio/Pro Audio | 否 | 避免 | output ring | 写 VB-Cable/render endpoint 或驱动 feed |
| Device watcher | 普通 | 可 | 可 | message queue | IMMNotificationClient / device change |
| UI / diagnostics | 普通 | 可 | 可 | snapshot queue | 显示 delay/drift/ERLE/glitch |
| Logger | 低 | 可 | 可 | lock-free log queue | 避免实时线程写文件 |

**实时线程规则:** WASAPI callback/packet loop 不运行 AEC;不在音频线程分配内存;不在音频线程写日志文件;不拿 UI mutex;不调用可能阻塞的 COM/UI/logging;ring buffer 固定容量预分配;参数更新用 atomic snapshot 或 double-buffered config;device reset / reconnect 放到 control thread,新 stream ready 后 atomic swap;underrun 时输出低电平舒适噪声或静音;overrun 时丢弃最老帧,不能阻塞 capture。

### 9.6 Buffer 与延迟策略

| Buffer | MVP | 成品目标 | 说明 |
|---|---:|---:|---|
| render history | 1000 ms | 500–1000 ms | 用于 delay search 和 drift recovery |
| mic buffer | 100–200 ms | 50–100 ms | 防 packet 抖动 |
| processing frame | 10 ms | 10 ms | 匹配 AEC3/Speex |
| virtual mic output | 30–50 ms | 10–20 ms | MVP 稳定优先 |
| delay search range | 0–500 ms | 0–800 ms 可调 | 外放房间可更长 |
| target render read headroom | 80–150 ms | 50–100 ms | 防 underrun |

### 9.7 设备切换与异常恢复

| 异常 | 行为 |
|---|---|
| 默认播放设备切换 | 停止 loopback stream,重建 reference buffer,AEC reset 或 soft reset |
| 麦克风断开 | 输出 silence,UI 提示,等待重连 |
| render stream silent | 给 AEC 提供 silence reference,不要停 processing tick |
| timestamp error | 标记诊断,短期用 sample counter fallback |
| data discontinuity | 丢弃当前 alignment,重新估计 delay |
| virtual cable 未安装 | UI 显示输出不可用,允许选物理 monitor output 做离线监听 |
| Discord 已打开 | virtual mic endpoint 变化时可能需用户在 Discord 重新选设备;软件端记录状态 |
| sample rate 改变 | 重建 resampler 和 AEC state |

---

## 10. 技术路线对比

### 10.1 路线 A:SpeexDSP 快赢路线(MVP)

端到端:`WASAPI loopback stereo → downmix 或 Speex MC reference + USB mic capture → SpeexDSP AEC → (可选 Speex preprocessor residual echo suppression) → WASAPI render to VB-Cable Input → Discord 选 VB-Cable Output`。构建见 §4.2(vcpkg),参数起点 `sample_rate=48000 / frame_size=480 / filter_length=14400 / nb_mic=1 / nb_speakers=2`。

- **优势:** 几天内可做可听原型;vcpkg 装包;C API 稳定;filter length 可直接设 100–500 ms;适合验证 WASAPI、虚拟 cable、整体延迟、Discord 兼容性。
- **劣势:** 无显式 double-talk detector;residual echo suppression 较弱;对 loud speaker / 男性对白 / 音乐 / 人声混合上限有限;stereo 处理虽有 API 但需大量测试;很可能在「我说话 + 对方声音外放」时吞字或残余 echo。

| 维度 | 评分 |
|---|---:|
| 开发成本 | 5 / 5 |
| 维护成本 | 4 / 5 |
| 效果上限 | 2.5 / 5 |
| 用户体验 | 2.5 / 5(取决于 VB-Cable) |
| 适合作为 | MVP / 声学可行性验证 |

### 10.2 路线 B:WebRTC AEC3 深度路线(成品核心)

端到端:`cpal reference 48 kHz stereo/mono → timestamp/queue diagnostics + drift aligner → AEC3 AnalyzeRender() + USB mic 48 kHz mono → AEC3 ProcessCapture() → Postprocess(limiter / optional NS / AGC) → VB-Cable / BlackHole / 外部虚拟设备 → Discord / VRChat`。构建见 §4.1;工程上建议封装自己的小 C API(`my_aec_create` 等,见 §4.1)。

- **增量收益(相对 SpeexDSP):** 更强延迟处理;更强 double-talk 稳定性;residual echo estimator + suppressor + comfort noise;CPU SIMD 优化;多通道检测;更接近现代浏览器/会议软件的生产 AEC 链路。
- **劣势:** 构建链重;WebRTC 内部 API 可能随版本变;官方 AEC3 target 不是给第三方稳定 ABI 用的小库;外放场景仍要自己解决 delay/drift/stereo/tail;输出到 Discord 仍需虚拟设备。

| 维度 | 评分 |
|---|---:|
| 开发成本 | 3 / 5 |
| 维护成本 | 2.5 / 5 |
| 效果上限 | 4.5 / 5 |
| 用户体验 | 取决于虚拟设备 |
| 适合作为 | 产品核心 |

构建方式建议:**保守路线**用 depot_tools 拉 WebRTC、GN/Ninja 构建静态库、封装稳定 C++ wrapper;**工程便利路线**评估 CMake wrapper(get-wrecked)并锁定 WebRTC milestone;**Rust 实验路线**用 Sonora AEC3 + FFI,需额外验证音质一致性和 Windows realtime 表现。

### 10.3 路线 C:推荐成品路线

```text
User-mode Windows Audio Engine
  - WASAPI loopback capture / WASAPI mic capture
  - timestamp / drift / delay aligner
  - WebRTC AEC3 / postprocess / metrics / control UI
MVP Output:     VB-Cable / VAC
Product Output: 基于 SysVAD 的自研虚拟 capture endpoint + signed installer,Discord/VRChat 选 "AEC Microphone"
Optional Win11 Native Mode: AEC APO / CAPX attached 到自研虚拟 capture endpoint(非 Win10 baseline 必需)
```

**为什么这样选:** ① 把最大未知项分离(AEC 效果、Windows capture、虚拟设备是三个不同风险;MVP 用 VB-Cable 避开驱动签名,快速验证声学效果);② WebRTC AEC3 是开源可审计引擎里综合最强;③ 成品必须有自己的虚拟麦克风(做到「装一次就忘」);④ APO 方案只适合作 Win11/驱动包增强路线。

| 维度 | 评分 |
|---|---:|
| 开发成本 | 3 / 5 MVP,1.5 / 5 成品驱动 |
| 维护成本 | 2.5 / 5 |
| 效果上限 | 4.5 / 5 |
| 用户体验上限 | 5 / 5 |
| 商业可控性 | 4 / 5 |
| 推荐程度 | 最高 |

### 10.4 Neural 路线(可选增强)

```text
Phase N1 离线评估:LocalVQE v1.2/v1.3 + DTLN-aec 128/256/512 + Microsoft DEC baseline + WebRTC AEC3 baseline
Phase N2 实时原型:WASAPI loopback + mic → delay/drift aligner → LocalVQE 或 DTLN-aec → VB-Cable output
Phase N3 产品架构:WebRTC AEC3 as primary AEC + LocalVQE as optional neural residual echo suppressor + 自研 virtual mic
```

**不建议**直接用 LLaSE-G1 / diffusion AEC / TSPNN 做实时 Windows 工具核心——离「低延迟、可分发、可维护、reference-based desktop AEC」太远(详见 §3.3)。

### 10.5 路线评分汇总

分数越高越友好(开发成本/维护成本/延迟风险/分发难度维度,高 = 越容易):

| 路线 | 开发成本 | 维护成本 | 效果上限 | 延迟风险 | 用户体验 | 分发难度 | 推荐程度 |
|---|---:|---:|---:|---:|---:|---:|---:|
| A:SpeexDSP + WASAPI + VB-Cable MVP | 5 | 3 | 2–3 | 3 | 2–3 | 4 | 4 |
| B:WebRTC AEC3 + raw WASAPI + virtual output | 3 | 3 | 5 | 4 | 4 | 3 | 5 |
| C:AEC3 + 自研 virtual mic / APO | 1–2 | 2 | 5 | 4–5 | 5 | 1–2 | 3–4 |
| 商业 SDK:NVIDIA/Superpowered/TVirtAudio 组合 | 3 | 3 | 4 | 4 | 4 | 3 | 3 |
| 硬件 AEC | 5(对软件) | 4 | 4–5 | 5 | 取决于硬件 | 5 | 2(作为软件产品) |

**不推荐路线:** 只用 NVIDIA Broadcast / RTX Voice / RNNoise(通常无 far-end reference);只做 VAD/voice isolation(视频对白被当人声保留);只用 mono reference 做成品(stereo 信息损失,至少需验证);直接移植 PulseAudio/PipeWire(平台语义不同);依赖 Synchronous Audio Router(Secure Boot/签名/GPL/ASIO 风险);直接复制 OBS GPL 代码(license 风险);以 APO 作唯一主线(Win10 覆盖不足);以硬件 AEC 替代软件产品(改变用户硬件前提)。

---

## 11. MVP 到成品路线图

工作日按 1 名熟悉 C++ / Windows audio 的工程师粗估。

| Phase | 目标 | 工作内容 | 工作日 | 验收标准 | 风险 | 降级 / 换路线 |
|---|---|---|---:|---|---|---|
| **Phase 0:离线验证** | 确认可消除性 | 录制 timestamped stereo render WAV + mono mic WAV(含 double-talk、male dialogue、music、Discord 对方录音、双讲插话);离线跑 SpeexDSP / WebRTC AEC3;输出 ERLE、delay、残余 subjective sample | 2–5 | ① 采集稳定(10 ms frame 无持续 underrun);② 能估出稳定 render→mic delay;③ Far-end only:男性对白 residual 明显下降;④ Double-talk:近端语音不被明显吞掉;⑤ ERLE >15 dB,理想 >20 dB | 数据未对齐;录制链路不同步 | 用 clapper/chirp 对齐;先固定 delay |
| **Phase 1:实时 MVP** | 跑通端到端 | 实时 ring buffer pipeline;WebRTC AEC3(或先 SpeexDSP)wrapper;delay/drift aligner 初版;VB-Cable output;简单 UI(选 mic / speaker reference / virtual output,显示 ERLE/delay/clipping);device change 自动重启 | 4–8 | ① Discord 输入可选 VB-Cable Output;② added latency <50 ms;③ 连续 1 小时无崩溃;④ 外放对白显著降低;⑤ 双讲说话不明显机器人化 | delay/drift;VB-Cable 延迟 | 降低目标,只做离线/半实时 demo |
| **Phase 2:稳定实时管线** | 长时间稳定 | ring buffer、timestamp、device switch、drift monitor、sample slip / adaptive resampler、glitch stats | 10–20 | 连续 1–2 小时无明显 drift 回声劣化;device reconnect 可恢复 | driver timestamp bug | sample counter fallback 和重校准 |
| **Phase 3:AEC3 产品化** | 提高效果上限 | 集成 WebRTC AEC3/APM、metrics、multi-channel reference、config tuning | 10–25 | double-talk、video dialogue、music case 优于 SpeexDSP | WebRTC 构建/API 变动 | 锁版本;保留 Speex fallback |
| **Phase 4:成品虚拟麦克风** | 摆脱第三方 cable / 降低用户配置 | SysVAD-derived virtual capture endpoint 或 TVirtAudio SDK;user-mode service 到 driver/endpoint 音频桥;signed test build;installer;自动命名 "AEC Microphone" | 20–45+ | ① 管理员安装后系统出现录音设备;② Discord/VRChat 可直接选择;③ 低延迟与 VB-Cable MVP 持平或更低;④ 卸载不残留坏设备;⑤ 睡眠/唤醒/插拔后可恢复 | 签名/HLK/兼容 | 短期继续 VB-Cable 或商业 SDK |
| **Phase 5:效果与鲁棒性打磨** | 从"能用"到"多数桌面外放可接受" | stereo reference 完整支持;filter tail preset;drift estimator 强化;echo path change reset;residual echo suppressor 强弱档;clipping/gain warning;device profile;crash-safe logging;optional NVIDIA Maxine AEC 对比实验 | 持续 | ① 外放音量变化中等音量下稳定;② 麦克风移动 2–5 秒内重新收敛;③ 双讲不明显吞近端人声;④ 多小时 drift 不劣化;⑤ 默认设置可用、少调参 | 过度抑制导致音质差 | 处理强度可调;提供 conservative mode |

> Neural 增强(LocalVQE / DTLN-aec)作为可选支线,按 §10.4 的 Phase N1/N2/N3 与上表并行推进。

---

## 12. 验证计划与实验设计

### 12.1 离线对比测试矩阵

用同一套数据离线对比所有引擎。输入统一成 `mic_48k.wav` + `loopback_48k_stereo.wav` →(`align + downmix + resample`)→ `mic_16k.wav` + `ref_16k.wav`(neural 模型需 16k;经典引擎用 48k)。

| 测试组 | 内容 | 目的 |
|---|---|---|
| Far-end only | 音箱播放男性对白,自己不说话 | 看能否静音/强消 echo |
| Near-end only | 自己说话,音箱静音 | 看是否伤害本声 |
| Double-talk | 视频对白 + 自己说话 | 核心场景 |
| Music echo | 音箱播放音乐 | 非语音 far-end |
| Game sound | 枪声/环境音/队友语音 | 非稳态内容 |
| Movement | 移动麦克风/转动音箱 | 看重新适应 |
| High volume | 音箱较大音量 | 看非线性失真残余 |
| Long run | 30–60 分钟 | 看 drift 和稳定性 |

**对比模型集:** LocalVQE v1.2 / v1.3、DTLN-aec 128/256/512、Microsoft DEC baseline、NKF-AEC、WebRTC AEC3 baseline、**WebRTC AEC3 + LocalVQE postfilter**。**强制单线程 benchmark**(TFLite ModelRunner 内部 `SetNumThreads(1)`,见 §3.3,否则标称实时倍率不可信)。

**客观指标管线(2026-06 新增,可直接用):** 用本地 `AEC-Challenge/AECMOS/AECMOS_local/aecmos.py:45-50` 的 **48kHz fullband AECMOS 模型**(`Run_1668423760_Stage_0.onnx`,本地 ONNX 无需联网),双指标 `echo_mos + deg_mos`,作 CI 回归。48k 版需人工标注场景 marker(st/nst/dt)。这把 §13.3「开发前必验假设 1」从主观听感升级为可量化门槛。

**进入实时工程的门槛:** Far-end only 中 echo 足够低(AECMOS echo_mos)+ Near-end only 不伤本声(deg_mos)+ Double-talk 不吞字 + 长时间不因 drift 劣化。

### 12.2 八个具体实验(每个含方法/数据/指标/通过标准/失败说明)

**14.1 loopback 延迟测量** — 方法:播放 impulse/chirp 经 loopback 捕获,同时麦克风录音,cross-correlation 估计 loopback timestamp 与 mic acoustic arrival。数据:48 kHz stereo chirp、不同播放设备、不同 buffer。指标:loopback-to-mic delay、jitter、timestamp error、discontinuity。通过:delay 稳定在 ±5 ms 内、无连续 timestamp error。失败:WASAPI timestamp 不可信或设备 buffer 抖动大,需 fallback。

**14.2 USB mic 与 render device 时钟漂移** — 方法:长时间播放 MLS/pink noise,render loopback 与 mic 同时录 30–60 分钟,分段估计相关峰位置变化。数据:48 kHz noise/chirp sequence。指标:drift ppm、reference buffer 水位趋势、alignment slope。通过:drift estimator 能稳定估计 ppm;adaptive resampling 后 delay residual <1–2 ms。失败:timestamp 或 resampler 控制不稳,需更强低通或重校准。

**14.3 SpeexDSP vs WebRTC AEC3 离线效果对比** — 方法:对同一组 render/mic recording 离线处理,固定同样 initial delay。数据:single-talk、double-talk、male dialogue、music、人声游戏台词。指标:ERLE、PESQ/POLQA(可选)、STOI(可选)、主观 ABX、用户语音失真。通过:AEC3 在 double-talk 和人声 far-end 下明显优于 SpeexDSP。失败:AEC3 集成/对齐错误,或测试数据 acoustic echo 太非线性。

**14.4 male dialogue echo case** — 方法:播放男性对白视频,用户静音和说话两组录音。数据:男声对白 far-end、用户男/女声 near-end。指标:far-end speech residual、用户语音保真、false suppression。通过:对方听不到清晰对白词句;用户音色不明显变薄。失败:AEC 把 far-end 人声与 near-end 混淆,双讲/残留抑制不足。

**14.5 double-talk 测试** — 方法:播放 far-end speech/music,同时用户读固定文本。数据:TIMIT/LibriSpeech 或自录用户语音 + far-end 播放。指标:near-end attenuation、residual echo、convergence recovery time。通过:用户语音电平下降 <3 dB;回声不形成可懂句子。失败:double-talk detection 或 residual suppression 失败。

**14.6 stereo speaker vs mono downmix reference** — 方法:同一外放布局下分别用 stereo reference 和 mono downmix 跑 AEC。数据:stereo music、游戏环境声、L/R panned speech。指标:ERLE、残留空间感、人声 residual、filter convergence。通过:stereo reference 在 L/R 差异大内容中优于 mono。失败:wrapper 没真正启用多通道,或房间/设备非线性主导。

**14.7 VB-Cable / 虚拟麦克风输出延迟测试** — 方法:AEC 输出 impulse 到 virtual cable input,用另一 app/WASAPI capture 录 virtual cable output。数据:impulse train。指标:output latency、jitter、dropout。通过:MVP <50 ms,成品目标 <20–30 ms output path。失败:virtual cable buffer 过大或 Windows audio engine 配置不合适。

**14.8 Discord / VRChat 实际输入稳定性测试** — 方法:长时间连麦,切换默认设备、拔插 USB mic、启动/关闭游戏、切换 sample rate。数据:真实游戏/视频/语音连麦。指标:app 是否丢输入、是否需重选设备、dropout 次数、CPU、延迟。通过:1 小时内无需重启 app;异常后 5 秒内恢复或明确提示。失败:virtual output 或 device watcher 不稳。

> NVIDIA Maxine 专项验证(离线 / 实时原型 / 产品化)见 §3.4.7。

---

## 13. 风险清单

### 13.1 风险矩阵

| 风险 | 严重度 | 概率 | 影响 | 缓解 |
|---|---:|---:|---|---|
| 外放音箱强非线性导致 AEC 残留明显 | 高 | 中高 | 用户仍把视频/游戏人声传出去 | 限制音量、calibration、AEC3、residual suppression、硬件建议 |
| stereo reference 集成不足 | 高 | 中 | 音乐/游戏 stereo 残留 | 保留 stereo pipeline,实验验证 |
| render/mic clock drift | 高 | 高 | 长时间效果衰减 | adaptive resampling + buffer level control(⚠️ ppm 闭环本地参考集合无 production 实现,须自研,见 §6.6) |
| `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY` 未处理 | 中高 | 中 | audio engine 已丢样本但 AEC 不知,对齐错乱、回声漏出 | 当前本地参考仓库(含 OBS)未见完整处理;检测到该 flag 触发软重置或重对齐(见 §9.7) |
| WebRTC 构建复杂/API 变动 | 中高 | 高 | 进度拖延 | 锁 milestone,封装 wrapper,保留 Speex fallback |
| 虚拟麦克风驱动签名/安装 | 高 | 高 | 产品分发困难 | MVP 用 VB-Cable,商业 SDK 评估,后续自研签名 |
| Discord/VRChat 热切换不稳定 | 中 | 中 | 用户需手动重选设备 | 稳定 endpoint name,减少设备重建 |
| shared-mode WASAPI 延迟不可控 | 中 | 中 | <20 ms 难达成 | 目标设为 <50 ms,提供低延迟模式 |
| NS/AGC 误伤用户声音 | 中 | 中 | 音质差 | 保守默认,可调强度,AB 测试 |
| OBS GPL 代码误用 | 高 | 低 | license 风险 | 只参考,不复制 |
| 商业 SDK 成本/条款不适合 | 中 | 中 | 路线切换 | 公开开源主线为基础 |
| Windows 10/11 APO 差异 | 高 | 中 | 系统级方案覆盖不足 | APO 只作为 Win11 高级路线 |
| 用户物理布局太差 | 高 | 高 | 软件效果差 | UI 诊断、摆位提示、clipping/ERLE 指标 |

### 13.2 关键未知项

- **AEC3 默认 tail 可能不够:** 默认约 52 ms 对桌面外放偏短,要验证更长 filter 配置对 CPU、收敛速度、残余 echo 的影响。
- **Drift 是产品成败点:** USB mic 和 speaker endpoint 不同 clock domain;没有 drift compensation 的 AEC 很可能短时间有效、长时间退化。必须把 timestamp、device frame position、动态重采样做成核心模块,而非后期补丁。
- **Stereo reference 需真实测试:** 左右音箱路径不同,mono reference 降低上限;AEC3 有多通道逻辑,但具体封装、输入 layout 和参数需实测。
- **虚拟驱动是产品化最大成本:** 驱动签名、安装、卸载、杀软误报、Windows 更新兼容性都会消耗大量时间(attestation 需硬件开发者计划、EV 证书、ADK、Partner Center 提交)。
- **APO 不是免费全局插件:** 现实上是驱动/endpoint 绑定的实时 COM 组件;Win11 AEC APO reference stream 是机会,但不等于可无侵入处理任意 USB 麦克风。
- **隐私与安全:** WASAPI loopback 抓的是系统播放混音,可能含会议、音乐、视频、游戏、通知音。成品应默认本地处理,不保存原始音频;日志只存指标,不存 PCM。

### 13.3 最大工程风险与开发前必验假设

**最大风险不是 AEC 算法本身,而是「外放音箱真实 acoustic path + 独立设备 clock drift + Windows 虚拟麦克风分发」三者叠加。** AEC3 提供高质量算法核心,但产品体验还取决于:reference 与 mic 是否长期 sample-accurate、stereo echo path 是否正确建模、用户布局是否物理可消、虚拟麦克风是否稳定低延迟易安装、Discord/VRChat 是否在设备切换后保持输入正常。

开发前最该验证的 3 个假设:

1. 真实外放房间中,48 kHz stereo loopback reference 与 USB mic recording 经 alignment 后,AEC3 能把视频对白/游戏人声残留降低到不可懂。
2. render/mic drift 可通过 timestamp + adaptive resampling 稳定控制,连续 1 小时不出现回声逐渐回来。
3. VB-Cable 或候选虚拟麦克风输出路径在 Discord/VRChat 中延迟和稳定性可接受,设备断连/重连不会频繁要求用户手动重配。

### 13.4 未确认项(需进一步查证)

| 未确认项 | 需要进一步查证 |
|---|---|
| WebRTC AEC3 最新 main 在 Windows 独立构建的最小依赖集 | 实际 clone、GN/CMake 构建、裁剪依赖 |
| AEC3 multi-channel render 在 stereo speaker 场景的实际收益 | 离线 AB 实验和实时测试 |
| VirtualDrivers 公开版本是否提供稳定 user-mode realtime feed API | clone 源码,检查 IOCTL/shared memory/named pipe 实现 |
| TVirtAudio SDK 授权成本和 redistributable 条款 | 商务/SDK 评估 |
| Superpowered AEC far-end reference API、stereo、Windows sample | SDK 试用 |
| Switchboard AEC node 的 Windows 支持、授权和 latency | SDK 试用 |
| NVIDIA NvAFX AEC 当前版本是否已非 beta、是否支持目标 stereo reference | 当前 SDK 包验证 |
| Discord/VRChat 对虚拟麦克风 endpoint 热切换的实际行为 | 自动化/人工长测 |
| Windows 11 APO AEC framework 能否满足第三方消费级工具分发 | APO sample、INF、HLK、签名流程验证 |

---

## 14. 参考资源清单

### 14.1 可复用仓库与 SDK 清单

| 名称 | 类型 | 链接 | Windows 可用性 | License / 成本 | 推荐 |
|---|---|---|---|---|---:|
| WebRTC AEC3 / AudioProcessing | AEC 引擎 | <https://webrtc.googlesource.com/src/> | 可构建,GN/Ninja/Clang 复杂 | BSD-style + PATENTS | 5/5 |
| get-wrecked/webrtc-audioprocessing | AEC wrapper | <https://github.com/get-wrecked/webrtc-audioprocessing> | 面向独立构建 | BSD-style | 4/5 |
| Sonora AEC3 | Rust/FFI AEC3 | <https://github.com/sonos/sonora>(亦见 dignifiedquire/sonora) | Win x64 普通 CI 通过(非 C++ reference validation) | BSD-3 | **4/5(classical AEC3 主线候选)** |
| aec3-rs | Rust AEC3 | <https://github.com/RubyBit/aec3-rs> | Cargo | MIT OR BSD-3 | 2.5/5(参考设计,milestone 旧+无 FFI) |
| SpeexDSP | AEC 引擎 | <https://github.com/xiph/speexdsp> | Windows 友好,vcpkg | BSD-like | 4/5 MVP,3/5 成品 |
| PulseAudio module-echo-cancel | AEC 架构参考 | <https://gitlab.freedesktop.org/pulseaudio/pulseaudio> | Linux,不直接落地 | LGPL | 2/5 |
| PipeWire module-echo-cancel | AEC 架构参考 | <https://gitlab.freedesktop.org/pipewire/pipewire> | Linux-first | MIT mostly | 2/5 |
| Windows WASAPI docs + samples | 采集 | <https://learn.microsoft.com/en-us/windows/win32/coreaudio/> | 官方 | MS docs/sample | 5/5 |
| Chromium WASAPI capture | 采集 | <https://chromium.googlesource.com/chromium/src/media/+/master/audio/win/> | 成熟 | BSD-style | 5/5 参考 |
| OBS plugins/win-wasapi | 采集 | <https://github.com/obsproject/obs-studio/tree/master/plugins/win-wasapi> | 成熟 | GPLv2+(注意污染) | 4/5 参考 |
| miniaudio | 音频 I/O | <https://miniaud.io/> | WASAPI backend,loopback 支持 | Public domain / MIT-0 | 3/5 |
| NAudio | 音频 I/O | <https://github.com/naudio/NAudio> | Windows/.NET | MIT | 2/5 C++ 成品 |
| PortAudio | 音频 I/O | <https://github.com/PortAudio/portaudio> | WASAPI backend | MIT-like | 2/5 |
| RtAudio | 音频 I/O | <https://github.com/thestk/rtaudio> | Windows WASAPI | MIT | 2/5 |
| Project Raven | 参考实现 | <https://github.com/Laxcorp-Research/project-raven> | Win x64(预编译 lib 实缺,需自 build) | 开源 | 3.5/5 参考(⚠️ 实为旧 APM/webrtcdsp,非 AEC3;`src/native/webrtc-aec/` C API **未接入主路径**,主应用走 GStreamer `webrtcdsp` addon;`stats.diverged` 语义错误。可复用:健康监控 bypass 阈值表 + IAecEngine C API 形状,见 §3.2) |
| Microsoft SysVAD | 虚拟音频驱动 | <https://github.com/microsoft/Windows-driver-samples/tree/main/audio/sysvad> | 官方 sample | MS-PL | 4/5 |
| Microsoft SimpleAudioSample | 虚拟音频驱动 | <https://github.com/microsoft/Windows-driver-samples/tree/main/audio/simpleaudiosample> | 官方 sample | MS-PL | 4/5 |
| VirtualDrivers Virtual Audio Driver | 虚拟音频驱动 | <https://github.com/VirtualDrivers/Virtual-Audio-Driver> | Win10/11 x64/ARM64 | MIT + MS-PL 派生 | **1/5(公开版 mic 恒静音、无 IPC,仅 INF/工程模板,见 §7.5)** |
| Synchronous Audio Router | 虚拟音频驱动 | <https://github.com/eiz/SynchronousAudioRouter> | Win10 Secure Boot 风险 | GPL | 1–2/5 |
| VB-Cable | 虚拟音频设备 | <https://vb-audio.com/Cable/> | XP–11 | donationware,商业需确认 | 4/5 MVP |
| Virtual Audio Cable | 虚拟音频设备 | <https://vac.muzychenko.net/> | Windows | 15–50 USD 级 | 3/5 测试 |
| Voicemeeter | 虚拟混音器 | <https://voicemeeter.com/> | Windows | donationware/pro | 2/5 对照 |
| TVirtAudio SDK | 商业虚拟音频 SDK | <https://vac.muzychenko.net/en/sdk.htm> | Windows | 商业授权 | 4/5 成品候选 |
| Windows APO AEC framework | 系统级 | <https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/windows-11-apis-for-audio-processing-objects> | Win11 22000+ | 驱动/APO 签名 + HLK | 3/5 |
| NVIDIA Maxine AFX AEC | 商业 AEC SDK | <https://docs.nvidia.com/deeplearning/maxine/audio-effects-sdk/index.html> | Win10/11 + RTX Tensor Core | SDK 条款 | 3/5 |
| LocalVQE | Neural AEC | <https://github.com/localai-org/LocalVQE> | CMake + C API | 开源 | 4/5 |
| DTLN-aec | Neural AEC | <https://github.com/breizhn/DTLN-aec> | 第三方 Win C wrapper | MIT | 4/5 |
| Microsoft AEC Challenge DEC baseline | Neural baseline | <https://github.com/microsoft/AEC-Challenge/tree/main/baseline/icassp2022> | ONNX Runtime | 开源 | 3/5 |
| NKF-AEC | Neural(linear) | <https://github.com/fjiang9/NKF-AEC> | PyTorch | 开源 | 3/5 |
| Superpowered SDK | 商业音频 SDK | <https://docs.superpowered.com/> | 文档列 Windows | 商业 | 3/5 需评估 |
| Switchboard Audio SDK | 商业 audio graph | <https://docs.switchboard.audio/> | 需 SDK 验证 | 商业 | 2–3/5 |
| Intel IPP | DSP primitives | <https://www.intel.com/content/www/us/en/developer/tools/oneapi/ipp.html> | Windows/Linux | oneAPI | 2/5 |
| PJSIP / PJMEDIA | 通信栈 | <https://docs.pjsip.org/> | Windows | GPL/commercial | 2/5 |
| Q-SYS AEC docs | 硬件/架构参考 | <https://q-syshelp.qsc.com/> | 非软件库 | 商业硬件 | 3/5 参考 |

### 14.2 最值得参考 / 最可能直接用

**最值得参考的 10 个仓库 / SDK:** ① WebRTC AEC3 / AudioProcessing ② SpeexDSP ③ Chromium Windows WASAPI capture ④ OBS `plugins/win-wasapi/` ⑤ Microsoft SysVAD ⑥ Microsoft SimpleAudioSample ⑦ VirtualDrivers Virtual Audio Driver ⑧ PipeWire `module-echo-cancel` ⑨ PulseAudio `module-echo-cancel` ⑩ TVirtAudio SDK。

**最可能直接拿来用的 3–5 个:**

| 名称 | 直接用途 | 备注 |
|---|---|---|
| SpeexDSP | MVP AEC | 最快集成 |
| WebRTC AEC3/APM | 成品 AEC | 需 wrapper 和构建治理 |
| raw WASAPI + Microsoft samples | 采集基础 | 自写最可控 |
| VB-Cable | MVP virtual mic output | 用户安装/授权需处理 |
| TVirtAudio SDK 或 VirtualDrivers | 成品 virtual mic 候选 | 商业/开源两条输出路线 |

### 14.3 关键链接索引(分类)

- **AEC 核心:** echo_canceller3.cc <https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/echo_canceller3.cc>;config <https://webrtc.googlesource.com/src/+/master/api/audio/echo_canceller3_config.h>;adaptive_fir_filter.cc <https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/adaptive_fir_filter.cc>;echo_remover.cc <https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/echo_remover.cc>;residual_echo_estimator.cc <https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/residual_echo_estimator.cc>;refined_filter_update_gain.cc <https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/refined_filter_update_gain.cc>;BUILD.gn <https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/BUILD.gn>;Native build <https://webrtc.github.io/webrtc-org/native-code/development/>;AEC3 overview(Switchboard)<https://switchboard.audio/hub/how-webrtc-aec3-works/>
- **快速 AEC:** speex_echo.h <https://github.com/xiph/speexdsp/blob/master/include/speex/speex_echo.h>;mdf.c <https://github.com/xiph/speexdsp/blob/master/libspeexdsp/mdf.c>;manual <https://www.speex.org/docs/manual/speex-manual/node7.html>
- **Neural AEC:** LocalVQE <https://github.com/localai-org/LocalVQE>;DTLN-aec <https://github.com/breizhn/DTLN-aec>;DTLN Win wrapper <https://github.com/RogerTeng/DTLN_AEC>;DEC baseline <https://github.com/microsoft/AEC-Challenge/tree/main/baseline/icassp2022>;NKF-AEC <https://github.com/fjiang9/NKF-AEC>;Deep Echo Path Modeling <https://github.com/ZhaoF-i/Deep-echo-path-modeling-for-acoustic-echo-cancellation>;TSPNN <https://github.com/enhancer12/TSPNN>;EchoFree <https://github.com/StellanLi/EchoFree>;FADI-AEC <https://arxiv.org/html/2401.04283v1>;LLaSE-G1 <https://huggingface.co/ASLP-lab/LLaSE-G1>;AEC Challenge paper <https://arxiv.org/html/2309.12553v1>
- **NVIDIA:** Maxine AEC 文档 <https://docs.nvidia.com/maxine/afx/latest/AboutTheEffects/AboutAcousticEchoCancellation.html>;Room Echo Removal 文档 <https://docs.nvidia.com/maxine/afx/latest/AboutTheEffects/AboutRoomEchoRemovalCancellation.html>;AFX SDK <https://github.com/NVIDIA-Maxine/Maxine-AFX-SDK>;安装/feature <https://docs.nvidia.com/maxine/afx/2.0.0/WindowsAFXSDK/InstallTheAFXSDK.html>;NvAFX_Run <https://docs.nvidia.com/maxine/afx/2.0.0/UseAFXInApps/LoadRunDestroyAnEffect.html>;SDK License <https://developer.nvidia.com/downloads/maxine-sdk-license>;Broadcast App <https://www.nvidia.com/en-us/geforce/broadcasting/broadcast-app/>;Broadcast 设置指南 <https://www.nvidia.com/en-us/geforce/guides/broadcast-app-setup-guide/>
- **Windows 采集 / 时序 / 低延迟:** loopback <https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording>;GetBuffer <https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer>;capture shared event-driven <https://learn.microsoft.com/en-us/windows/win32/coreaudio/capturesharedeventdriven>;Application loopback sample <https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/>;IAudioClock::GetPosition <https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclock-getposition>;low latency audio <https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio>;Chromium <https://chromium.googlesource.com/chromium/src/media/+/master/audio/win/audio_low_latency_input_win.cc>;OBS win-wasapi <https://github.com/obsproject/obs-studio/tree/master/plugins/win-wasapi>;OBS wasapi-output.c <https://github.com/obsproject/obs-studio/raw/refs/heads/master/libobs/audio-monitoring/win32/wasapi-output.c>;miniaudio <https://miniaud.io/docs/manual/index.html>
- **虚拟设备 / 驱动 / 签名 / APO:** VB-Cable <https://vb-audio.com/Cable/>;VAC <https://vac.muzychenko.net/>;TVirtAudio <https://vac.muzychenko.net/en/sdk.htm>;Voicemeeter <https://voicemeeter.com/>;SysVAD <https://github.com/microsoft/Windows-driver-samples/blob/main/audio/sysvad/README.md>;SimpleAudioSample <https://github.com/microsoft/Windows-driver-samples/blob/main/audio/simpleaudiosample/README.md>;VirtualDrivers <https://github.com/VirtualDrivers/Virtual-Audio-Driver>;Synchronous Audio Router <https://github.com/eiz/SynchronousAudioRouter>;驱动签名策略 <https://learn.microsoft.com/en-us/windows-hardware/drivers/install/kernel-mode-code-signing-policy--windows-vista-and-later->;attestation signing <https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-attestation>;APO 架构 <https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/audio-processing-object-architecture>;APO 实现 <https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/implementing-audio-processing-objects>;Win11 APO APIs <https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/windows-11-apis-for-audio-processing-objects>
- **架构参考 / 现有项目:** Project Raven <https://github.com/Laxcorp-Research/project-raven>;PipeWire echo-cancel <https://docs.pipewire.org/page_module_echo_cancel.html>;PulseAudio wrapper <https://github.com/pulseaudio/pulseaudio/blob/master/src/modules/echo-cancel/webrtc.cc>;PulseAudio 改进博客 <https://arunraghavan.net/2016/05/improvements-to-pulseaudios-echo-cancellation/>
- **硬件 AEC / 应用:** Q-SYS <https://q-syshelp.qsc.com/Content/Schematic_Library/acoustic_echo_canceler_simd.htm>;Yamaha YVC-200 <https://sg.yamaha.com/en/business/audio/products/speakerphones/yvc-200/>;Discord <https://support.discord.com/hc/en-us/articles/214925018-Where-d-my-Audio-Input-go-Various-Voice-Issues>;VRChat <https://help.vrchat.com/hc/en-us/articles/360062659053-I-want-to-change-where-my-audio-is-coming-from>

### 14.4 最终工程决策

- **MVP:** SpeexDSP + raw WASAPI + VB-Cable。
- **成品核心:** WebRTC AEC3/APM + raw WASAPI + adaptive render-reference resampling。
- **成品输出:** 短期依赖现成 virtual cable,中期评估 TVirtAudio,长期自研 signed virtual microphone driver。
- **Neural:** LocalVQE / DTLN-aec 作为可选 neural residual suppressor;NVIDIA Maxine 作为 RTX 用户可选增强与效果对照。
- **APO:** 保留为 Windows 11 系统级高级路线,不作为 Win10/11 统一主线。
- **不把 NS、AI voice isolation、VAD、RNNoise、NVIDIA Broadcast 类工具当作核心 AEC。**
