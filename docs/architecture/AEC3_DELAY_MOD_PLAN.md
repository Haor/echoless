# AEC3 延迟魔改方案:延迟保持(惯性)+ 负方向延迟搜索

日期:2026-07-03 · 状态:方案(待执行)
前置:`AEC3_INTERNALIZATION_PLAN.md`(先内化改名再魔改;下文行号基于当前 `vendor/aec3` 路径,改名后同构映射)。
背景真理来源:`docs/research/aec3_internal_map.md`(延迟链路 §6.6)。

---

## 0. 要解决的实际问题(证据)

| 故障模式 | 实测证据 | 对应魔改 |
|---|---|---|
| ref 断续(underrun 补零 / stale 跳帧)后回声漏一段,AEC3 需 ~1s 重新收敛 | Windows loopback 时序抖动,health 面板 ref underruns=8 为真实数据;underrun 时 `far.fill(0.0)`(`realtime.rs:511-517`),stale 时跳帧(`realtime.rs:565-575`) | **功能1 惯性** |
| 切歌/远端静音后一小段回声漏过 | render 静音期 matched filter 无激励,对齐停滞;若此时扬声器实际还在响(ref 抓取断了),AEC3 误判为纯近端语音 | **功能1 惯性** |
| mic 领先 ref(负 lag)时 AEC3 完全对不上,只能靠 probe 手测 + 手填 near_delay | probe 语义:负 lag = mic 领先 ref(`probe_delay.rs:616-644`,单测 `:868-886`);`recommended_near_delay_ms` 只对负 lag 给值(`:711-716`);macOS Process Tap 固有超前,默认 near_delay=25ms 就是在救这个(`echoless-core/src/lib.rs:37-42`) | **功能2 负方向** |
| 时钟漂移无补偿,长跑对齐点缓慢偏移 | 管线无 ppm 闭环(`cross_platform_architecture.md:337-341`);AEC3 clockdrift_detector 只检测不补偿 | 功能1 缓解症状;根治属 drift 补偿(另立项) |

---

## 1. 延迟链路现状(魔改切入点地图)

```
render(far) ──insert──> RenderDelayBuffer ──low_rate──┐
                          │ underrun: delay-- + 软重置 ①      │
capture(near) ──> BlockProcessor                        ▼
                    └─> RenderDelayController.get_delay
                          └─> EchoPathDelayEstimator
                                ├─ MatchedFilter.update ← 每帧无条件更新 ②(无 render gate)
                                ├─ LagAggregator(250-tap 直方图,票数判决)
                                └─ consistent>125 → 软 reset(保置信度)
                          └─> compute_buffer_delay(迟滞,仅双 Refined 且仅压制增大方向)
                    └─> render_buffer.align_from_delay(移 render 读指针,clamp [0, max] ③)
```

关键事实(file:line 见 `vendor/aec3/crates/aec3-core/src/`):

- **① underrun 打击面**:`render_delay_buffer.rs:229-240` underrun 时 `delay -= 1`(硬扣一 block);`block_processor.rs:166-170` 同时对 controller `reset(false)` 软重置。**这是 ref 断续毁掉对齐的直接来源。**
- **② 无 render-activity gate**:`echo_path_delay_estimator.rs:107-111` 无条件调 `matched_filter.update`。`render_delay_buffer` 算了 `render_activity`(`:185-191,425-431`,阈值 `active_render_limit=100.0`)但**只给 echo_remover 用,从不进延迟估计**。MatchedFilter 内部只有逐样本防除零弱门(`matched_filter.rs:104`)。→ 静音期直方图被噪声污染、估计漂移。
- **③ lag ≥ 0 的三处结构假设**(功能2 的障碍):
  - matched filter 搜索窗起点固定 lag=0,窗口 [0, ~608ms](`matched_filter.rs:483-484,563-565`);
  - 直方图下标非负,负 lag 已被 `.max(0)` 钳死(`matched_filter_lag_aggregator.rs:231`);
  - 对齐机制只能"延迟 render 读指针",**结构上无法让 capture 相对 render 提前**(`render_delay_buffer.rs:347-357`,clamp `align_from_delay:275`)。
