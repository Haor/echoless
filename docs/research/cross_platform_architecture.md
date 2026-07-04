# 跨平台实时 AEC 工具 — 架构蓝本(Windows + macOS)

> 本文是面向落地开发的**架构蓝本**,在 `windows_aec_research.md`(真理来源,Windows 调研)之上扩展跨平台设计。
> 引擎选型/源码证据仍以主文档与 `reference_repos_exploration_report.md` 为准;本文聚焦**如何把核心做成平台无关、把平台差异压到最小**。
>
> **2026-06-07 产品决策更新:** 原生虚拟麦克风驱动不再作为路线图目标。本文早期提到的 WaveRT / AudioServerPlugin “产品虚拟麦”保留为历史调研背景,当前实现与 GUI 规划长期使用 VB-Cable / BlackHole / Virtual Desktop Mic 等外部虚拟设备。当前没有证据表明 `cpal` 是瓶颈,因此不排期原生平台 I/O 重写;平台专用 I/O stub crate 已删除,通用 trait crate 已改名为 `echoless-audio-io`。当前边界见 `docs/architecture/audio_io_scope.md`。

## 0. 目标与基线决策

| 维度 | 决策 | 依据 |
|---|---|---|
| 目标平台 | **Windows 10/11 + macOS 14.4+** | 用户确认 |
| 用途 | **本地自用**,无 license / 分发 / 合规 gating | 用户确认;GPL 代码可直接复用 |
| 主语言 | **Rust**(核心 + 平台后端) | 与 aec3 主线一致 |
| 处理方案(当前两种) | **经典 AEC3(aec3)** 与 **LocalVQE**,统一为 `EchoProcessor` 节点 | 用户确认范围 |
| 组合方式 | **可单开 · 可串联 · 可自由组合 · 可扩展**(未来加更多方案不改核心) | 用户确认 |
| ~~Neural REE 深度集成~~ | **不做**(我们要的是 LocalVQE,不是 AEC3 内置 neural REE) | 用户明确否决;见 §7.1 |
| 前端 | **CLI 先行**,后期 **Electron**(经 sidecar daemon + JSON-RPC,见 §14) | 用户确认 |
| macOS 系统音频捕获 | **Core Audio Process Tap**(`AudioHardwareCreateProcessTap`,14.4+) | 本文 §8 |
| 重采样/drift | **rubato**(纯 Rust,跨平台共用) | 主文档 §6 |

**一句话原则:** 跨平台的真正成本不在 AEC 算法,而在两条 I/O 边——「系统播放声音怎么抓」「处理后人声怎么送进 Discord」。AEC3、LocalVQE、对齐、drift、重采样全部天然跨平台;框架的全部精力花在**把这两条边抽象干净**。

## 0.5 技术栈选型(完整,带理由)

| 层 | 选型 | 理由 / 备选 |
|---|---|---|
| 语言 | **Rust** | 核心 + 平台后端统一;FFI 调 C/C++ 库 |
| 处理器:经典 AEC3 | **aec3(纯 Rust,默认)** | 跨平台、纯 Rust、避开 GN/depot_tools(C++ AEC3 的「硬墙」)。实现 `EchoProcessor` |
| 处理器:LocalVQE | **LocalVQE(GGML)+ Rust FFI** | 端到端 AEC+NS+dereverb,有公开权重;macOS Metal。实现 `EchoProcessor`(可单开/可串联) |
| 处理器抽象 | **`EchoProcessor` trait + `ProcessorChain`** | 单开/串联/自由组合/可扩展;边界自动重采样+downmix |
| 可选后端(后门) | 官方 C++ AEC3(webrtc-aec3-src)+ 薄 FFI;SpeexDSP(`cc`) | 都实现 `EchoProcessor` 即可加;⚠️ C++ AEC3 编译=探索报告 §8 硬墙 #1,**非当前范围** |
| 重采样/drift | **rubato**(纯 Rust) | 跨平台共用;ppm 闭环核心;处理器边界 SRC |
| FFT | rustfft / aec3-fft | 按需 |
| 环形缓冲 | `ringbuf` / `crossbeam` SPSC | 热路径无锁;**不引入 async runtime** |
| Windows 音频 | **`windows` crate**(WASAPI) | 已在用 |
| macOS 音频 | `coreaudio-sys` + `coreaudio-rs` + `objc2` + `core-foundation`;Process Tap 手 `extern "C"` | 14.4+ 原生 |
| 虚拟麦 | VB-Cable(Win)/ BlackHole(mac)/Virtual Desktop Mic 等外部设备 | 当前产品决策:不做自研虚拟麦驱动 |
| LocalVQE 构建 | `cmake` crate 编 GGML(mac `-DGGML_METAL=ON`) | — |
| C 库构建 | `cc` crate(可选 speex / C++ AEC3) | — |
| **前端(当前)** | **CLI:`clap` + TOML 配置 + 终端 metrics** | 先行;直接内嵌 core |
| **前端(后期)** | **Electron + `echoless-daemon`(JSON-RPC over local WS/stdio)** | core 暴露 `ControlApi`,CLI/Electron 都是它的客户端(§14) |
| 错误处理 | `anyhow`(app)/ `thiserror`(lib) | — |

