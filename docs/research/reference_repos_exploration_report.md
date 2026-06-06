# 参考代码库自主探索报告

> 范围:`reference_repos/` 下 16 组(21 个顶层目录,部分同类仓库按组归并)的源码级深扒,结合 `research/windows_aec_research.md` 已有结论与确定的需求目标(Windows 外放音箱 + USB 麦克风,reference-based 实时 AEC,经虚拟麦克风送 Discord/VRChat)。
> 方法:graphify 索引导航 + 逐文件源码核实,关键「直接可用」结论带 `file:line` 证据。关键判断以**源码为准**(README 与源码不一致处按已发现证据修正)。
> 产出统计:**86 条直接可用项 · 135 条被忽视高价值点 · 16 组/21 顶层目录覆盖**。建议弃用 1(EchoFree),降权 2(Virtual-Audio-Driver 公开版、SynchronousAudioRouter)。
> 2026-06-05 复核更新:修正 `sonora` 的 Windows/C++ validation 口径、确认 `sonora-aec3` 尚未移植 neural REE、补充 AecApo 样例缺口,并收窄 `DATA_DISCONTINUITY` 的证据边界。
> 2026-06-05 增补:新加入第 17 个仓库 **`MicYou`**(Kotlin/GPLv3)——非 AEC,但「不写驱动、自动化第三方虚拟声卡」的安装/配置 UX 是我们最大自研缺口的 MVP 参考(见 §3.6、§6)。同时确认架构方向:经典 AEC3(sonora)与 LocalVQE 统一为可组合 `EchoProcessor` 节点(可单开/串联/扩展),**neural REE 移出目标**——详见 `cross_platform_architecture.md`。

---

## 1. 执行摘要(最重要的结论)

1. **AEC3 已经把 neural 残余抑制从「串联」升级成「深度集成接口」(本次最大认知更新)。** 当前官方 AEC3 源码新增了可注入的 `NeuralResidualEchoEstimator` 抽象基类(TFLite 实现),与内部 `dominant_nearend / S2_linear / Y2 / E2` 全部中间状态共享。这意味着「AEC3 主消 + LocalVQE/DTLN 残余」的工程量可从「写独立 neural 模块 + 时序对齐」降到「写一个 `NeuralResidualEchoEstimator` 子类」,且官方给了 production tuning(`webrtc-aec3-src/aec3/neural_residual_echo_estimator/neural_residual_echo_estimator_impl.cc:569-589`)。**现有调研把主要 neural 候选定位为 AEC3 之外的串联后处理,是最大盲区。**

2. **`sonora` 是 WebRTC APM/AEC3 的纯 Rust M145 移植 + 现成 `wap_*` C API,但不是 2025 neural REE 的现成替代品。** Windows x64 CI 覆盖普通 build/test;2400+ C++ reference validation 在 Ubuntu/macOS 跑,不是 Windows;且 `sonora-aec3` 明确跳过 `NeuralResidualEchoEstimator`。因此它仍是 classical AEC3/APM 的强主线候选(2.5/5 → 4.0/5),能显著降低 GN/depot_tools 成本,但若要官方 2025 neural REE 路径,仍需 C++ official 或补 port(`sonora/README.md:57-71`、`sonora/crates/sonora-aec3/src/residual_echo_estimator.rs:179`、`sonora/crates/sonora-ffi/src/functions.rs:22-52`)。

3. **虚拟麦克风的 user→kernel 实时喂音频通道,当前本地参考集合留白——必须自研或购买。** `Virtual-Audio-Driver` 公开版的 `WriteBytes` 是 `RtlZeroMemory`(**永远输出静音**),基线 3/5 严重高估,实际可用度为 0(`Virtual-Audio-Driver/Source/Main/minwavertstream.cpp:1392-1421`);作者明确把 IPC 通道作为付费定制内容。`simpleaudiosample` / `sysvad` 也都只写正弦波。这是落地最大的自研缺口(预算 2-8 人周)。

4. **虚拟麦克风的最佳起点是 `simpleaudiosample` 而非 `sysvad`;且 `Windows-driver-samples/audio/sysvad/APO/AecApo` 揭示了一条被低估的 Win11 官方 AEC APO 接口路线。** 走 APO,OS audio engine 会把 render endpoint 的同步 loopback 作为 aux input 喂到 APO,可把一部分延迟/对齐/设备恢复问题交给系统接口处理(`Windows-driver-samples/audio/sysvad/APO/AecApo/AecApoMfx.cpp:610-671,720-738`)。但样例算法体为空、16k mono 硬编码、仅 COMMUNICATIONS 模式、audiodg 沙箱约束,且 `AcceptInput` 中有明显 `inputId`/`dwInputId` typo;它是 API 模板,不是可直接落地的 AEC 实现。

5. **「mic 主时钟 + render 自适应 ppm fractional resampling」的真正闭环,当前本地参考集合未见 production 实现——这是我们的技术创新空间。** 四处死代码/注释互证 drift 必须在应用层做(PulseAudio `pulseaudio/src/modules/echo-cancel/webrtc.cc:369` 空 stub + `pulseaudio/src/modules/echo-cancel/module-echo-cancel.c:369` rate 计算被注释 + PipeWire `pipewire/spa/plugins/aec/aec-webrtc.cpp:268` 强制关闭 + AEC3 `clockdrift_detector` 只分类不补偿)。但 PulseAudio 的 `calc_diff` 公式 + 参数表 + `apply_diff_time` 可 clean-room 复刻(`pulseaudio/src/modules/echo-cancel/module-echo-cancel.c:138-178,299-335,678-704`)。

6. **`obs-studio/plugins/win-wasapi` 是本地参考集合里 WASAPI 采集鲁棒性最完整的生产级来源(GPLv2,只能 clean-room 重写)。** 一站式覆盖 event-driven、SILENT 双路径、DEVICE_INVALIDATED 静默重连、默认设备热切换、loopback 静音预热、MMCSS。`project-raven` 的轮询式采集几乎踩遍所有坑,可作「不要这么写」清单。

