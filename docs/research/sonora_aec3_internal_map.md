# sonora-aec3 内部地图(集成前调研)

> 目的:在决定"如何把 sonora-aec3 集成进 Echoless"之前,摸清它的内部结构、可调参数、
> 延迟/漂移链路、已知坑点。**研究先行,不盲目搬代码。**
>
> 源码版本:`dignifiedquire/sonora` rev `aacadf0`(= 我们 `Cargo.lock` 锁定的 commit)。
> 本地路径:`~/.cargo/git/checkouts/sonora-c0cc0c3c8411109a/aacadf0/crates/sonora-aec3`。
> 下文行号引用均以该副本为准(`crates/sonora-aec3/src/...` 省略前缀写作 `文件:行`)。
>
> 调研方法:4 个只读 Explore agent 分头深挖(结构/config/延迟/滤波+坑点),本文为汇总核实稿。

---

## 0. 一句话结论

sonora-aec3 是 WebRTC AEC3 的**逐文件 1:1 Rust 端口**,架构忠实、能用,但:

- **真正的"调参旋钮"(tail 长度、抑制激进度、延迟搜索范围)全在 `EchoCanceller3Config`,
  而高层 `sonora` crate 把它们 100% 锁死,只放行 `transparent_mode` 一个字段。**
- **它假设输入是 i16 量程的浮点(±32768)**,不是 ±1.0;高层 API 帮我们自动转,绕过高层就得自己转。
- **几个对外统计是"假的"**:`residual_echo_likelihood` / `divergent_filter_fraction` 在 aec3 crate 里
  **根本没实现**(我们现在读到的值另有来源,见 §7,需复核)。
- **延迟**:内部 matched filter 粗对齐预算 ≈ **608ms**(不是主文档说的 152ms);`set_stream_delay_ms`
  默认几乎不起作用;时钟漂移**只检测不补偿**。

→ 集成结论见 §9。**核心判断:要拿到"上限",必须能改 `EchoCanceller3Config`,这就意味着
要么 fork 高层 sonora 开个 config 注入口,要么绕过高层直接驱动 `BlockProcessor`。**

---

## 1. crate 结构与公开入口

### 1.1 真正的入口是 `BlockProcessor`(不是 EchoCanceller3)

`EchoCanceller3` 这一层在**上层 `sonora` crate**里、且是 `pub(crate)`。sonora-aec3 对外暴露的处理入口是
**`BlockProcessor`**,以 64 样本 block 为粒度。