**一句话技术栈:** Rust + 统一 `EchoProcessor`(aec3 经典 AEC3 / LocalVQE,可单开可串联可组合可扩展)+ rubato(drift/边界 SRC)+ windows/coreaudio(I/O)+ 现成虚拟声卡(MVP,UX 仿 MicYou)+ CLI 先行、Electron 后期(经 daemon)。**核心与前端解耦,处理方案插拔。**

---

## 1. 核心设计原则

1. **核心层零 `#[cfg]`。** `echoless-core` / `echoless-processors` 不含任何平台代码;当前实时 I/O 由 `echoless-cli` 的 `cpal` 路径负责,通用 pull 式 trait 放在 `echoless-audio-io`。
2. **far-end reference 也是一个 `AudioSource`。** 核心不知道这帧 reference 来自 WASAPI loopback、macOS Process Tap、还是虚拟声卡——只拿到「带时间戳的 far-end 帧」。这一个抽象挡住 90% 平台差异。
3. **I/O 两条边用现成虚拟声卡兜底。** 两平台都用 VB-Cable / BlackHole / Virtual Desktop Mic 等外部设备兜输出(必要时兜输入),把核心 pipeline 一次写成两平台复用;当前不推进原生平台 I/O 重写,除非 diagnostics 证明 capture/loopback/timestamp/recovery 是明确瓶颈。不做自研虚拟麦。
4. **时间统一为单调纳秒。** 平台各自把 QPC / `mHostTime` 换算成 `u64` ns,核心只认 ns;drift/对齐逻辑跨平台同一套。
5. **48k 内部处理。** 所有引擎/对齐在 48k 进行;采集若非 48k,在音频 I/O 或处理器边界用 rubato 转;LocalVQE 16k 在其自己的边界内转。

---

## 2. 分层架构

```
   前端(core 的客户端,均经 ControlApi)
   ┌ echoless-cli   (当前:clap + TOML,直接内嵌 core)
   └ Electron  (后期:echoless-daemon + JSON-RPC,见 §14)
            │ ControlApi: 选链/start/stop/stats 流
            ▼
┌─────────────────────────────────────────────────────────────┐
│  echoless-core  (纯 Rust,Windows/macOS 完全共用)                  │
│  Pipeline 编排 · SPSC ring · 延迟/drift 对齐(rubato)         │
│  · 后处理 · 诊断 · ControlApi                                  │
│                                                              │
│   ProcessorChain(可单开 / 串联 / 自由组合 / 可扩展):         │
│     近端 mic ─▶ [EchoProcessor] ─▶ [EchoProcessor] ─▶ 输出    │
│                 far ref ┘ (每级都拿真实 far ref;边界自动 SRC) │
│     现有节点: ┌ Aec3Engine   (经典 AEC3,48k/stereo-ref)      │
│              └ LocalVqe      (端到端,16k/mono;可单开)        │
│     未来加更多方案 = 再写一个 impl EchoProcessor,核心不变      │
└───────────────▲──────────────────────────────────────────────┘
                │  realtime I/O: cpal + ringbuf
                │  offline I/O: echoless-audio-io wav/null
        ┌───────┴────────┬───────────────────────┐
┌───────┴──────┐  ┌──────┴───────────────────┐
│ Windows cpal │  │ macOS cpal               │
│ WASAPI host  │  │ CoreAudio host           │
│ system ref   │  │ system/external ref      │
│ 外部虚拟设备 │  │ BlackHole/外部虚拟设备    │
└──────────────┘  └──────────────────────────┘
```