7. **`use_external_delay_estimator` + `SetAudioBufferDelay` 是外放场景的硬要求,不只是「契合 QPC」而已。** 它既是把 WASAPI 时间戳接进 AEC3 的正确路径,也是 neural REE 拿到 12ms 前瞻 render 处理非线性 echo 的前提(`webrtc-aec3-src/aec3/residual_echo_estimator.cc:217-238`)。但切到 external 后内部 matched_filter 被完全 bypass,失去兜底,必须外面再跑一份独立 GCC-PHAT sanity check。

8. **最大未解难点(需自研)集中在 4 处:** (a) ppm 级闭环 fractional resampling;(b) 虚拟麦克风 user→kernel 通道 + EV 签名/HLK;(c) `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY` 处理(当前本地参考仓库未见完整处理);(d) 48kHz 全带 + stereo-ref 的 neural REE(本地可改的开源 neural 候选主要是 16k mono)。

---

## 2. 直接可用清单(按可用度排序)

> 「集成成本」= 接入到我们 Rust/C++ 主线的工作量;GPL 项均标注「只学不抄」。

| # | 东西 | 来源 file:line | License | 成本 | 关键坑 |
|---|---|---|---|---|---|
| 1 | **sonora 全栈**(纯 Rust WebRTC APM M145,含 AEC3/NS/AGC2)+ `sonora-ffi` 现成 `wap_*` C ABI(staticlib+cbindgen) | `sonora/README.md:57-71`;`sonora/crates/sonora/src/audio_processing.rs:400-405,564-580`;`sonora/crates/sonora-ffi/src/functions.rs:22-52` | BSD-3 | 低 | 0.1.0 初期接口;f32 为 deinterleaved、i16 为 interleaved;2400+ C++ validation 非 Windows;neural REE 未 ported;config 1036 行无 preset |
| 2 | **AECMOS_local 48kHz fullband 评测模型**(echo_mos + deg_mos 双指标,本地 ONNX 无需联网) | `AEC-Challenge/AECMOS/AECMOS_local/aecmos.py:45-50` + `Run_1668423760_Stage_0.onnx` | MIT | 低 | 48k 版需人工标注场景 marker(st/nst/dt) |
| 3 | **AEC3 `NeuralResidualEchoEstimator` 抽象接口 + 官方 AdjustConfig tuning** | `webrtc-aec3-src/api-audio/neural_residual_echo_estimator.h:26-65`;`webrtc-aec3-src/aec3/neural_residual_echo_estimator/neural_residual_echo_estimator_impl.cc:569-589` | BSD-3 + PATENTS | 中 | 仓库不含模型;frame 硬锁 256 样本(16ms@16k);只跑 band0 |
| 4 | **AEC3 `use_external_delay_estimator` + `SetAudioBufferDelay`** 外部延迟接入 | `webrtc-aec3-src/aec3/render_delay_buffer.cc:351,375-384`;`webrtc-aec3-src/api-audio/echo_canceller3_config.h:55` | BSD-3 + PATENTS | 低 | 切 external 后内部 matched_filter 完全 bypass,无自我修正 |
| 5 | **LocalVQE 全套**(`liblocalvqe.dll` + C API + Windows backend 自发现 + 1024ms echo 窗 + 16ms 延迟) | `LocalVQE/ggml/localvqe_api.h:25-258`;`LocalVQE/ggml/localvqe_graph.cpp:531-635` | Apache-2.0 | 低 | 16k mono 写死;权重 19MB 不在仓库;Windows 路径官方自承未实测 |
| 6 | **webrtc-audioprocessing 4 文件 CMake harness**(把 AEC3+APM 编成 MSVC 静态库) | `webrtc-audioprocessing/{CMakeLists.txt,sources.cmake}` | Apache-2.0(壳) | 中 | 「脱离 GN/Ninja」半真:仍需 depot_tools 拉源码;需手 patch M124 的 AVX2 bug + Python3 |
| 7 | **SpeexDSP 五件套**:MDF/AUMDF AEC + `speex_preprocess` 残余抑制 + `speex_resampler`(set_rate_frac drift)+ `speex_decorrelate` + jitter buffer | `speexdsp/libspeexdsp/{mdf.c,preprocess.c,resample.c:797-1225,scal.c:85-280}` | BSD-3 | 低 | API 锁 int16;MC 路径源码自标 FIXME;tail 无运行时 setter;默认采样率 8000 必须改 |
| 8 | **simpleaudiosample 虚拟 mic+speaker WaveRT 骨架**(核心注入点 = `WriteBytes`) | `Windows-driver-samples/audio/simpleaudiosample/Source/Main/minwavertstream.cpp:1392-1421` | MS-PL | 中 | 注入通道要自研;1ms DPC 软时钟;签名需 EV+HLK(capture-only 可缩小范围) |
| 9 | **sysvad/APO/AecApo**(Win11 官方 AEC MFX APO 模板,OS 自动喂 aux loopback) | `Windows-driver-samples/audio/sysvad/APO/AecApo/AecApoMfx.cpp:610-738` | MS-PL | 中-高 | 算法体为空壳;16k mono 写死;仅 COMMUNICATIONS;`AcceptInput` 样例有 `inputId` typo;audiodg 沙箱无锁无分页 |
| 10 | **PulseAudio `calc_diff` 延迟对齐公式 + 工程参数表**(5ms 容差/1s watchdog/+10帧 safety) | `pulseaudio/src/modules/echo-cancel/module-echo-cancel.c:299-335,138-178,678-704` | LGPL(只学公式) | 低 | Linux 变量要换成 WASAPI 等价物(GetCurrentPadding 等) |
| 11 | **DTLN-aec TFLite 三档权重 + DTLN_AEC-wrapper Win x64 standalone**(预编译 TFLite v2.5.2) | `DTLN-aec/run_aec.py:52-64`;`DTLN_AEC-wrapper/DTLN_AEC/DTLN_AEC.cpp:19-20,321-419` | MIT + Apache-2.0 | 中 | 32ms 窗口/8ms hop,wrapper 以 512 样本块处理;16k mono;C++ class 非 C ABI |
| 12 | **Maxine `Maxine-AFX-SDK/nvafx/include/nvAudioEffects.h` C API**(RTX 用户可选 GPU AEC 后端) | `Maxine-AFX-SDK/nvafx/include/nvAudioEffects.h:100-228`;`Maxine-AFX-SDK/samples/effects_demo/effects_demo.cpp:213-219,535-663` | 头 MIT / 运行时 EULA | 低 | AEC 示例以 mic/far 两路输入调用;通道数从 SDK 参数查询;无 delay/drift/ERLE API;DLL/模型需用户装;不能进 WASAPI 回调线程 |
| 13 | **NKF-AEC GCC-PHAT 延迟估计(30 行 numpy)+ 28KB 权重** | `NKF-AEC/src/utils.py:5-38`;`NKF-AEC/src/nkf_epoch70.pt` | BSD-3(需补 utils.py 来源 MIT) | 低 | one-shot 离线,非 sliding;16k/64ms tail |
| 14 | **project-raven 健康监控 bypass 状态机 + IAecEngine C API 形状** | `project-raven/src/main/systemAudioNative.ts:111-340`;`project-raven/src/native/webrtc-aec/include/aec_api.h:1-88` | MIT | 低 | 主路径加载 GStreamer addon;`project-raven/src/native/webrtc-aec/` 是遗留老 APM 路径;阈值表可复用,实现不要照搬 |