- **有利遗产**:controller 对估计器返回 `None` 是"粘"的——保留已有 delay 只递增计数(`render_delay_controller.rs:118-121`);`significant_candidate_found` 一旦置位,非 hard reset 不清(`matched_filter_lag_aggregator.rs:220-222`)。**上游本身有半套惯性,缺的是 ①② 两处。**
- **调参够不到**:`delay_selection_thresholds.converged` 提高、`hysteresis_limit_blocks` 调大(仅压延迟增大方向)、`smoothing_delay_found` 调小,都只能减缓漂移,**治不了 ① 的硬扣和 ② 的静音污染**。
- **外部延迟路径不可用作惯性**:`use_external_delay_estimator=true` 会整个关掉自适应估计(`block_processor.rs:75-83,193-195`),变成开环跟随;且 `align_from_external_delay` 对负值未 clamp,行为未定义(`render_delay_buffer.rs:283-290`)。
- **注入通道已就绪**:fork 的 `builder.aec3_config(EchoCanceller3Config)`(`audio_processing_impl.rs:1291-1302`)→ `echoless-processors/src/aec3.rs` 的 `Aec3Tuning`(`:21-50`)→ `build_aec3_config`(`:199-219`)。builder 级字段 = **重建引擎生效(重启),非热更新**。

---

## 2. 功能1:延迟保持(惯性)— core 魔改,改动面小

### 设计

新增 config(`config.rs` 的 `struct Delay`,`:407-473`):

```rust
pub delay_hold: bool,                    // 总开关,echoless 默认 true(上游语义默认 false)
pub render_gate_power_threshold: f32,    // 降采样 render 子块能量阈值,默认对齐 active_render_limit
pub render_gate_hold_blocks: usize,      // 连续低能量 N 块才进入 hold(迟滞防抖),默认 3(12ms)
```

三处逻辑改动(全部 `if delay_hold` 门控,关掉即上游原行为):

1. **underrun 不扣延迟**(治①):`render_delay_buffer.rs:235-239` 的 `delay -= 1` 跳过——underrun 补零帧在时间轴上仍占一个 block 位置,扣 delay 反而制造错位;同时 `block_processor.rs:166-170` 的 `dc.reset(false)` 跳过。underrun 事件仍照常上报 metrics。
2. **render 静音门(治②)**:`echo_path_delay_estimator.rs:107` 入口处,对当前降采样 render 子块(16 样本)算能量,低于阈值连续 `render_gate_hold_blocks` 块 → 跳过 `matched_filter.update` 与 `aggregate`,直接返回上次 aggregated lag(冻结估计);恢复激励后立即解冻。gate 期间**不递增** `consistent_estimate_counter`(避免静音期触发 `>125` 软 reset,`echo_path_delay_estimator.rs:144-147`)。
3. **hold 期间对齐冻结**:`render_delay_controller.rs` 在 gate 生效期不更新 `delay_samples`(现状对 `None` 已保持,此处只需保证 gate 路径返回 `None`/旧值即可,基本免费)。

### 为什么不做 hold 超时

hold 错误(静音期间物理延迟真的变了,如切设备)的代价 = 恢复后重新收敛 ≈ 现状的默认代价,**下界不劣于现状**;而超时机制引入新参数和新状态机。设备切换在 echoless 里本来就走引擎重建路径,不依赖估计器自愈。故不加超时,保持机制最简。

### 下发与生效

`Aec3Tuning` 加 `delay_hold: Option<bool>`(默认 Some(true),echoless 侧默认开)→ `build_aec3_config` 映射 → builder 注入。**重启生效**(builder 级)。高级页暂不暴露;若暴露,归入 AEC3 分组,Hint 写「参考信号断续时保持延迟对齐,推荐开启」。

### 验证

- 单测(vendor 内):合成 48k 信号收敛后注入 200ms 补零 + 5 次人为 underrun,断言 `delay_samples` 不变、恢复后 ERLE 立即回到收敛水平;对照组(hold=false)断言出现 delay 扣减与重收敛窗口。
- 集成测(echoless):复用 `tests/echo_cancellation.rs` 场景,ref 流中间挖 300ms 洞,比较输出残余能量曲线(hold on/off)。
- 注意实测坑:激励必须非平稳语音类(平稳白噪声被 stationarity gate 压制,见 internal map §11)。

---

## 3. 功能2:负方向延迟搜索 — 偏置方案为主,core 负 tap 为远期

### 核心洞察:负搜索 = 正搜索 + 恒定偏置,且物理上无法免费

负 lag(mic 先于 ref 收到对应信号)要对齐,**必然延迟近端**——core 魔改内部也得给 capture 加缓冲,额外延迟一样逃不掉,只是藏在引擎里。因此「wrapper 偏置」与「core 负 tap」端到端延迟代价**相同**,而前者改动面小一个数量级。

现有机制恰好就是半成品:`apply_near_delay`(`realtime.rs:577-593`)整体后移 near,**支持运行中热更新**(`set_near_delay_ms`,`control.rs:385-401,519-529`,不重启),macOS 已默认 25ms。缺的只是**语义升级**:从「修正已知超前的补丁」变成「常开的负方向搜索余量(search bias)」。

### 方案 N1(主方案):near_delay 升级为搜索偏置