**组合示例(配置驱动,运行时可切):**
- 单开经典:`mic → Aec3Engine → out`
- 单开 LocalVQE:`mic → LocalVqe → out`
- 串联(经典主消 + 端到端兜底):`mic → Aec3Engine → LocalVqe → out`
- 未来:`mic → Aec3Engine → XxxResidual → LocalVqe → out`(任意顺序/任意数量)

---

## 3. Cargo workspace 结构

```
echoless/                         (cargo workspace)
├── crates/
│   ├── echoless-audio-io/        平台无关音频 I/O trait + 类型 + wav/null 后端
│   ├── echoless-processors/      EchoProcessor trait + ProcessorChain + IoSpec(边界 SRC/downmix)
│   │     ├ aec3.rs   经典 AEC3 节点(包 aec3)
│   │     └ localvqe.rs      LocalVQE 节点(GGML FFI,cmake crate;可单开)
│   ├── echoless-core/            Pipeline / ring / aligner(rubato)/ 后处理 / metrics / ControlApi
│   ├── echoless-cli/             当前前端:clap + TOML 配置,内嵌 core,终端打印 metrics
│   ├── echoless-daemon/          (后期)headless 守护 + JSON-RPC,给 Electron 当 sidecar
│   └── echoless-recorder/        (历史规划)采集/诊断工具,当前诊断能力在 CLI realtime 内
└── vendor/  aec3 · LocalVQE   (git submodule / path 依赖)
```

当前依赖方向单向:`echoless-cli → echoless-core → {echoless-processors, echoless-audio-io}`。核心永不依赖平台专用 crate;**前端(CLI/GUI)只透过 CLI JSON 接口或 `echoless-core::ControlApi` 访问,不直接碰处理器内部实现**。新增处理方案 = 在 `echoless-processors` 加一个 `impl EchoProcessor`,其余 crate 不动。

**第三方 crate:** Windows = `windows`;macOS = `coreaudio-sys`/`coreaudio-rs`(CoreAudio/AudioToolbox)+ `objc2` + `core-foundation`;Process Tap 是 14.4 新 C API,若 `coreaudio-sys` 未覆盖则 `extern "C"` 手声明。重采样 `rubato`;WAV `hound`;LocalVQE 经 `cmake` crate 编 GGML(macOS 开 `-DGGML_METAL=ON`)。

---

## 4. 核心 trait(签名级)