---

## 3. 可直接参考的成熟设计(按主题)

### 3.1 WASAPI 采集鲁棒性(真理来源:obs-studio,GPL 只学不抄)
- **七 Event 协作状态机 + inactive/active sigs 切换**:线程长期存活分两阶段(先等 init,成功才进主循环),比「init 失败线程退出再重启」代码量小一个数量级(`win-wasapi.cpp:217-225, 1066-1180`)。
- **SILENT 双重处理**:(a) capture 端把指针重定向到自家 silence vec;(b) loopback 端用独立 `IAudioRenderClient` 预写一帧零样本防 reference 流冻结——**两个独立技巧,缺一不可**(`:1003-1012` + `:724-764`)。
- **DEVICE_INVALIDATED 静默白名单**:拔出时驱动高频返回此错误,必须加白名单不打日志,否则刷爆磁盘;`GetNextPacketSize` 与 `GetBuffer` 两处都要检查(`:974, :988`)。
- **`ProcessCaptureData` while(true) 排空多 packet**:单次 event 唤醒常对应多 packet 堆积,只取一个会累积延迟(`:966-1043`)。
- **`IMMNotificationClient` + role 区分**:mic 用 `eCommunications`,loopback 用 `eConsole`(`obs-studio/plugins/win-wasapi/wasapi-notify.cpp:48-55`)。
- **时间戳分两路**:mic 默认 `useDeviceTiming=false`(便宜 USB mic 的 device ts 抖),用 `QPC_now - frames/sr`;loopback 用 device timing(`:1022-1027, 1333, 1339`)。

### 3.2 音频图拓扑与线程模型(真理来源:pipewire/pulseaudio)
- **四流→三流简化拓扑**:我们实际是 PipeWire `monitor.mode` 简化版 = WASAPI loopback + WASAPI mic + 虚拟麦克风 endpoint。
- **单 Processing 线程持有 AEC state**:RenderLoopback/MicCapture 只往 SPSC ring 写,Processing 只读(PulseAudio 用 asyncmsgq 达到同样效果,`pulseaudio/src/modules/echo-cancel/module-echo-cancel.c:980-1007`)。
- **引擎 activate/deactivate 状态机**:capture+render 双流都 STREAMING 才 activate AEC,任一 PAUSED 立刻 deactivate + reset_buffers(防长 silence 后发散,`pipewire/src/modules/module-echo-cancel.c:596-631`)。
- **ring xrun 主动 drop + `request_resync`**:drop 老数据会损坏收敛,之后必须 resync(`pulseaudio/src/modules/echo-cancel/module-echo-cancel.c:542-554,997`)。
- **engine 接口形状**:参考 PipeWire `spa_audio_aec_methods`(`init/init2/run`,deinterleaved float**,**已删掉 set_drift**)而非 PulseAudio 的 `set_drift` 回调——PipeWire 的演进就是对 PulseAudio 经验的投票(`pipewire/spa/plugins/aec/aec-webrtc.cpp:358-364`)。

### 3.3 延迟对齐与时钟漂移(真理来源:PulseAudio 公式 + AEC3 接口)
- **两层对齐**:粗对齐(byte-skip,>5ms 容差立即跳,留 +10 帧 safety)+ 细对齐(rubato `set_resample_ratio_relative`,1s watchdog,ramp 50ms 防 sinc 抖动)。
- **mic 主时钟铁律**:fractional resampler 只放 RenderLoopback→Processing,**mic 路径绝不重采样**(PulseAudio `pulseaudio/src/modules/echo-cancel/module-echo-cancel.c:393` 只动 sink;PipeWire 同)。
- **buffer_delay 预热闸门**:启动期 `current_delay < buffer_delay` 时输出静音不跑 AEC,给 AEC grace period 防冷启动错学(`pipewire/src/modules/module-echo-cancel.c:407-433`)。
- **resampler 群延迟要算进 stream_delay**(`pulseaudio/src/modules/echo-cancel/module-echo-cancel.c:426,464`)。

### 3.4 AEC 算法层(真理来源:AEC3)
- **stereo 分两路处理**:delay 估计用 `AlignmentMixer` 强制 mono(`prefer_first_two_channels=true`, `activity_power_threshold=10000`),adaptive filter + suppressor 保留 stereo(`webrtc-aec3-src/aec3/alignment_mixer.h:25-27`)。**IAecEngine 要暴露两个独立选项**,不要捆成单一 stereo 开关。
- **双讲门控 `DominantNearendDetector`**:per-channel 低频 [bin1..15] 的 ENR/SNR 双阈值 + 滞回(enter 慢/stay 久/echo 突涨立即退出),OR 合并(`webrtc-aec3-src/aec3/dominant_nearend_detector.cc:32-76`)。作为 IAecEngine 统一 DTD 输出事实标准。
- **多通道默认配置** `CreateDefaultMultichannelConfig()`(coarse 11 blocks/rate 0.95):stereo 直接用,别自己拍参数(`webrtc-aec3-src/api-audio/echo_canceller3_config.cc:288-301`)。
- **filter tail 无硬上限**:`Validate()` 只 FloorLimit(1),可改到 64 blocks(256ms);sonora README 给出的 48k full pipeline benchmark 是 13.3µs/10ms 帧,调大 tail 至少有性能试验空间(`sonora/BENCHMARKS.md`)。