1. **两平台默认非零偏置**:macOS 保持 25ms;Windows 由 0 → **20ms**(`echoless-core/src/lib.rs:37-42`)。效果:AEC3 有效搜索窗从 `[0, +608ms]` 平移为 `[-D, +608ms-D]`,D 范围内的负 lag 被 AEC3 **自适应**吸收(不再依赖 probe 手测精确值,probe 只需保证最坏负 lag 不超过 D)。
2. **probe 语义联动**:`recommended_near_delay_ms`(`probe_delay.rs:711-716`)从「负 lag 才给值」改为「`max(bias_default, -lag + safety)`」——正 lag 时维持默认偏置而非归零;测得负 lag 超出默认偏置时自动上调。同时修复 UI 文案歧义(UI 报告 C6):正 lag 显示「回声延迟 +Xms,由 AEC3 追踪;负向余量 D ms 已生效」。
3. **代价核算**:输出链路额外 +20ms(总延迟预算内需确认;语音连麦可接受)。搜索窗损失 20ms(608→588ms)可忽略,必要时 `delay_num_filters` +1 补回。
4. **观测钩子**:AEC3 内部估计的 delay 若长期贴 0 下限,说明真实 lag 比 -D 更负、偏置不足——在 metrics 里透出 `aec3_delay_blocks`(fork 已可访问内部状态),前端诊断页给「建议增大 near delay / 重跑 probe」提示。

改动面:`echoless-core`(默认值)、`probe_delay.rs`(推荐公式)、`aec3.rs`(metrics 透出)、前端文案。**vendor 零改动**。

### 方案 N2(远期,真负 tap):core 搜索窗负偏移

给 `Delay` 加 `negative_search_samples`,`matched_filter.rs:483` 起点前移、`matched_filter_lag_aggregator.rs` 下标体系整体平移、`render_delay_controller`/`render_delay_buffer` 支持带符号 delay 并新建 capture 侧缓冲。改动横跨 4 个核心文件 + config + validate,且 capture 缓冲带来的延迟与 N1 相同。**仅当 N1 的固定偏置被证明不够(如负 lag 动态范围大、偏置吃不住)时再做**;届时从本方案的观测钩子数据反推所需窗口。

### 弃选:external delay estimator 路径

`use_external_delay_estimator=true` + probe 值理论可覆盖负 lag(`align_from_external_delay` 的 ext_delay 是 i32),但:完全关闭自适应估计(开环)、负值未 clamp 行为未定义需先修、失去 matched filter 精修。与功能1 的自适应诉求矛盾,不采用。

### 与功能1 的协同

偏置(N1)把工作点搬进正搜索窗,惯性(功能1)保住搬进来之后的收敛成果。两者独立生效、叠加受益:ref 断续 + 负 lag 并存的最差场景(Windows loopback 抖动 + 蓝牙 mic 路径)正是两者同时命中的地方。

---

## 4. 实施顺序与里程碑

| 阶段 | 内容 | 依赖 | 生效方式 |
|---|---|---|---|
| 0 | 内化改名(`AEC3_INTERNALIZATION_PLAN.md` 全部 8 步) | — | — |
| 1 | 功能2-N1:Windows 默认偏置 + probe 推荐公式 + 文案(顺带修 UI 报告 C6) | 无(不碰 vendor) | near_delay 热更新,立即 |
| 2 | 功能1:config 3 字段 + 三处 core 改动 + vendor 单测 | 阶段 0 | 重启生效 |
| 3 | 观测钩子:`aec3_delay_blocks` 透出 metrics + 诊断页提示 | 阶段 0 | — |
| 4(远期) | 功能2-N2 真负 tap,视阶段 3 数据决定是否立项 | 阶段 2、3 | 重启生效 |

阶段 1 与阶段 0 可并行(互不触碰);阶段 2 的 diff 必须落在改名后的命名空间上。

## 5. 风险清单

- **功能1 静音门阈值**:阈值过高会把低音量音乐当静音冻结估计 → 沿用上游 `active_render_limit=100.0`(i16 量程!注意 aec3 内部是 ±32768 量程,不是 ±1.0)起步,用真实录音回归。
- **功能1 与两阶段收敛的交互**:gate 只冻结 matched filter/aggregator,不碰 refined/coarse 滤波器自适应(它们有自己的 render activity 逻辑),不会重蹈「拉长 initial 滤波器致效果暴跌」的坑(internal map §11.3)。
- **N1 偏置加大端到端延迟**:20ms 是否可接受需在总延迟预算(采集+处理+虚拟麦+Discord)里实测确认;不行则 Windows 偏置降为 10ms,牺牲部分负向余量。
- **长跑退化疑云**(internal map §11.6,15s 合成场景 23→9.6dB 待验证):功能1 改动触及估计器生命周期,回归时必须包含 >60s 长跑用例,顺带验证该疑云。