```rust
// ===== echoless-audio-io:音频 I/O 抽象 =====
pub struct AudioFormat { pub sample_rate: u32, pub channels: u16 }

bitflags::bitflags! {
    pub struct PacketFlags: u32 { const SILENT = 1; const DISCONTINUITY = 2; const TS_ERROR = 4; }
}

pub struct OwnedPacket {
    pub data: Vec<f32>,        // interleaved
    pub format: AudioFormat,
    pub frames: u32,
    pub timestamp_ns: u64,     // 统一单调纳秒(Win=QPC换算, mac=mHostTime换算)
    pub device_pos: u64,       // device frame position(drift 用)
    pub flags: PacketFlags,
}

/// 麦克风 与 far-end reference 都实现它 —— 核心不区分来源
pub trait AudioSource: Send {
    fn start(&mut self) -> anyhow::Result<AudioFormat>;
    fn read(&mut self, timeout: std::time::Duration) -> anyhow::Result<Option<OwnedPacket>>;
    fn stop(&mut self);
}

/// 音频输出(MVP=写进 VB-Cable/BlackHole 等外部虚拟设备)
pub trait AudioSink: Send {
    fn start(&mut self, fmt: AudioFormat) -> anyhow::Result<()>;
    fn write(&mut self, interleaved: &[f32], frames: u32) -> anyhow::Result<()>;
    fn stop(&mut self);
}

pub trait MonotonicClock: Send + Sync { fn now_ns(&self) -> u64; }

// ===== echoless-processors:统一回声处理节点(aec3 与 LocalVQE 都实现它)=====

/// 每个处理器的「天然处理域」。chain 在节点边界按它自动重采样 + 声道适配。
/// 例:Aec3Engine = {48000, near 1ch, far 2ch};LocalVqe = {16000, near 1ch, far 1ch}
pub struct IoSpec {
    pub sample_rate: u32,
    pub near_channels: u16,
    pub far_channels: u16,
    pub algorithmic_latency_ms: f32,   // 该节点引入的算法延迟(用于总预算 & 对齐补偿)
}

pub struct ProcessorStats {
    pub name: &'static str,
    pub erle_db: f32,
    pub residual_echo_likelihood: f32,
    pub estimated_delay_ms: i32,
    pub diverged: bool,
    pub mic_clipped: bool,
}

/// 统一回声处理节点。约定:
///   near = 上一级输出(链首则为原始 mic);far = 始终为真实 far-end 参考(非上一级产物)。
///   节点只在自己的 io_spec() 域里工作;跨域转换由 ProcessorChain 负责,节点不关心。
///   有状态节点(如 LocalVQE LSTM、AEC3 滤波器)即便被旁路也应持续喂帧——由 chain 保证。
pub trait EchoProcessor: Send {
    fn name(&self) -> &'static str;
    fn io_spec(&self) -> IoSpec;
    fn configure(&mut self, params: &toml::Value);   // 方案各自的参数(tail/preset/gate…)
    fn set_stream_delay_ms(&mut self, ms: i32);       // 不需要的节点空实现
    /// near/far 已被 chain 转到本节点 io_spec 域;写 out(同域)
    fn process(&mut self, near: &[f32], far: &[f32], out: &mut [f32], frames: u32);
    fn stats(&self) -> ProcessorStats;
    fn reset(&mut self);
}

/// 把若干 EchoProcessor 串成链;负责相邻节点间的 SRC + 声道适配 + far ref 分发到各节点域。
/// 单开 = 长度 1 的链;串联/组合 = 配置里给一个有序节点列表。
pub struct ProcessorChain { /* nodes + per-edge rubato resamplers + far-ref fanout */ }
impl ProcessorChain {
    pub fn from_config(cfg: &ChainConfig, base_rate: u32) -> anyhow::Result<Self>;
    /// near=原始 mic(base_rate);far=真实 ref(base_rate);out=链尾(base_rate)
    pub fn process(&mut self, near: &[f32], far: &[f32], out: &mut [f32], frames: u32);
    pub fn total_latency_ms(&self) -> f32;
    pub fn stats(&self) -> Vec<ProcessorStats>;
    pub fn reset(&mut self);
}

// 配置驱动(CLI 的 TOML / 后期 Electron 的 JSON 都映射到它)
pub struct ChainConfig { pub nodes: Vec<NodeConfig> }          // 有序,空=直通
pub struct NodeConfig  { pub kind: String, pub params: toml::Value }  // kind: "aec3" | "localvqe" | 未来…

// ===== echoless-core::ControlApi:CLI 现在用、Electron 后期用(同一套)=====
pub trait ControlApi: Send + Sync {
    fn list_devices(&self) -> Vec<DeviceInfo>;             // mic / 系统音频源 / 输出
    fn start(&self, cfg: &PipelineConfig) -> anyhow::Result<()>;
    fn stop(&self);
    fn set_chain(&self, cfg: &ChainConfig) -> anyhow::Result<()>;  // 运行时换链
    fn subscribe_stats(&self) -> Receiver<Vec<ProcessorStats>>;    // 推流给 UI
}
```

---

## 5. 平台后端映射表(基线 macOS 14.4+)

| 边 | Windows 10/11 | macOS 14.4+ |
|---|---|---|
| 麦克风采集 | WASAPI shared event-driven(主文档 §5.2) | CoreAudio `AUHAL`(`kAudioUnitSubType_HALOutput`,enable input)/ AVAudioEngine |
| **系统播放(far-end ref)** | WASAPI loopback `AUDCLNT_STREAMFLAGS_LOOPBACK`(一等公民) | **Core Audio Process Tap**(`AudioHardwareCreateProcessTap` + aggregate device,§8) |
| 虚拟麦输出 | VB-Cable / VAC / 用户选择的外部虚拟设备 | BlackHole / Virtual Desktop Mic / 用户选择的外部虚拟设备 |
| 时钟 | QPC `QueryPerformanceCounter` | `mach_absolute_time` / `AudioTimeStamp.mHostTime`(同 mach 时基) |
| 实时线程 | MMCSS `"Pro Audio"`(主文档 §9.5) | `os_workgroup`(从 device `kAudioDevicePropertyIOThreadOSWorkgroup` 取,join) |
| AEC3 | **aec3**(共用) | **aec3**(共用,2400+ C++ validation 跑在 macOS) |
| 重采样/drift | **rubato**(共用) | **rubato**(共用) |
| LocalVQE | GGML CPU(可选 CUDA/Vulkan) | GGML CPU + **Metal** |
| Rust 绑定 | `windows` | `coreaudio-sys` + `objc2` + `core-foundation`(Process Tap 手 `extern "C"`) |

