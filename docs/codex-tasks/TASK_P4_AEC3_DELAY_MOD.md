# Codex 任务规格 P4:AEC3 延迟魔改(惯性 + 负方向偏置)

日期:2026-07-04 · 执行者:Codex(gpt-5.5 xhigh)· 工作树:P3 合入 main 后从 main 新切 `phase-2/aec3-delay-mod`
背景方案:`docs/architecture/AEC3_DELAY_MOD_PLAN.md`(设计动机与决策依据,先通读)。
**前置硬依赖:P3(内化改名)已合入 main。** 本文档锚点写的是改名前路径 + 2026-07-04 校准行号;P3 是纯改名不动逻辑,行号在新路径下同构成立,按下方映射换算:

| 改名前 | 改名后 |
|---|---|
| `vendor/aec3/crates/aec3-core/src/` | `vendor/aec3/crates/aec3-core/src/` |
| `vendor/aec3/crates/aec3-apm/src/` | `vendor/aec3/crates/aec3-apm/src/` |
| `crates/echoless-processors/src/aec3.rs`(`Aec3Engine`) | `crates/echoless-processors/src/aec3.rs`(`Aec3Engine`) |

## 锚点校准注记(2026-07-04 逐项核实,相对原方案的修正)

- 原方案引用 `render_delay_buffer.rs:347-357` 的「二次 clamp」**不存在**;`align_from_delay` 唯一 clamp 点是 `:275` `total_delay.max(0).min(self.max_delay())`。
- 顶层重导出在 `vendor/aec3/crates/aec3-apm/src/lib.rs:34-35`(非 31):`pub use aec3::config as aec3_config;` 等。
- 软 reset 阈值不是字面 `125`,是常量 `NUM_BLOCKS_PER_SECOND_BY_2`(`echo_path_delay_estimator.rs:145`)。
- `realtime.rs` 拆分后,`apply_near_delay`(577-593)、underrun `far.fill(0.0)`(511-519)、`skip_stale`(565-575)**仍在 `crates/echoless-cli/src/realtime.rs`**;`set_near_delay_ms` 热更新在 `realtime/control.rs:385-401`,retune 在 `:519-530`。
- 其余锚点全部核实无漂移。

## 功能 1:延迟保持(惯性)— vendor core 改动

### 新增 config(`aec3-core/src/config.rs` 的 `struct Delay`,当前 407-438 + Default 440-473)

```rust
pub delay_hold: bool,                    // 总开关;上游语义默认 false,echoless 注入时开 true
pub render_gate_power_threshold: f32,    // 降采样 render 子块能量阈值,默认 = active_render_limit(100.0,i16 量程!内部 ±32768 非 ±1.0)
pub render_gate_hold_blocks: usize,      // 连续低能量 N 块才进入 hold(迟滞防抖),默认 3(=12ms)
```

`config.rs` 若有 validate 逻辑,新字段纳入(threshold > 0,hold_blocks ≥ 1)。

### 三处逻辑改动(全部 `if delay_hold` 门控,关掉 = 上游原行为,diff 最小化)

1. **underrun 不扣延迟**:`render_delay_buffer.rs:229-240`(实际 `self.delay = Some(d - 1)` 在 235-239)——`delay_hold` 时跳过扣减;同时 `block_processor.rs:166-170` 的 `dc.reset(false)` 跳过。underrun 事件照常上报 metrics。理由:补零帧在时间轴上仍占位,扣 delay 制造错位。
2. **render 静音门**:`echo_path_delay_estimator.rs:107-111` 的 `matched_filter.update(...)` 入口前,对当前降采样 render 子块(16 样本)算能量;低于阈值**连续** `render_gate_hold_blocks` 块 → 跳过 `update` 与 `aggregate`,返回上次 aggregated lag(冻结估计);恢复激励立即解冻。gate 期间**不递增** `consistent_estimate_counter`(避免静音期触发 `> NUM_BLOCKS_PER_SECOND_BY_2` 软 reset,`:135-146`)。能量判定参考既有 `detect_active_render`(`render_delay_buffer.rs:425-431`,`x_energy > limit²·FFT_LENGTH_BY_2`)。
3. **hold 期间对齐冻结**:`render_delay_controller.rs:118-121` 对 `None` 已粘滞(保留旧 delay 只递增计数)——只需保证 gate 路径向 controller 返回 `None`/旧值,基本免费。核对 gate 期不会误触发 controller 内其它 reset 路径。

### 明确不做

- **不做 hold 超时**(设计权衡见方案 §2):错误 hold 的代价 = 恢复后重新收敛 ≈ 现状默认代价;设备切换走引擎重建路径,不依赖估计器自愈。
- **不碰 refined/coarse 滤波器自适应**(它们有自己的 render activity 逻辑);gate 只冻结 matched filter / aggregator。不许重蹈「拉长 initial 滤波器致效果暴跌」的坑(internal map §11.3)。