### 3.5 Neural 残余抑制(真理来源:AEC3 集成层)
- **neural REE「始终运行」**:即便输出被忽略也要每 block 喂数据保持 LSTM state 一致(`webrtc-aec3-src/aec3/residual_echo_estimator.cc:214-216`)。任何 stateful neural 后端不能按需启停。
- **双 mask 输出 + DTD 二值选择**:`dominant_nearend=true` 用 unbounded(保近端),false 用 bounded(狠压残余)(`webrtc-aec3-src/aec3/neural_residual_echo_estimator/neural_residual_echo_estimator_impl.cc:541-566`)。DTD 是 neural REE 的 conditioning input,不能让 neural 独立判 DTD。
- **neural 激活时关 coarse filter + 切 suppressor tuning**(`webrtc-aec3-src/aec3/echo_remover.cc:431,513-520`)。
- **「preprocess 必须在 AEC 之后」铁律**(SpeexDSP 时代经验,neural 时代仍成立):neural residual 必须在线性 AEC 输出之后,绝不并行或放前面(`pulseaudio/src/modules/echo-cancel/speex.c:218-225`「This is not a mistake!」)。
- **DTLN Model_2 是天然 time-domain residual refiner**,可只拆它接「AEC3 e + 真实 ref」,不必整端到端用(`DTLN_AEC-wrapper/DTLN_AEC/DTLN_AEC.cpp:395-397`)。

### 3.6 虚拟麦克风与驱动(真理来源:Windows-driver-samples)
- **WaveRT 零拷贝**:`AllocatePagesForMdl + MapAllocatedPages(MmCached)` 让 driver 与 user-mode 共享同一物理页,**不需要 user-kernel ring 双拷贝**(`Windows-driver-samples/audio/simpleaudiosample/Source/Main/minwavertstream.cpp:499-525`)。
- **driver 声明 `AUDIO_EFFECT_TYPE_ACOUSTIC_ECHO_CANCELLATION`**:让 Discord/VRChat 看到「此 endpoint 已自带 AEC」从而关掉自己的 AEC,避免双 AEC 互打(`Windows-driver-samples/audio/sysvad/EndpointsCommon/minwavert.cpp:1924-1959`)。
- **不要声明 `MIC_ARRAY_GEOMETRY`**:否则 Windows 自动插入 Voice Capture DSP(AEC/NS/BF)与我们互打(反例:`Virtual-Audio-Driver/Source/Filters/micarraytopo.cpp:373-419`)。
- **EndpointFormFactor=Headset + friendly name 含 "Communications"** 让 Discord 优先列出。
- **三段式 componentized INF**(base+extension+APO,Win10 1809+ 强制):套 sysvad 模板,保留 `PETrust=true`/`DRMLevel=1300`。
- **QPC ↔ device position 用整数余数累加**(`hnsElapsedTimeCarryForward`),不用浮点乘除,否则长跑漂移(`:1302-1388`)。

### 3.6b 虚拟麦克风「不写驱动」MVP 路线(真理来源:MicYou,Kotlin/GPLv3,只学不抄)
> MicYou 本身非 AEC(单端 dereverb+denoiser,无 reference),但它把「装现成虚拟声卡 + 自动配置默认设备」这条 MVP 路线做到了开源里最完整,正好补我们最大的自研缺口的 MVP 版。

- **Windows 全自动 VB-Cable**(`MicYou/composeApp/src/jvmMain/kotlin/com/lanrhyme/micyou/platform/VBCableManager.kt`,856 行):下载安装包(`:769`)→ `Start-Process -Verb RunAs` UAC 提权装(`:633`)→ nirsoft **SoundVolumeView** 设默认录音设备/采样格式(`:301,323`)→ 缺工具时直接读写注册表 `HKLM\...\MMDevices\Audio`(`:412-465`)→ 双重检测防幽灵设备(JavaSound + `reg query ...\Services\VB-Cable`,`:77`)→ UAC 拒绝/超时分类 + 卸载回滚(`:735-757,847`)。Rust 直译:`reqwest` + `ShellExecuteW(runas)` + `winreg`。
- **macOS BlackHole 切换**(`BlackHoleManager.kt`):`SwitchAudioSource -f json` 枚举/切换(`:42`),正则 `BlackHole\s*\d*ch` 兼容 2/16ch,保存/恢复原输入设备(`:110-127`)。
- **Linux PipeWire 零驱动虚拟麦**(`PipeWireManager.kt`,若日后做):`pw-cli create-node ... support.null-audio-sink` + `pw-loopback` 把 monitor 变 source(`:182-218`)。
- **跨平台抽象模式**:`AudioEngine.kt` `expect/actual` + `PlatformAdaptor.kt` 的 `usesSystemAudioSinkForVirtualOutput` 能力开关 → 直译成我们的 `AudioSink` trait + cfg 分发 + `VirtualDeviceManager{install/set_default/restore}`。

### 3.7 工程组织与产品化(真理来源:project-raven + sonora-ffi)
- **AEC 健康监控 bypass 状态机**(Recall.ai 同款实战阈值,直接当默认 SLA):drift>200ms bypass / drift<100ms 才 reenable / 5s holdoff / 10 overflows/2s / 200 empty pulls / health check 2s。**bypass 期仍继续喂数据让 background filter 暗中收敛**(`project-raven/src/main/systemAudioNative.ts:115-123,373-378`)。
- **panic 不跨 FFI**:`ffi_guard!` + `catch_unwind` 三层保护,Rust panic 返回错误码而非 abort(`sonora/crates/sonora-ffi/src/panic_guard.rs`)。
- **IAecEngine C ABI** 取三家之长:LocalVQE opaque handle + options builder + Maxine string selector + typed setter。