---

## 6. 数据流与线程模型(跨平台同构)

```
[RefSource 线程]  系统音频 → OwnedPacket(ts) → SPSC ring(ref)
[MicSource 线程]  麦克风   → OwnedPacket(ts) → SPSC ring(mic)
                                  │
[Processing 线程(实时优先级)]   每 10ms tick:
    pull mic frame → 按 ts 从 ref ring 取对齐帧 → DelayDriftAligner(rubato)
    → ProcessorChain.process(near=mic, far=ref)    // 单开/串联/组合由配置决定
        每个节点:near=上一级输出, far=真实 ref;chain 在节点边界自动 SRC/downmix
    → PostProcess(HPF/limiter) → SPSC ring(out)
                                  │
[Sink 线程]  out ring → AudioSink.write()(VB-Cable / BlackHole / 原生驱动)
```

- **线程模型与主文档 §9.5 一致**,只是优先级设置是平台 adapter:Windows = MMCSS Pro Audio;macOS = `os_workgroup` join + time-constraint。
- **mic 主时钟铁律**(主文档 §6.2/§6.6):fractional resampler 只放 ref 路径,mic 路径绝不重采样。跨平台同。
- 所有 ring 固定容量预分配;热路径不分配、不锁、不写文件、不进 COM/Obj-C 运行时调用。

---

## 7. 处理器组合策略(单开 / 串联 / 自由组合)

当前两种方案都是平级的 `EchoProcessor` 节点,**没有「主引擎 + 残余」的固定主从关系**——怎么组合由配置决定:

| 组合 | 链 | 适用 |
|---|---|---|
| 单开经典 | `mic → Aec3Engine → out` | 48k/stereo-ref/robust DTD,CPU 最低,默认起点 |
| 单开 LocalVQE | `mic → LocalVqe → out` | 端到端 AEC+NS+dereverb;非线性/混响强、但 16k mono、double-talk ERLE 仅 8.5dB |
| 串联(经典→端到端) | `mic → Aec3Engine → LocalVqe → out` | 经典先把线性回声/双讲压住,LocalVQE 收非线性/混响残余 |
| 未来扩展 | `mic → Aec3Engine → Xxx → LocalVqe → out` | 加新 `impl EchoProcessor` 即可,任意顺序/数量 |

**chain 自动处理的事(节点不关心):**
- **边界 SRC + 声道适配**:Aec3Engine 在 48k(near 1ch / far 2ch),LocalVqe 在 16k mono;chain 在两节点之间用 rubato 做 `48k↔16k` + downmix。
- **far ref 分发**:每个节点的 `far` 始终是**真实 far-end 参考**(转到该节点域),不是上一级产物;`near` 才是上一级输出。
- **延迟累计**:`total_latency_ms()` 把每个节点 `io_spec().algorithmic_latency_ms` + 边界 SRC 延迟加起来,计入延迟预算(主文档 §9.3)。串联会叠加 LocalVQE 的 ~16ms,需权衡。
- **有状态节点持续喂帧**:LocalVQE 的 LSTM / AEC3 的滤波器即便被旁路也保持喂数据,避免状态断裂(主文档 §3.3「始终运行」铁律)。