### 下发链路

`crates/echoless-processors/src/aec3.rs` 的 `Aec3Tuning`(改名前 21-36 + Default 38-50,字段:tail_ms / delay_num_filters / linear_stable_echo_path / ns / ns_level / agc / far_channels)加 `delay_hold: Option<bool>`(echoless 侧默认 Some(true))→ `build_aec3_config`(199-219,注意末尾有 `c.validate()`)映射三字段 → builder 注入口 `aec3_config`(fork 注入链:`vendor/aec3/crates/aec3-apm/src/audio_processing_impl.rs` 169 字段 / 206-208 setter / 1291-1302 应用)。**builder 级 = 重启生效,非热更新。** 高级页暂不暴露 UI(前端另由 UI 重构分支处理)。

## 功能 2:负方向偏置(方案 N1)— vendor 零改动

1. **Windows 默认偏置 0 → 20ms**:`crates/echoless-core/src/lib.rs:37-42` `default_near_delay_ms()`(现 `if macos {25} else {0}`)→ `if macos {25} else {20}`。效果:AEC3 有效搜索窗从 `[0, +608ms]` 平移为 `[-20, +588ms]`,20ms 内负 lag 被自适应吸收。
2. **probe 推荐公式升级**:`crates/echoless-cli/src/probe_delay.rs:711-716` `recommended_near_delay_ms` 从「`lag>=0 → 0; else ((-lag+safety)/5).round()*5`」改为「`max(平台默认偏置, -lag + safety)`(仍 5ms 取整)」——正 lag 时维持默认偏置而非归零;负 lag 超出默认偏置时自动上调。相关单测同步:`probe_delay.rs:818-823`(`probe_recommendation_uses_only_mic_lead`)、`:867-886`(mic 领先检测)。
3. **观测钩子**:`aec3.rs` 透出 AEC3 内部延迟估计 `aec3_delay_blocks` 到 metrics/status JSON(fork 可访问内部状态;经由现有 stats 通道,参考 `realtime/stats.rs` 与 status JSON 结构)。语义:该值长期贴 0 下限 = 真实 lag 比 -bias 更负、偏置不足。
4. **前端文案不在本任务**(诊断页提示 + C6 文案修复归 UI 重构分支);但 status JSON 字段名要在本任务定死并写入 `docs/frontend/FRONTEND_PARAMETER_BOUNDARIES.md` 交接:`aec3_delay_blocks: Option<u32>`。

## 测试与验收

### 单测(vendor,`aec3-core` 内联 `#[cfg(test)] mod tests`,与既有风格一致)

- 合成 48k 信号收敛后注入 200ms 补零 + 5 次人为 underrun:断言 `delay_samples` 不变、恢复后无重收敛窗口;对照组(`delay_hold=false`)断言出现 delay 扣减。
- 静音门:render 静音 ≥ hold_blocks 后估计冻结(aggregated lag 不变、consistent counter 不增),恢复激励后解冻。

### 集成测(`crates/echoless-processors/tests/echo_cancellation.rs`,既有 4 场景基于输出能量下降 dB)

- 新增场景:ref 流中间挖 300ms 洞,比较 hold on/off 的输出残余能量曲线(hold on 恢复显著更快)。
- ⚠️ 激励必须**非平稳语音类**信号(平稳白噪声被 stationarity gate 压制,见 internal map §11)。

### 长跑回归(必做)

- **>60s 长跑用例**(可作 `#[ignore]` 标注的重测试):全程 ERLE/能量下降不退化。顺带验证 internal map §11.6 的「15s 合成场景 23→9.6dB 长跑退化疑云」——若复现,单独报告,不要顺手修。

### 全局验收

1. `cd vendor/aec3 && cargo test -p aec3-apm -p aec3-core` 全绿,含既有 `realtime_alloc` **零分配测试**(gate 的能量计算不得引入堆分配!预分配或栈上计算)。
2. 根 workspace `cargo build --workspace && cargo test --workspace` 全绿。
3. `delay_hold=false` 时全部行为与上游一致(既有测试全绿即证)。
4. 输出:每处改动的 file:line diff 摘要 + 新增测试清单 + 长跑结果数据。

## 风险提醒(来自方案 §5)

- 静音门阈值过高会把低音量音乐当静音 → 从 `active_render_limit=100.0` 起步,别自作主张调高。
- N1 的 +20ms 端到端延迟是否可接受待实测;如需回调,只改 `default_near_delay_ms` 一处(10ms 档)。
- 功能 1 与功能 2 独立生效,分开 commit(vendor 改动一批,echoless 侧一批)。