---

## 4. 被忽视的高价值点(本次探索的核心增量)

> 这些是对照「现有调研基线」逐条找出的、基线没注意到或判断有误的点。按影响排序。

### 4.1 架构方向级(改变技术决策)
1. **AEC3 内置 neural REE 接口**(见执行摘要 #1)——把 neural 从串联改深度集成,显著降低接口和时序对齐成本。`webrtc-aec3-src` + `LocalVQE` 双向印证。
2. **sonora 把 classical AEC3/APM 在 Windows 落地从「数月 GN 折腾」降到 Rust+FFI 路线**(见执行摘要 #2)。这条直接改变引擎选型与 Phase 3 路线;但若目标包含 2025 neural REE,当前本地 `sonora-aec3` 仍缺 port,不能直接替代 official C++。
3. **drift ppm 闭环在当前本地参考集合中未见 production 实现 = 我们的创新空间**(4 处死代码互证)。应把这块作为单独可验证模块做出工程证据。
4. **按 DTD 切换 neural 拓扑的产品策略**:LocalVQE 1024ms echo 窗单兵就能覆盖外放长 tail,但 double-talk ERLE 仅 8.5dB;应在「far-end-only 用 LocalVQE single」与「double-talk 用 AEC3+LocalVQE residual」之间按 dominant_nearend 切换(`LocalVQE README:98` + `DT ERLE 8.5dB`)。
5. **Win11 AEC APO 是 OS 官方接口路线而非纯「高级路线」**:aux loopback 入口能把一部分对齐/设备恢复问题交给系统接口,应升级为 Win11 主路径之一,与用户态虚拟麦并行。但 `Windows-driver-samples/audio/sysvad/APO/AecApo` 只是模板,算法与实时注入策略仍需自研。

### 4.2 纠正基线错误判断(必须改文档)
6. **`Virtual-Audio-Driver` 公开版 mic 永远输出静音**(`WriteBytes`=`RtlZeroMemory`),基线 3/5 → 实际可用度 0;「custom builds 可用 named pipes/shared memory」经全仓 grep 证伪(付费内容)。
7. **`project-raven` 的 `project-raven/src/native/webrtc-aec/` C API 未接入主路径**:主应用实际加载 GStreamer `webrtcdsp` addon,`project-raven/src/native/webrtc-aec/` 是遗留实验且本地只见 macOS 预构建痕迹;`stats.diverged` 语义还是错的(`stream_has_echo()` ≠ filter diverged)。连子目录 README 都虚标 M124/AEC3。
8. **`aec3-rs` 与 `sonora` 不是同一 milestone,但 `sonora` 也尚未覆盖 2025 neural REE**:aec3-rs 用旧 main/shadow 命名,sonora 用新 refined/coarse;不过 `sonora/crates/sonora-aec3/src/residual_echo_estimator.rs:179` 明确 `NeuralResidualEchoEstimator is skipped (not ported)`。
9. **DTLN-aec 是 32ms window / 8ms hop,不是单纯 8ms API frame**;wrapper 以 512 样本块进入 `Process`,内部跑 4 个 128-sample hop,叠在 AEC3 后要按窗口和 buffering 重算延迟预算(`DTLN_AEC-wrapper/DTLN_AEC/DTLN_AEC.cpp:19-20,321-419`)。
10. **TSPNN 的 ONNX 是 AECMOS 评测器,不是 TSPNN AEC 模型**;基线「无可下载 checkpoint」判断反了——拿到的是一份现成评测 harness(`TSPNN/eval/eval.py`)。
11. **NKF 模型只有 28KB**(candidate 池最小),小到可塞进 hot path 作 SpeexDSP MVP 的零成本增量(`NKF-AEC/src/nkf_epoch70.pt`);基线「3/5 算法参考」偏乐观——无 train 脚本,自家重训成本高于 DTLN。
12. **当前本地 EchoFree 副本只有 README,代码/模型未释出**,按工作区证据应从「占位跟踪」直接弃用。

### 4.3 工程细节级(踩坑预警)
13. **AEC3 内部 block 是 4ms 不是 10ms**;48k 输入下主滤波只跑 band0(0-8kHz),高频两 band 走共享 gain——**外放音箱齿音/共振高频残余 echo 在当前现成模型/参考方案中仍缺强证据**,是产品差异化点(`webrtc-aec3-src/aec3/aec3_common.h:35,57-58`)。
14. **AEC3 `SetAudioBufferDelay` 内部按 16kHz round**,传 delay 必须是 4ms 倍数否则被静默截断(`webrtc-aec3-src/aec3/render_delay_buffer.cc:338-342`)。
15. **`AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY`(0x1)当前本地参考仓库未见完整处理**——这是 audio engine 知道丢了样本的关键信号,对 AEC 应触发软重置或重对齐(OBS 只处理 SILENT/TIMESTAMP_ERROR)。
16. **默认设备 format/channel 运行时变化无人处理**:用户在系统设置改 stereo→7.1,OBS `OnPropertyValueChanged` 直接 return S_OK,会让 AEC 崩(`obs-studio/plugins/win-wasapi/wasapi-notify.cpp:46`)。
17. **SpeexDSP 默认采样率 8000 写死**,init 后必须立刻 `SET_SAMPLING_RATE` 否则 beta/notch 全偏 8k 假设(`speexdsp/libspeexdsp/mdf.c:427`);很多教程直接抄 8000。
18. **OBS MMCSS 用 "Audio" 不是 "Pro Audio"** + 500ms 大缓冲——OBS 设计偏稳定;我们 processing 线程上 Pro Audio,capture 线程 "Audio" 可能够,别无脑全 Pro Audio。
19. **`speex_decorrelate`(scal.c)是 stereo 的第三条路**:基线只在「AEC3 multichannel vs 早 mono downmix」二选一,漏了「前置去相关(strength 30-50)仍 stereo 喂 AEC」(`speexdsp/libspeexdsp/scal.c:85-280`)。
20. **TFLite ModelRunner 强制单线程**(`SetNumThreads(1)`)——直接反驳 LocalVQE 标榜的「4 线程 p50 3.21ms」,production 集成要重新 benchmark(`webrtc-aec3-src/aec3/neural_residual_echo_estimator/neural_residual_echo_estimator_impl.cc:411-415`)。

### 4.4 license/合规级
21. **WebRTC PATENTS + LICENSE 不在 webrtc-aec3-src 瘦切片里**,所有源码头注释引用它们,必须单独下载嵌入(合规清单遗漏)。
22. **MS-PL 派生边界要单独处理**:从 sysvad/simpleaudiosample 派生的源文件不应笼统当 MIT/闭源自有代码处理,至少要保留对应许可与 NOTICE 边界。
23. **webrtc-audioprocessing 把完整 protobuf(含各语言生成器)静态链入**,死代码膨胀几十 MB,可删到 1/5(`webrtc-audioprocessing/sources.cmake:293-460`)。
24. **DRMLevel=1300 + PETrust=true 会抬高签名/合规确认成本**,普通 EV 证书 + attestation 是否足够需单独验证(`Virtual-Audio-Driver/Source/Main/VirtualAudioDriver.inx:16-23`)。

---

## 5. 横切维度结论(7 维)

### 维度 1 · Windows 音频采集鲁棒性
- **最佳来源**:obs-studio/plugins/win-wasapi(GPL,clean-room)。反例:project-raven 轮询采集。
- **可直接用**:event-driven 主循环、SILENT 双路径、DEVICE_INVALIDATED 静默、七 Event 状态机、while-loop 排空、CoTaskMemPtr RAII。
- **必须自研的 gap**:DATA_DISCONTINUITY 处理、format/channel 运行时变化、reconnect 指数退避(OBS 固定 3s 不可接受)、IAudioClient3 5ms period underrun 恢复、Modern Standby 唤醒恢复、多 client 虚拟 mic fan-out。
- **建议**:Phase 1 clean-room 重写 OBS 事件驱动骨架为 Rust;Phase 2 补 OBS 没做的硬 gap;BUFFER_TIME 用 100ms(OBS 500ms 偏大)。

### 维度 2 · 时钟漂移补偿
- **最佳来源**:`pulseaudio/src/modules/echo-cancel/module-echo-cancel.c`(完整 resync + watchdog 状态机)。三方死代码互证「drift 必须应用层做」。
- **可直接用**:`calc_diff` 公式、参数表(5ms/1s/+10帧/±10%)、`apply_diff_time` byte-skip、rubato `SincFixedIn` 参数、speex `set_rate_frac`、AEC3 `use_external_delay_estimator`。
- **必须自研的 gap**:真正 ppm 级闭环 fractional resampling(本地参考集合未见完整实现)、IAudioClock/QPC/DevicePosition 三时间源退化链、Modern Standby 时钟跳变、USB DAC ASRC 影响、bypass 重收敛策略。
- **建议**:「PulseAudio 公式 + rubato 实现 + AEC3 外部 delay」三段式;mic 主时钟;实现 binary drift trace 调试基础设施。Phase 1 只做粗对齐,Phase 2 上 rubato 细对齐。

### 维度 3 · 延迟估计与回声路径对齐
- **最佳来源**:AEC3(render_delay_buffer + matched_filter)+ PulseAudio calc_diff + PipeWire 双层 buffer_delay。
- **可直接用**:`SetAudioBufferDelay` + `AlignFromExternalDelay`(自动补跨线程飞行 block)、matched_filter 152ms 上限计算、neural delay headroom 公式、LocalVQE obs-plugin 三态机(lag/lead/喂零)。
- **必须自研的 gap**:WASAPI 端到端 timestamp budget 实测、启动收敛期「静音灌热 N 秒」标准做法、虚拟麦→AEC→loopback 累积延迟链、多 render endpoint 切换延迟重估。
- **建议**:四层(时间戳采集 → 粗对齐+静态补偿 → AEC 引擎对接 external delay → drift 闭环);external delay 模式必须配独立 GCC-PHAT sanity check。

### 维度 4 · Neural 残余/非线性抑制
- **最佳来源**:AEC3 2025 `NeuralResidualEchoEstimator` 深度集成层 + LocalVQE(模型源)。
- **可直接用**:抽象基类 + AdjustConfig tuning、double-mask 范式、mask 频率下采样公式、LSTM state 衰减、LocalVQE C API、DTLN Model_2、NKF 28KB 权重、AECMOS harness。
- **必须自研的 gap**:Windows 上 AEC3+neural REE 真实编译路径(第一道硬墙)、stereo render 喂 neural 策略、48kHz 全带模型、production 单线程 benchmark、neural 健康 watchdog(超时/NaN/crash 降级)、外放专属训练数据。
- **建议(主线决策)**:**AEC3 主消后必须叠 neural,但走深度集成而非串联,且分阶段**。MVP 不上 neural(SpeexDSP preprocess 已够);Phase 2/3 若走 official C++ 写 `LocalVqeNeuralReeAdapter : public NeuralResidualEchoEstimator`;若走 `sonora`,需先补 neural REE port。强制单线程 + 独立 MMCSS 线程;强制 `use_external_delay_estimator`。

### 维度 5 · 立体声参考与双讲门控
- **最佳来源**:AEC3(本地参考集合中最完整的 stereo+DTD 四级管线)。其他引擎只能配合不能替代。
- **可直接用**:AlignmentMixer 三模式、DominantNearendDetector 整段算法、`CreateDefaultMultichannelConfig`、MultiChannelContentDetector、speex_decorrelate 前置去相关。
- **必须自研的 gap**:neural 后处理在 stereo render 上的处理(本地可改的开源模型主要是 mono ref)、DTD 作 IAecEngine 一等输出契约、Discord/VRChat loopback 真实通道相关性实测、channel-count 运行时变化 graceful reinit。
- **建议**:IAecEngine day-1 三 spec 分离(mic mono / render stereo / out mono);stereo 分 delay_mixing + filter_path 两层独立配置;DTD 统一四态输出;neural REE 接收 AEC3 的 DTD 作 conditioning;驱动侧删 MIC_ARRAY_GEOMETRY + 声明 AEC effect 防双 AEC。

### 维度 6 · 虚拟麦克风输出
- **最佳来源**:`simpleaudiosample`(当前最可取的虚拟 mic 起点,比 sysvad 纯、比 Virtual-Audio-Driver 完整)。注入点 = `WriteBytes`。
- **可直接用**:WaveRT 零拷贝 DMA、QPC 时戳簿记、sysvad 三段式 INF、AEC effect 声明、AecApo 模板、VB-Cable(MVP)。
- **必须自研的 gap(最大)**:user→kernel 实时音频通道(当前本地参考集合留白,2-8 人周)、EV 签名 + Partner Center attestation、多 client 并发(`MAX_INPUT_STREAMS=1`)、Win11 ARM64、Discord/VRChat 真机行为、虚拟麦+APO 双路 fallback 编排。
- **建议**:阶段 1 VB-Cable 兜底;阶段 2 基于 simpleaudiosample 派生 capture-only 虚拟 mic(`WriteBytes` 换 shared-memory+KEVENT memcpy,缩小 HLK 范围);阶段 3 并行 AEC APO;≤2 人团队认真评估买 TVirtAudio SDK / MikeTheTech 定制把这块外包。**绝对避免** fork Virtual-Audio-Driver 公开版做业务。

### 维度 7 · 构建集成与许可证
- **最佳来源**:webrtc-audioprocessing 4 文件 build harness(壳 Apache-2.0)+ LocalVQE/sonora-ffi C API。
- **可直接用**:4 文件 CMake harness、LocalVQE C 头、sonora-ffi 双 crate-type+cbindgen、Maxine string selector、Speex `speexdsp/win32/libspeexdsp.def`。
- **必须自研的 gap**:cargo+CMake 多后端混编模板、单文件 signed installer、cpu dispatch 多 DLL、EV+Authenticode+attestation CI、GPL 隔离边界文档、模型供应链安全、统一 THIRD_PARTY_NOTICES。
- **建议**:主 build = Rust/cargo,C/C++ 后端走 cmake crate;WebRTC 走 fork harness + vendor M124 子树(脱 depot_tools);IAecEngine C ABI 取三家之长;严格 license 矩阵(GPL 仅 clean-room,WebRTC PATENTS 法务确认,MS-PL 源码层保留)。

---

## 6. 逐仓库速判表

| 仓库 | 定位 | Verdict | 一句话理由 |
|---|---|---|---|
| **webrtc-aec3-src** | 成品核心引擎源码 | **直接用**(配 wrapper) | 线性 AEC 行业最成熟 + 2025 neural REE 深度集成接口 + 真·外部 delay;主要阻塞是独立编译 |
| **sonora** | AEC3 纯 Rust 移植 | **直接用(classical AEC3 主线候选)** | M145 移植 + Windows 普通 CI + 现成 C API;2400+ C++ validation 非 Windows,且 neural REE 未 ported |
| **LocalVQE** | neural AEC+NS+dereverb | **直接用**(neural 后端) | Apache-2.0 + C API + Win 自发现 + 1024ms 窗 + 16ms 延迟;集成成本最低的 neural 路径 |
| **speexdsp** | MVP 引擎 + 基础设施 | **直接用 + 参考** | MDF AEC + resampler(drift)+ decorrelate + preprocess 五件套;成品核心上限低于 AEC3 |
| **webrtc-audioprocessing** | AEC3 构建脚手架 | **直接用**(构建) | 4 文件把 AEC3 编成 MSVC 静态库,省 1-2 周;但「脱 GN」半真 |
| **AEC-Challenge** | 客观评测 + 对照基线 | **直接用**(AECMOS_local) | 48k fullband AECMOS 本地推理 echo_mos+deg_mos;升至 4/5 作 CI 回归。DEC baseline 作对照 |
| **Windows-driver-samples** | 虚拟麦克风起点 | **直接用 + 大量参考** | simpleaudiosample 派生骨架 + sysvad/APO/AecApo 官方 AEC APO 模板;但 AecApo 算法为空且样例有 typo |
| **DTLN-aec(+wrapper)** | neural fallback/评测 | **参考 + 占位** | Win x64 现成 DLL 做 Phase 0 评测;16k mono + 32ms window/8ms hop,优先级低于 LocalVQE |
| **pipewire+pulseaudio** | 架构 + drift 真理来源 | **参考(强 5/5)** | 四流拓扑 + calc_diff 公式 + 接口形状可 clean-room 复刻;Linux-only 不能 link |
| **obs-studio** | WASAPI 采集真理来源 | **参考(GPL 只学)** | 本地参考集合中最完整的生产级采集鲁棒性来源;zero LoC 复制,clean-room 重写 |
| **NKF-AEC** | 极轻量 neural 算法 | **参考设计** | 28KB hybrid Kalman + GCC-PHAT 可借鉴;16k/64ms,不作成品后端 |
| **Maxine-AFX-SDK** | RTX GPU AEC 后端 | **参考 + 可选后端** | AEC effect 使用 mic/far reference 两路输入;通道数由 SDK 参数返回,无诊断 + EULA + Tensor Core 限制,非默认。基线 3.5/5 偏乐观→2.5-3/5 |
| **aec3-rs** | AEC3 旧版 Rust 移植 | **参考设计** | graph DAG + discontinuity 标志框架值得借鉴;milestone 偏旧 + 无 FFI,不作主线 |
| **project-raven** | 端到端参考 | **参考 + 部分骨架** | 健康监控 bypass 阈值表 + IAecEngine 形状是真价值;主路径依赖 GStreamer `webrtcdsp`,自带 `project-raven/src/native/webrtc-aec/` 是未接入的旧 APM 路径 |
| **Virtual-Audio-Driver** | 虚拟麦克风模板 | **参考(降权)** | 公开版恒输出静音、无 IPC 通道;只剩 INF/ARM64/build 模板价值。基线 3/5 高估 |
| **SynchronousAudioRouter** | DAW 路由驱动 | **参考(降权 2.5/5)** | 内核虚拟驱动+用户态零拷贝架构样板;GPLv3 + Secure Boot 不支持,不 fork |
| **research-models** | 学术算法/评测 | **参考 + 评测复用** | TSPNN eval harness + DEPM ICCRN 结构/checkpoint 可借鉴但推理脚本未产品化;**EchoFree 弃用**(本地代码未释出) |
| **MicYou** | 虚拟麦安装/配置 UX | **参考设计(中-高)** | 非 AEC(单端 dereverb+denoiser);但「不写驱动、自动化 VB-Cable/BlackHole/PipeWire 安装+设默认+回滚」是最完整开源实现(`VBCableManager.kt:633-847`),补我们 MVP 虚拟麦缺口。GPLv3 只学不抄 |

---

## 7. 对现有调研文档的修正/补充建议

> 指向 `research/windows_aec_research.md` 的章节。

1. **§3.3 / §9(Neural 架构)**:新增「AEC3 内置 `NeuralResidualEchoEstimator` 深度集成接口」——把主要 neural 候选从「AEC3 之外串联」重定位为「AEC3 内 adapter 子类」。这是架构层的重写,影响 §9.4 引擎抽象。
2. **§3.2 / §4.1(引擎选型)**:`sonora` 评级 2.5/5 → 4.0/5,新增为 classical WebRtcAec3Engine 的 Rust 主线实现路径,与 webrtc-audioprocessing CMake wrapper 并列。明确 caveat:Windows 普通 CI 不等于 Windows C++ reference validation,且当前 `sonora-aec3` 未 port neural REE。`aec3-rs` 标注 milestone 偏旧 + 无 FFI。
3. **§7(虚拟麦克风)**:(a) `Virtual-Audio-Driver` 公开版降到「仅 INF/工程模板」,明确「mic 恒静音、无 IPC」;(b) 虚拟麦克风起点改为 `simpleaudiosample`;(c) 新增「Win11 AEC APO(sysvad/APO/AecApo)」为官方接口路线;(d) 新增「driver 声明 AEC effect / 不声明 mic-array 防双 AEC」。
4. **§6(drift)**:把「PulseAudio 把 drift 留给应用层」修正为更准确的「PulseAudio 既没做应用层 fractional resample 也没做内部 set_drift,只做 drop-based resync;ppm 闭环在当前本地参考集合中未见 production 实现」。新增 4 处死代码互证 + calc_diff 公式 + 参数表。
5. **§5(WASAPI 采集)**:补 obs-studio 的具体技巧清单(SILENT 双路径、DEVICE_INVALIDATED 静默、ClearBuffer、时间戳分两路、while 排空);明确 project-raven 采集为反例。
6. **§4.1(AEC3 细节)**:补正「内部 4ms block」「48k 只跑 band0,高频无 neural 建模」「matched_filter 152ms 上限」「clockdrift_detector 是 stub 只分类」「filter tail 无硬上限」「SetAudioBufferDelay 按 16k round」。
7. **§13(风险)/§12(验证)**:新增 `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY` 处理为已识别风险;把 AECMOS_local(48k)+ DTLN/LocalVQE/NKF 离线对比 + 场景五分类(含 with_movement)定为 Phase 0 评测矩阵。
8. **§14(资源清单)**:逐项纠正 project-raven(`project-raven/src/native/webrtc-aec/` 未接入主路径)、DTLN(32ms window/8ms hop)、TSPNN(ONNX 是评测器)、NKF(28KB/无 train)、EchoFree(按本地副本弃用)。

---

## 8. 建议的下一步深扒/验证

按优先级(均为基线 open question 或本次新发现的硬墙):

1. **【硬墙】Windows 上编出含 `NeuralResidualEchoEstimator` 的 AEC3**:webrtc-audioprocessing 是 M124(早于 neural REE 抽象),需 bump milestone + 解决 TFLite/protobuf/pffft/flatbuffers 完整依赖。若选择 `sonora`,第一步是补 port neural REE。这是 neural 主线的第一道墙。
2. **【选型对照】sonora vs webrtc-audioprocessing**:实测 sonora 在 Windows MSVC 下与官方 C++ 的数值一致性 + 长时稳定性 + stress test(当前 Windows 只覆盖普通 cargo test;2400+ C++ reference validation 不在 Windows)。若 classical AEC3 过关,可省掉一大块 GN/depot_tools 路线。
3. **【Phase 0 评测 harness】**:用 AECMOS_local(48k)+ DTLN audio_samples ground-truth,**强制单线程**对比 SpeexDSP / AEC3 / LocalVQE v1.2/v1.3 / DTLN / NKF 在单 mic+render 场景的 ERLE / echo_mos / deg_mos。
4. **【虚拟麦克风 PoC】**:基于 simpleaudiosample 派生,把 `WriteBytes` 换成 shared-memory + KEVENT 喂音频,Win10/11 各跑一次 Discord/VRChat 真机(验证 AEC effect 声明是否让 Discord 关自己的 AEC、format=Headset 是否优先列出、设备热切换是否无缝)。
5. **【drift 公式离线验证】**:Python 离线 pipeline 实现 calc_diff + apply_diff_time + 简易 SRC,用已有 mic/ref wav 验证公式,对比 AEC3 `use_external_delay_estimator` vs 内部 matched_filter 的 ERLE。
6. **【WASAPI timestamp budget 实测】**:测 `GetCurrentPadding`/`GetPosition`/QPC 在 shared-mode 5/10ms period 下的真实 jitter 分布、IAudioClient3 强行 5ms 的 underrun 概率(当前本地参考集合无数据)。
7. **【stereo 实测】**:对 Discord/VRChat loopback 实测通道相关性,决定 MultiChannelContentDetector 的 `detection_threshold`(默认 0.0 偏激进,易误判伪 stereo)。

---

*报告经 2026-06-05 源码复核更新;关键结论应继续以本地 `reference_repos/<repo>` 源码与 `file:line` 证据为准。*