**工程注意:** LocalVQE 的 GGML/TFLite 标称多线程实时倍率需在 production 单线程下重测(探索报告 §4.3 #20);串联时延迟预算要把它算进去。

### 7.1 为什么**不做** AEC3 内置 neural REE(也不为此改造 aec3)

用户已明确:我们要的是 **LocalVQE 作为独立可组合节点**,不是 AEC3 内置的 `NeuralResidualEchoEstimator` 深度集成。两者是不同的东西,这里记录为何放弃后者(2026-06 源码评估,以免日后重复纠结):

- **改造 aec3 技术上可行但不值得**:`aec3-core/src/residual_echo_estimator.rs:179` 明确跳过 neural REE;neural REE 需要的中间状态(`s2_linear/y2/e2/r2/dominant_nearend/时域 y,e/render block`)aec3 其实都已算好(`echo_remover.rs:225-354`,命名对齐官方 M145),纯代码 port 约 2-3 周。
- **真瓶颈与 aec3 无关**:① **AEC3 ML-REE 训练权重 Google 不公开**(只在 chromium 内部),自训需数月;② LSTM+Hanning+PFFFT+0.15 次幂,数值对齐无 golden test。无权重则一切是空架子。
- **LocalVQE 的 GGML 不能复用为 neural REE**——它是 DeepVQE 衍生的端到端模型(替代整个 AEC3+NS),不是 AEC3 的 residual mask,两条不相交的路。

**结论:neural REE 整个移出当前与可见未来的目标。** LocalVQE 通过 `EchoProcessor` 串联即可获得「经典 + 神经」的组合收益,无需深度集成。**唯一保留的低成本后门(~1 天,可选):** 给 aec3-core 留一个 `NeuralResidualEchoEstimator` trait + 扩 `ResidualEchoInput` 的 hook 不实现,纯粹为「万一哪天拿到权重」留口子——非当前任务。

---

## 8. macOS 专题(14.4+)

### 8.1 系统音频捕获 = Core Audio Process Tap(最大不对称点)

macOS 无 WASAPI loopback 等价物;14.4+ 用原生 Process Tap,**无需 BlackHole 捕获**:

1. 建 `CATapDescription`(可选 stereo mixdown / 指定或排除进程;默认抓全系统混音作 reference)。
2. `AudioHardwareCreateProcessTap(desc, &tapID)` 得到 tap 对象。
3. 用 `AudioHardwareCreateAggregateDevice` 建聚合设备,把 tap 放进 `kAudioAggregateDeviceTapListKey`。
4. 在聚合设备上装 `AudioDeviceIOProc`(或 AUHAL)读出 far-end PCM + `AudioTimeStamp.mHostTime`,封成 `OwnedPacket` 喂核心。
5. **时间戳**:`mHostTime` 是 mach 时基,与 mic 输入回调同源 → drift 估计逻辑直接复用核心(主文档 §6.6)。

注意:
- **TCC 授权**:Process Tap 抓系统音频需用户授权(系统隐私弹窗),要设计授权引导与失败兜底;Windows loopback 无此要求。
- 14.2 引入、14.4 稳定;固定基线 14.4+ 可直接用,无需 ScreenCaptureKit / BlackHole 降级路径(若日后要支持 13,则加 ScreenCaptureKit 后端,仍实现同一个 `AudioSource` trait,核心不变)。

### 8.2 虚拟麦克风输出 = 外部虚拟设备

- 直接用 BlackHole / Virtual Desktop Mic / 用户选择的外部虚拟设备作输出端。我们的 app 把 clean 人声播到该输出设备,Discord/VRChat 选择对应输入即可。
- AudioServerPlugin 保留为历史调研背景,当前不做。它虽然比 Windows 内核驱动门槛低,但仍会带来安装、授权、兼容和维护成本。
- `AudioSink` 继续抽象成“写入用户选择的输出设备”,不承诺创建系统级虚拟麦。

### 8.3 实时线程

- 从聚合设备/输出设备取 `kAudioDevicePropertyIOThreadOSWorkgroup`,processing 线程 `os_workgroup_join` 加入音频 workgroup,获得与系统音频 I/O 同步的实时调度;否则用 pthread time-constraint policy。

### 8.4 LocalVQE on macOS

- GGML 开 Metal(`-DGGML_METAL=ON`)走 GPU,或纯 CPU(Apple Silicon NEON 也够 16k 实时)。`cmake` crate 编出 `liblocalvqe`,FFI 同 Windows。

---

## 9. Windows 专题(简述,详见主文档)

- 采集鲁棒性 clean-room 抄 OBS(自用无 license 顾虑,可直接复用):事件状态机 / SILENT 双路径 / DEVICE_INVALIDATED 静默 / while 排空 / 时间戳分两路(主文档 §5.4)。
- 虚拟麦:MVP VB-Cable;产品 simpleaudiosample 派生(`WriteBytes` 换 shared-memory),声明 AEC effect、禁 mic-array 防双 AEC(主文档 §7.3)。
- external delay:`use_external_delay_estimator` + `SetAudioBufferDelay`(4ms 倍数),外面配独立 GCC-PHAT sanity check(主文档 §4.1)。

### 9.2 虚拟麦克风安装/配置 UX(参考 `reference_repos/MicYou`,跨平台)

MicYou(Kotlin/GPLv3,**只作 how-to 参考不抄代码**)给了「不写驱动、自动化第三方虚拟声卡」这条 MVP 路线最完整的开源实现,正好补我们最大的自研缺口的 MVP 版。值得直译成 Rust 的工程模式:

- **Windows**(`MicYou/.../platform/VBCableManager.kt`,856 行):自动**下载** VB-Cable 包(`:769`)→ `Start-Process -Verb RunAs` UAC 提权安装(`:633`)→ 用 nirsoft **SoundVolumeView** 设默认录音设备/采样格式(`:301,323`)→ 无工具时直接读写注册表 `HKLM\...\MMDevices\Audio`(`:412-465`)→ 双重检测防幽灵设备(JavaSound mixer + `reg query ...\Services\VB-Cable`,`:77`)→ UAC 拒绝/超时分类 + 卸载回滚(`:735-757,847`)。Rust 对应:`reqwest` 下载 + `ShellExecuteW`(runas)+ `winreg`。
- **macOS**(`BlackHoleManager.kt`):`SwitchAudioSource -f json` 枚举/切换(`:42`),正则 `BlackHole\s*\d*ch` 兼容 2ch/16ch,保存/恢复原输入设备(`:110-127`)。
- **Linux**(`PipeWireManager.kt`,若日后做):`pw-cli create-node ... support.null-audio-sink` + `pw-loopback` 把 sink monitor 变 source(`:182-218`),**纯命令行零第三方驱动**。
- **抽象模式**:`AudioEngine.kt` `expect/actual` + `PlatformAdaptor.kt` 的 `usesSystemAudioSinkForVirtualOutput` 能力开关——直译成我们的 `AudioSink` trait + `cfg` 分发 + `VirtualDeviceManager{ install / set_default / restore }`。

**Verdict(MicYou):** AEC 引擎层无价值(单端 dereverb + denoiser,非 reference-based);**虚拟麦安装/配置 UX 层中-高参考价值**。它也反向印证我们 MVP「用现成虚拟声卡」方向正确。

---

## 10. 重采样 / drift / 延迟(跨平台共用)

完全复用主文档 §6 / §6.6 的结论,实现是纯 Rust 跨平台:
- 两层对齐:粗对齐(byte-skip,>5ms 容差,+10 帧 safety)+ 细对齐(rubato `set_resample_ratio_relative`,1s watchdog,50ms ramp)。
- drift 三段式 = PulseAudio `calc_diff` 公式 + rubato 实现 + 引擎 external delay;mic 主时钟。
- ppm 闭环 fractional resampling 本地参考集合无 production 实现 = 我们的核心自研模块(主文档 §6.6),**跨平台只写一次**。

---

## 11. 分阶段路线(对照主文档 §11)

| 阶段 | Windows | macOS | 共用核心 / 前端 |
|---|---|---|---|
| **P0 采集** | ✅ echoless-recorder(已交付) | 当前用 cpal/CoreAudio 设备路径;Process Tap 仅保留研究背景 | OwnedPacket / sidecar 格式 |
| **P1 离线评测** | 同一套 harness | 同一套 | AECMOS_local + aec3/LocalVQE 对比(经 EchoProcessor 离线跑) |
| **P2 实时主路径** | cpal/WASAPI + VB-Cable | cpal/CoreAudio + BlackHole/外部虚拟设备 | echoless-core + ProcessorChain;**echoless-cli**(TOML 选链) |
| **P3 处理器成品** | aec3 节点 + LocalVQE 节点,单开/串联可配 | 同 | EchoProcessor 全跨平台共用;运行时换链 |
| **P4 原生 I/O** | 不排期;仅在 cpal 被证明是瓶颈时重新评估窄范围 I/O | 不排期;仅在 cpal 被证明是瓶颈时重新评估窄范围 I/O | — |
| **P5 前端 + 打磨** | stereo/tail/drift 强化 | 同 | **Electron + echoless-daemon(JSON-RPC)**;主文档 §11 Phase 5 |

**节奏要点:** ① 两平台都用现成虚拟声卡兜 I/O,核心 pipeline 一次写成两平台跑通;② **前端 CLI 先行(P2 起),Electron 留到 P5**——但 P2 就把 `ControlApi` 边界划好,CLI 直接内嵌、Electron 后期经 daemon,二者共用同一控制面(§14);③ 原生平台 I/O 不排期,只在证据明确时作为窄范围修补。

---

## 12. 关键风险与未决

| 项 | 平台 | 缓解 |
|---|---|---|
| Process Tap TCC 授权流程 / 行为 | macOS | 仅当 cpal/CoreAudio reference 路径被证明不可接受时再调研 |
| `os_workgroup` join 失败退化 | macOS | fallback time-constraint;监控 underrun |
| LocalVQE 串联额外延迟拉高总预算 | 双 | 延迟预算把 ~16ms+重采样算进 §9.3;残余设为可关 |
| aec3 Windows 数值/长稳未自测 | Windows | P1 与 official C++ 数值对照(探索报告 §8) |
| ppm 闭环无参考实现 | 双 | 先离线验证 calc_diff 公式(主文档 §6.6),再上实时 |
| 自研虚拟麦 user→驱动通道 | 双 | 不做;使用外部虚拟设备 |

---

## 13. 与现有文档的关系

- **引擎选型、源码证据、Windows I/O 细节、风险矩阵** → 以 `windows_aec_research.md` 为准(已按探索报告做 8 处修正)。
- **仓库级证据(aec3/LocalVQE/AEC3 file:line)** → 以 `reference_repos_exploration_report.md` 为准。
- **本文** → 跨平台分层、trait 边界、macOS 专题、workspace 结构的蓝本。三者交叉引用,不重复证据。

---

## 14. 前端架构(CLI 先行,Electron 后期)

核心与前端**彻底解耦**:`echoless-core` 暴露一个 `ControlApi`(§4),前端只是它的客户端。这样 CLI 和 Electron 共用同一控制面,换前端不动核心。

### 14.1 当前:CLI(`echoless-cli`)

- `clap` 解析参数 + TOML 配置;**直接内嵌 `echoless-core`**(同进程,无 IPC 开销)。
- 配置即「设备选择 + 处理链」:
  ```toml
  mic    = "default"          # 或设备名/ID
  ref    = "system"           # Win=loopback, mac=当前 cpal/CoreAudio 参考路径
  output = "CABLE Input"      # Win=VB-Cable, mac=BlackHole
  [[chain]]                   # 有序;空 = 直通
  kind = "aec3"
  [[chain]]
  kind = "localvqe"
  model = "localvqe-v1.3.gguf"
  ```
- 终端实时打印 metrics(ERLE/delay/drift/diverged),`subscribe_stats()` 拉流。
- 单开/串联/组合就是改 `[[chain]]` 列表,**无需重编**。

### 14.2 后期:Electron(经 `echoless-daemon`)

- **不把音频实时跑进 Node/Electron 进程**(GC/事件循环会破坏实时性)。改为:`echoless-daemon`(headless,内嵌 core,跑在原生进程)+ Electron 前端,二者用 **JSON-RPC over local WebSocket 或 stdio**。
- daemon 把 `ControlApi` 的方法映射成 RPC(`list_devices/start/stop/set_chain`),`subscribe_stats` 映射成 server→client 推送。Electron 渲染设备选择、链编辑器(拖拽节点)、metrics 仪表盘。
- 不用 napi-rs 把 core 塞进 Electron 主进程——sidecar daemon 隔离更稳,且 daemon 可独立于 UI 常驻(关掉窗口 AEC 继续跑)。
- **现在就要做的前置准备:把 `ControlApi` 的类型设计成可序列化**(`serde`),CLI 用本地结构、daemon 用同结构 + serde——P2 就划好边界,P5 直接长出 Electron。

*2026-06-05 创建,2026-06-05 更新(处理器统一为可组合 EchoProcessor 图;neural REE 移出目标;前端 CLI 先行 Electron 后期;加入 MicYou 参考);基线 Windows 10/11 + macOS 14.4+,处理方案仅经典 AEC3(aec3)+ LocalVQE,本地自用无 license 约束。*