- `pub struct BlockProcessor` — `block_processor.rs:21-33`
- 构造:
  - `new(config: &EchoCanceller3Config, sample_rate_hz, num_render_channels, num_capture_channels)` — `block_processor.rs:47-61`
  - `with_backend(SimdBackend, ...)` — `block_processor.rs:64-106`(显式选 SIMD,但只透传给 Subtractor,见坑 #12)
- 主要方法:
  - `buffer_render(&Block)` — `block_processor.rs:215-224`(远端入队,**必须先于** capture)
  - `process_capture(echo_path_gain_change, capture_saturation, linear_output: Option<&mut Block>, capture: &mut Block)` — `block_processor.rs:124-212`(近端就地消回声)
  - `get_metrics() -> BlockProcessorMetricsOutput` — `block_processor.rs:109-116`(只有 ERL / ERLE / delay_ms)
  - `set_audio_buffer_delay(i32)` — `block_processor.rs:119-121`(转发到 render delay buffer,默认形同摆设,见 §5.3)
  - `update_echo_leakage_status`、`set_capture_output_usage`
- **没有 public `reset()`**(坑 #4):重置只能重建实例,或靠内部 echo-path-change。

辅助公开类型:`Block`(`block.rs:14-77`)、`FrameBlocker`/`BlockFramer`、`EchoCanceller3Config`(`config.rs`)。

### 1.2 处理域硬约束(改 config 也改不到)

| 量 | 值 | 出处 |
|---|---|---|
| 内核采样率 | **固定 16 kHz** | `common.rs:31`(64×250=16000) |
| block 大小 | **64 样本 = 4 ms** | `common.rs:29,31` |
| FFT 长度 | 128 | `common.rs:15` |
| 分带 | `num_bands = sr/16000`(16k→1, 32k→2, 48k→3) | `common.rs:41-43` |
| 合法采样率 | **仅 16k/32k/48k**(其它 `debug_assert!` panic) | `common.rs:46-48` |
| 线性 AEC 实际工作带 | **只在 band 0(0–8kHz)**;高带用标量增益 | `echo_remover.rs:415-422` |

> 输入帧:上层以 10ms 帧 / 5ms sub-frame(80 样本)进,`FrameBlocker` 拼成 64 样本 block;
> 48k/32k 先经三带分析滤波拆成 16k 子带再喂 AEC,出口 `merge_frequency_bands` 合回。

### 1.3 模块速览(按职责)

- **数据容器**:`block.rs` `[bands][channels][64]`、`block_buffer.rs`、`frame_blocker.rs`/`block_framer.rs`(80↔64 转换)、`config.rs`、`common.rs`。
- **顶层编排**:`block_processor.rs`(串 render 缓冲→延迟→echo remover)。
- **Render 路径**:`render_delay_buffer.rs`(三套并行缓冲+低速副本)、`render_buffer.rs`(只读视图)、`render_delay_controller.rs`、`render_signal_analyzer.rs`、`downsampled_render_buffer.rs`、`decimator.rs`、`alignment_mixer.rs`、`multi_channel_content_detector.rs`。
- **延迟估计**:`matched_filter.rs`(+ `sse2/avx2/neon`)、`matched_filter_lag_aggregator.rs`、`echo_path_delay_estimator.rs`、`delay_estimate.rs`、`clockdrift_detector.rs`。
- **线性自适应滤波(STFT 域)**:`aec3_fft.rs`、`fft_data.rs`、`fft_buffer.rs`/`spectrum_buffer.rs`、`adaptive_fir_filter.rs`(频域分块 FIR)、`coarse/refined_filter_update_gain.rs`、`subtractor.rs`(coarse+refined 双滤波,产残差 e)、`subtractor_output*.rs`、`filter_analyzer.rs`。
- **状态/估计**:`aec_state.rs`(中心状态机)、`erl_estimator.rs`/`erle_estimator.rs`/`fullband_erle_estimator.rs`/`subband_erle_estimator.rs`/`signal_dependent_erle_estimator.rs`、`echo_audibility.rs`、`echo_path_variability.rs`、`echo_remover.rs`、`nearend_detector.rs`、`transparent_mode.rs`、`reverb_*.rs`、`residual_echo_estimator.rs`、`stationarity_estimator.rs`。
- **非线性抑制/后处理**:`suppression_gain.rs`(算频域增益 G)、`suppression_filter.rs`(应用 G + 舒适噪声 + IFFT/OLA)、`comfort_noise_generator.rs`。
- **工具**:`vector_math.rs`、`moving_average.rs`、`cascaded_biquad_filter.rs`、`circular_buffer.rs`。

### 1.4 数据流(capture 一帧)

`process_capture`(`block_processor.rs:124-212`)→ `EchoRemover::process_capture`(`echo_remover.rs:201-442`):

1. 渲染缓冲就绪检查(underrun/overrun → reset 延迟控制器)。
2. **延迟估计** `RenderDelayController::get_delay`(matched filter → lag aggregator → clockdrift),写回 `align_from_delay`。
3. 饱和检测 → echo-path-change 处理。
4. **线性自适应滤波** `Subtractor::process`:coarse+refined 频域 FIR → 残差 `e`,选优。
5. FFT(sqrt-Hanning 加窗零填充)→ `Y`、`E`、功率谱 `Y2/E2`、线性回声 `S2`。
6. `AecState::update`(滤波频响/脉冲响应/收敛度)。
7. **残余回声估计** `ResidualEchoEstimator::estimate` → `R2`(线性 `|S|²/ERLE` 或非线性 `X²·gain²` + 混响)。
8. 舒适噪声 `ComfortNoiseGenerator::compute`。
9. **抑制增益** `SuppressionGain::get_gain`(依赖 nearend/stationarity/ERLE)→ 子带增益 `g`。
10. **重建** `SuppressionFilter::apply_gain`:频域乘 g + 注舒适噪声 + IFFT + sqrt-Hanning OLA → 就地写回 `capture`。

---

## 2. config 全景 + 高层可达性对照(本调研最重要的一节)

主结构体 `EchoCanceller3Config`(`config.rs:11-37`),12 个子结构、上百字段,全是 WebRTC config 的 1:1 移植。
**无任何 feature flag / cfg 开关**——行为 100% 由这个 config 决定(`Cargo.toml:13-23` 无 `[features]`)。

### 2.1 高层 sonora 的可达性:几乎为零

- 唯一构造点 `audio_processing_impl.rs:1280-1282`:
  ```rust
  let mut config = EchoCanceller3Config::default();
  config.echo_removal_control.transparent_mode = ec.transparent_mode;  // ← 唯一透出的字段
  let multichannel_config = Some(EchoCanceller3Config::create_default_multichannel_config());
  ```
- 高层 `EchoCanceller`(`sonora/src/config.rs:175-194`)只有 2 个字段:`enforce_high_pass_filtering`(走独立 HPF 子模块,**不进 aec3 config**)+ `transparent_mode`。
- builder(`audio_processing.rs:332-388`)只有 `.config/.capture_config/.render_config/.echo_detector`,**零 AEC3 setter**。
- `set_audio_buffer_delay` 在 `echo_canceller3.rs:707` 是 `pub(crate)`,外部够不到。
- `sonora_aec3::config::EchoCanceller3Config` 在 sonora 公共 API 里**没有任何注入 hook**。

### 2.2 "想调但高层调不到"清单(= 我们的上限被锁在哪)

| 关注点 | config 路径 | 默认值 | 对外放场景的意义 |
|---|---|---|---|
| **echo tail 长度(主)** | `filter.refined.length_blocks` | **13(=52ms)** | 外放+房间混响可能要 25+ blocks(100ms);**最该调** |
| tail(影子/初始) | `filter.coarse.length_blocks` / `*_initial.length_blocks` | 13 / 12 | 同步调 |
| tail 强度比例 | `ep_strength.default_len` / `nearend_len` | 0.83 | 混响残余建模 |
| **延迟搜索范围** | `delay.num_filters` / `down_sampling_factor` | 5 / 4 → ≈608ms | 链路延迟大时要加 |
| 强制 capture 延迟 | `delay.fixed_capture_delay_samples` | 0 | 已知固定延迟时直接补 |
| 外部延迟估计开关 | `delay.use_external_delay_estimator` | false | 想让 `set_stream_delay_ms` 真生效必须开 |
| **抑制激进度(normal)** | `suppressor.normal_tuning.{mask_lf,mask_hf,max_inc_factor,max_dec_factor_lf}` | 见 §6.2 | 残余回声 vs 人声损伤的权衡 |
| **抑制激进度(nearend)** | `suppressor.nearend_tuning.*` | 见 §6.2 | 双讲时保人声 |
| 双讲检测阈值 | `suppressor.dominant_nearend_detection.*` | enr 0.25 / snr 30 / hold 50 / trigger 12 | 双讲响应快慢 |
| 线性滤波开关 | `filter.use_linear_filter` | true | 调试用 |
| 导出线性 AEC 输出 | `filter.export_linear_aec_output` | false | **给 LocalVQE 当输入的关键**(线性残差比全抑制后更适合喂神经网络) |
| echo reference 高通 | `filter.high_pass_filter_echo_reference` | false | 注意≠高层的 `enforce_high_pass_filtering` |
| ERLE 上下界 | `erle.{min,max_l,max_h}` | 1/4/1.5 | |
| 时钟漂移标记 | `echo_removal_control.{has_clock_drift, linear_and_stable_echo_path}` | false | loopback 稳定路径可标 `linear_and_stable` |
| 多通道自定义 config | (高层永远写死 `create_default_multichannel_config()`) | — | 立体声参考无法自定义 |
| **可达** | `echo_removal_control.transparent_mode` | Legacy | 唯一能调的 |

> `create_default_multichannel_config()`(`config.rs:307-316`)是唯一预设档,只改 4 项(coarse 长度 11、rate 0.95、suppressor max_dec_factor_lf 0.35、max_inc 1.5)。没有 aggressive/low-latency 等档位。
> `validate()`(`config.rs:42-304`)把数值 clamp 到合法区间,**不在构造时自动调**,需手动调。

---

## 3. 子模块要点(供后续深挖索引)

### 3.1 自适应滤波器(`subtractor.rs` / `adaptive_fir_filter.rs`)

- **频域分块卷积 FIR**(partitioned FFT),非时域 NLMS。每 partition = 64 样本 FFT block。
- 双滤波:`refined`(NLMS 自适应步长 `mu=H_error/(0.5·H_error·X²+N·E²)`,`refined_filter_update_gain.rs:74`)+ `coarse`(固定速率 `mu=rate/X²`,默认 0.7,`coarse_filter_update_gain.rs:51`)。
- **长度是编译期上限**:`max_refined_len = max(refined_initial, refined).length_blocks`(`subtractor.rs:151-155`),`new` 一次性分配满(`adaptive_fir_filter.rs:180-186`),运行期只能在 0..max 间平滑切换。**想要更长 tail 必须构造前改 config**(坑 #1)。
- 暖机/暂停:`poor_excitation || saturated_capture || call_counter<=size` → 增益清零。
- misadjustment 缩放(`subtractor.rs:294-303`):e²/y² 过大时整体缩 H、当块增益置 0。

### 3.2 抑制器(`suppression_gain.rs` / `suppression_filter.rs`)

- 核心 `gain_to_no_audible_echo`(`suppression_gain.rs:413-435`):按 `enr=echo/(nearend+1)`、`emr=echo/(masker+1)` 在 `[enr_transparent, enr_suppress]` 线性插值。
- LF/HF 模板分界 `last_lf_band=5` / `first_hf_band=8`,各自一套 mask 阈值。
- 默认 tuning(`config.rs:952-979`):normal LF `(0.3/0.4/0.3)`、HF `(0.07/0.1/0.3)`;nearend LF `(1.09/1.1/0.3)`(双讲几乎不动 LF)。
- 增益时序约束:`max_inc_factor=2.0`(逐块上升上限)、`max_dec_factor_lf=0.25`。
- 残余回声 `ResidualEchoEstimator`:线性路径 `R2=|S|²/ERLE`,非线性 `R2=X²·gain²`(transparent 模式 gain 硬编码 0.01)+ reverb。

### 3.3 双讲(没有独立 detector)

AEC3 不设单独 double-talk detector,靠两套间接机制:
- **冻结/放缓滤波**:noise_gate + misadjustment 缩放 + leakage 切换三重间接(`refined_filter_update_gain.rs:88-138`),**无显式 "if doubletalk: freeze" 开关**。
- **dominant nearend 检测**(`nearend_detector.rs`):只看 LF bins `[1..16]`,进入条件持续 `trigger_threshold=12 blocks(48ms)`、保持 `hold_duration=50 blocks(200ms)`。检测到就把抑制器切到 nearend 模板。默认响应偏慢(坑 #17)。

---

## 4. 延迟与对齐链路(reference-based AEC 的头号难点)

### 4.1 Render delay buffer(`render_delay_buffer.rs`)

- 三套并行高速缓冲(`blocks`/`spectra`/`ffts`)+ 一个低速降采样副本(`low_rate`,给 matched filter)。
- 容量 ≈ **612ms** 远端历史(`low_rate` 2448 降采样样本 / `blocks` 153 blocks,`common.rs:64-84`)。
- **对齐颗粒度 = 整 block(4ms)**;`apply_total_delay`(`:349-359`)移读指针,**无分数延迟**。亚 ms 由内部自适应 FIR 在频域吸收。

### 4.2 Matched filter / 延迟估计(`matched_filter.rs` 等)

- NLMS 互相关,5 个滤波器并行(`num_filters=5`),各偏移 `alignment_shift`,挑误差最小者。
- **最大 lag ≈608ms**(不是 152ms!):`get_max_filter_lag = num_filters·intra_shift + filter_len = 5·384+512 = 2432` 降采样样本 ×4 / 16000 ≈ 608ms(`matched_filter.rs:563-565`,核实于测试 `:797-815`)。
  → **主文档 §6.6 的 "152ms max-lag" 应改为 ≈608ms。** 粗对齐预算其实很宽松,只要链路总延迟落在 0–600ms 内,内部就能搜到。
- 输出经 250 帧直方图聚合(`matched_filter_lag_aggregator.rs`),候选超 `initial=5`/`converged=20` 票才采纳,再转 block 数喂 `align_from_delay`。

### 4.3 外部延迟注入(`set_stream_delay_ms`)— 默认形同摆设

- 调用链:`Apm::set_stream_delay_ms`(钳 [0,500]ms)→ 每帧 `set_audio_buffer_delay` → `render_delay_buffer.rs:317-324` **只缓存,不应用**。
- **默认(`use_external_delay_estimator=false`)**:外部值**只在 reset 时用作一次初始猜测**(`render_delay_buffer.rs:142-153`),之后被 matched filter 持续覆盖。
- **想让它真生效**:必须 `delay.use_external_delay_estimator=true`(`block_processor.rs:75-83`)——此时 matched filter / EchoPathDelayEstimator **完全不实例化**,改走 `align_from_external_delay`(`:285-292`)。但这个开关高层 API 够不到。
- → 我们现在 `sonora_aec3.rs` 里填 `initial_delay_ms` 基本没用(走高层默认路径);要么靠内部自动估(已验证可行,自动估出 48ms),要么 fork 开 `use_external_delay_estimator` 自己做粗对齐。

### 4.4 时钟漂移(`clockdrift_detector.rs`)— 不是 stub,但只检测不补偿

- **真实现**(`clockdrift_detector.rs:30-70`):维护 3 拍 delay 历史,模式匹配出 None/Probable/Verified(up/down)。算法很粗,只能检"delay 估计连续单调走 1/2/3 步"的局部线性趋势,**不算速率、不做任何重采样补偿**。
- 仅在 lag aggregator 达 Refined 质量时喂数据,分辨率 ≥4ms。
- 输出只是 `EchoPathVariability` 里一个 bool,影响自适应行为,**不做时基纠正**。
- → **主文档 §6.6 "clockdrift_detector stub" 应改为:真实现,但只给布尔提示、不做补偿。** 实测 ppm 偏差明显时,必须在 AEC3 之外做异步重采样补偿(这正是 recorder 录 QPC+设备位置 CSV 的用途)。

### 4.5 可暴露的延迟监控量

- 对外只有 `delay_ms`(= 当前对齐量 `render_buffer.delay() × 4ms`,`block_processor.rs:109-116`)。
- 内部已算但 `pub(crate)`(要 fork 才能读):`has_clockdrift()`、原始样本级 `delay_samples`、`consistent_estimate_counter`、`max_observed_jitter`、underrun/overrun 计数、`min_direct_path_filter_delay()`(自适应 FIR 估的回声直达延迟)、`reverb_decay()`。
- **诊断对齐异常最该暴露的三项**:`has_clockdrift()`、原始 `delay_samples`、`max_observed_jitter`。

---

## 5. 集成坑点清单(22 条,按影响排序的关键 8 条)

| # | 坑 | 位置 | 影响 |
|---|---|---|---|
| **1** | **输入量程必须 i16(±32768)浮点,非 ±1.0**;用 ±1.0 则 ERLE 恒最小、suppressor 阈值全失效 | `common.rs:53`(X2 门槛假设 int16)、`suppression_gain.rs:166`、`aec_state.rs:337`、`suppression_filter.rs:275` | **绕过高层时必须自己 ×32768**;走高层则 `copy_from_float` 自动转(`audio_processing_impl.rs:335`) |
| **2** | tail 长度硬上限 = `filter.refined.length_blocks`(默认 52ms),且必须构造前定;"无上限"说法不成立 | `subtractor.rs:151-165`、`adaptive_fir_filter.rs:180-186` | 外放场景最该调的参数,高层够不到 |
| **3** | `residual_echo_likelihood` 在 aec3 crate **零实现** | 全局 0 命中 | 我们现在读到的值另有来源(§7),需复核 |
| **4** | `divergent_filter_fraction` 零实现,只有内部 `all_filters_diverged` bool | `subtractor_output_analyzer.rs:29-65` | 同上 |
| **5** | `BlockProcessor` 无 public `reset()` | `block_processor.rs:47-237` | 重置只能重建实例或触发 echo-path-change |
| **6** | 采样率只支持 16k/32k/48k | `common.rs:46-48` | 其它率 debug 下 panic |
| **7** | `set_stream_delay_ms` 默认形同摆设(只 reset 时用一次) | `render_delay_buffer.rs:142-153` | 见 §4.3 |
| **8** | clockdrift 只检测不补偿 | `clockdrift_detector.rs` | 见 §4.4 |

其余 14 条(集成时查阅):
9. ERLE 用 `fast_approx_log2f` 位 trick,与 C++ 不 bit-exact(`common.rs:89-94`)。
10. SIMD vs Scalar 有 ~1e-2 偏差,跨 backend 不可复现(`adaptive_fir_filter.rs:842-863`)。
11. SIMD 仅 x86/x86_64/aarch64,其它平台只走 Scalar(`matched_filter.rs:13-17`)。
12. `detect_backend()` 在 suppression_filter/gain/cng 各自独立调用,`with_backend` 管不到它们(`suppression_filter.rs:163` 等)。
13. `BlockProcessor` 是 `Send` 非 `Sync`;render/capture 必须共享同一实例 + 外部锁(WebRTC 同模型)。
14. `handle_echo_path_change` 语义反直觉:`delay_change` 才全清,`gain_change` 不清 H、不重置 poor_excitation(`subtractor.rs:400-421`)。
15. `last_gain`/`last_nearend`/`e_output_old` 在 path change 不清,可能残影 1 帧。
16. `ResidualEchoEstimator.echo_reverb` 无外部 reset 入口。
17. 双讲检测只看 LF bins[1..16],hold 200ms/trigger 48ms,响应偏慢(可调,见 §2.2)。
18. 无显式 doubletalk freeze,靠 noise_gate+misadjustment+leakage 三重间接。
19. suppressor 大量 magic 常量(`K_UPPER_ACCURATE_BAND_PLUS_1=29` 等)config 调不到。
20. 热路径每块 `vec![...; num_capture_channels]` 分配,低延迟场景需 fork 优化(`echo_remover.rs:225-237`)。
21. 必须先 `buffer_render` 再 `process_capture`,否则 capture 被丢(`block_processor.rs:138-150`)。
22. `partition_to_constrain` 在 echo-path-change 不重置,FFT 约束 round-robin 从中途接续。

---

## 6. 高层 sonora 在中间做了什么(绕过它就要自己补)

高层 `EchoCanceller3`(`sonora/src/echo_canceller3.rs`)在调 `BlockProcessor` 前后做的"胶水":

1. **量程转换** `copy_from_float`:f32[-1,1] ↔ ±32768(`audio_processing_impl.rs:335-349`)——**坑 #1 的解药**。
2. **三带分析/合成滤波**:48k/32k ↔ 16k 子带(`three_band_filter_bank.rs`)。
3. **分帧**:10ms 帧 → 2×5ms sub-frame(80)→ 64 样本 block(`FrameBlocker`/`BlockFramer`)。
4. **跨线程队列** `SwapQueue`(容量 100 帧),隔离 render/capture 调用顺序。
5. **HPF + 饱和检测 + 立体声内容检测 + 下混**。
6. 可选固定 capture 延迟 `BlockDelayBuffer`。
7. 指标转发、NS/AGC2 串接(我们不用)。

→ **"精简直连"方案要自己重写 1/2/3/4/5**(尤其量程和三带滤波),工作量不小。

---

## 7. 我们现在读的统计是真的吗?(已核实)

`sonora_aec3.rs:177-185` 读了 `statistics()` 的字段,核实 `audio_processing_impl.rs:732-749`:

- `echo_return_loss_enhancement` → **真**(aec3,`echo_remover.rs:195` fullband_erle;`impl:746`)。
- `delay_ms` → **真**(当前对齐量;`impl:748`)。
- `residual_echo_likelihood` → **真有效**:来自独立 `EchoDetector`(由 `.echo_detector(true)` 开启,
  **非 AEC3 内部**;`impl:733-736`)。我们已开 echo_detector,故此值有效。
- `divergent_filter_fraction` → **恒 `None`**:`stats.rs:18` 定义了字段,但
  `audio_processing_impl.rs` 全文**无任何赋值点**(grep 0 命中)。

> ⚠️ **坐实的 bug**:`sonora_aec3.rs:183` 的
> `diverged: s.divergent_filter_fraction.map(|f| f>0.5).unwrap_or(false)` 因该字段恒 `None`,
> **`diverged` 永远为 `false`,发散检测失效**。
> 修法:(a)暂用替代判据(ERLE 长期 <某阈值 且 residual_echo_likelihood 高 → 疑似发散);
> 或(b)fork 高层把 aec3 内部 `all_filters_diverged`(`subtractor_output_analyzer.rs:29-65`)
> 算成 fraction 暴露到 `AudioProcessingStats`(随方案 A/B 下沉时一并做)。

---

## 8. 主文档需修正项

`research/windows_aec_research.md` §6.6 据本调研更正:
- ❌ "matched_filter 最大 lag ≈152ms" → ✅ **≈608ms**(num_filters=5)。
- ❌ "clockdrift_detector 是 stub" → ✅ **真实现,但只输出布尔提示、不做漂移补偿**。
- 补充:**外部延迟 `set_stream_delay_ms` 默认仅 reset 时生效一次**,需 `use_external_delay_estimator=true` 才真正绕过 matched filter。

---

## 9. 集成方案建议(基于以上事实)

### 9.1 核心判断

用户诉求是"知道坑在哪、上限更高"。本调研证明:
- **上限被锁死的根因**:高层 sonora 不放行 `EchoCanceller3Config`。要拿 tail 长度/抑制激进度/延迟搜索范围/导出线性 AEC 输出(喂 LocalVQE 的关键),**必须能改这个 config**。
- 三条路:

| 方案 | 做法 | 成本 | 拿到的上限 |
|---|---|---|---|
| **A. fork 高层 + 开 config 注入口** | vendor 整个 sonora workspace 改 path,在 `AudioProcessingBuilder` 加一个 `.aec3_config(EchoCanceller3Config)` setter,改 `audio_processing_impl.rs:1280` 让它接受外部 config | **低**(改几十行,胶水全复用:量程/三带/分帧/队列都不用自己写) | **全部 config 可调**,且自动避开坑 #1/#6 等 |
| **B. 精简直连 BlockProcessor** | 只 vendor aec3+common-audio+fft+simd,自己写量程转换+三带滤波+分帧+队列 | **高**(要重写 §6 的 1/2/3/4/5,三带滤波尤其麻烦) | 全部 config + 内部状态全可见,但前期啃内部 API、自己背所有坑 |
| **C. 不动,继续黑盒** | 现状 | 0 | 只有 transparent_mode |

### 9.2 推荐:A(fork 高层开 config 口),B 作为后续可选下沉

理由:
- A 用最小改动拿到 **99% 的上限**(`EchoCanceller3Config` 全可调),且**白嫖高层全部胶水**——量程转换(坑 #1)、三带滤波、分帧、跨线程队列这些又难又容易错的活儿都不用自己碰。
- B 的额外收益(内部 `pub(crate)` 状态可见)只在"要做精细诊断/改算法"时才需要;**等真要改 AEC3 算法本身或做 neural REE hook 时再下沉到 B**(届时只需把可见性放开,而非重写胶水)。
- 两者都需要 vendor 源码(git 依赖无法改),所以第一步动作相同:**vendor sonora workspace 进 `echoless/vendor/`,改 path 依赖**。

### 9.3 落地步骤(若采纳 A)

1. vendor:把 sonora workspace(去 cpp/fuzz/ffi/bench/sys)拷进 `echoless/vendor/sonora/`,`echoless-processors` 改 path 依赖。
2. 在高层 `AudioProcessingBuilder` 加 `aec3_config: Option<EchoCanceller3Config>` + setter;`audio_processing_impl.rs:1280` 改为 `cfg.unwrap_or_default()`,multichannel 同理开口。
3. `EchoCanceller3Config` 在 sonora 公共 API 重导出(`pub use sonora_aec3::config::EchoCanceller3Config`)。
4. `sonora_aec3.rs::configure` 把我们的 TOML 参数(tail_ms→length_blocks、suppressor 激进度、num_filters、export_linear_aec_output…)映射进 `EchoCanceller3Config`,构造前注入。
5. 复核 §7 统计来源;按需在高层 `statistics()` 暴露 `has_clockdrift`/原始 delay/jitter(下沉 B 的前哨)。
6. 回归:重跑合成 ERLE 测试 + 真实录音 eval,确认改 tail/激进度后指标变化符合预期。

> 注:本项目本地自用、无 license 顾虑([[aec-self-use-no-license]]),fork/改 sonora 源码无障碍。

---

## 10. 与现有代码的直接行动项(优先级)

1. **复核统计来源**(§7)——可能影响 `diverged` 判据,**先做**。
2. **决定 A/B/C**——决定后第一步都是 vendor。
3. 若 A:按 §9.3 开 config 口,把 `configure` 接上真实参数(当前 `initial_delay_ms` 基本无效,见 §4.3)。
4. 修正主文档 §6.6(§8)。
5. tail 长度做成可配(外放场景大概率要 >52ms),用真实录音验证最优值。

---

## 11. 集成实测结论(方案 A 落地后,2026-06)

vendored fork(`echoless/vendor/sonora`)+ config 注入口已落地并跑通。合成回归测试
(`echoless-processors/tests/{echo_cancellation,sonora_direct}.rs`)挖出几条**关键经验**:

### 11.1 ⚠️ 激励必须非平稳——平稳白噪声会被 AEC3 自己抑制

**最大的坑(测试层面)**:用平稳白噪声做合成回声,实测仅 **~8dB**,且 `erle` 指标恒定不更新、
延迟估计乱跳。原因:AEC3 有 **stationarity gate / poor-excitation 判据**,故意把平稳信号当背景
噪声、不让滤波器自适应(避免把稳态噪声误当回声)。换成**非平稳语音类**信号(白噪声 × 音节
起伏包络 + 周期停顿)后:

| 回声延迟 | 平稳白噪声 | 非平稳语音类 |
|---|---|---|
| 10ms | 7.8dB | **41.2dB** |
| 50ms | 8.7dB | **23.2dB** |

→ 结论:**sonora AEC3 工作正常**。任何"AEC 没效果"的合成测试,先检查激励是否非平稳。
真实麦克风/语音天然非平稳,不受此限;但合成测试与单元测试必须用语音类信号。

### 11.2 ⚠️ `erle_db` 统计不可信,以输出能量下降为准

实测 `echo_return_loss_enhancement` 在所有场景恒为 bit-identical 的 `0.1755…`,从不更新
(fullband_erle 的更新阈值 `X2_BAND_ENERGY_THRESHOLD×65` 在合成信号下过不去),但**实际回声
确实被消除**。→ `ProcessorStats.erle_db` 不能作为效果判据;`eval`/监控应测**输出/输入能量比**。
`residual_echo_likelihood`(独立 EchoDetector)随回声强度起伏,相对可用。

### 11.3 ⚠️ 注入 tail 时不要动 initial 滤波器长度

`build_aec3_config` 拉长 `refined.length_blocks` 时,**不可同时拉长 `refined_initial`/
`coarse_initial`**。上游初始滤波器故意短(快速粗收敛→再切长滤波器精修),拉长 initial 破坏
两阶段收敛,实测 tail=120ms 从 23dB 暴跌到 6.6dB。已在 `build_aec3_config` 修正(只改主长度)。

### 11.4 注入 config 仅在真有调参时(避免改变默认路径)

无条件注入会把默认路径的 `multichannel_config` 从上游 `create_default_multichannel_config()`
退化成 `default()`。已用 `Aec3Tuning::is_default()` 守卫:全默认时不注入,走上游原始路径。

### 11.5 已修复 / 待验证

- ✅ diverged bug(§7):改用 `residual_echo_likelihood>0.95` 近似(注释标注;可靠版待 fork 暴露 `all_filters_diverged`)。
- ✅ tail / delay_num_filters / linear_stable_echo_path 可经 TOML 注入,测试覆盖。
- ⏳ **长跑(>10s)合成场景效果随 pause 段累积退化**(15s 单径从 23dB→9.6dB,5s 正常)。疑为合成
  信号 artifact,**真实退化行为待 Phase 1 真实录音验证**——若真实长跑也退化,需查 misadjustment/
  状态漂移,这是连麦数小时稳定性的关键风险点。
- ⏳ tail 数值调优的真实最优值:合成短窗测不出(对短回声过参数化反而更差),留真实混响录音定。
