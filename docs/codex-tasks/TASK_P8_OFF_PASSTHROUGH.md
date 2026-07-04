# Codex 任务规格 P8-D1:OFF = 穿透(bypass)模式(后端)

日期:2026-07-04 · 执行者:Codex(gpt-5.5 xhigh)· 分支:P3 合入 main 后从 main 切 `phase-2/off-bypass`
**产品决策(用户 2026-07-04 拍板)**:不搞三态交互。电源 OFF = 穿透(mic 原样直通虚拟麦),
**绝不能让用户的麦克风变哑**;「完全停机」不作为用户级操作(退出应用 = 停机)。
背景:审计 D1(`docs/audit/UI_ISSUES_VERIFICATION_20260703.md` §D1)。

## 现状(2026-07-04 核实)

- 现在 OFF = 前端调 `stop_run` 杀掉整个 sidecar → 虚拟麦无声,通话对方以为麦坏了。
- stdin 热控制通道成熟:`crates/echoless-cli/src/realtime/control.rs` 的 `RuntimeControlCommand`
  (enum :19,parse :86,dispatch :262)已有 SetOutputLevel / SetNearDelayMs / SetInitialDelayMs /
  SetAec3Ns / SetAec3Agc / SetLocalvqeNoiseGate 六个热命令,均逐 buffer 生效、回吐 JSON 事件。
- `passthrough` 处理器已在 registry 注册(`registry.rs:10`),但**本任务不走换处理器路线**(见设计)。

## 设计:chain 级 bypass,不换处理器

新增热命令 `SetBypass(bool)`,在 realtime 主循环的处理点旁路:

- **bypass=true**:输出 = 近端原始信号(**不经** AEC/NS 处理;**不加** near_delay——那是给 AEC 对齐用的,
  直通时是纯增益延迟;**仍经过** `apply_output_level` 输出增益与 soft limiter,音量滚轮/未来 mute 继续有效)。
- **保温(keep-warm,默认开)**:bypass 期间**照常喂 processor 处理并丢弃其输出**——AEC3 的延迟对齐与
  滤波器保持收敛,重新 ON 时零重收敛(瞬时生效)。CPU 代价 = ON 态持平,可接受。
  实现为常量或 config 字段 `bypass_keep_warm`,默认 true;false 路径(完全跳过处理,省 CPU)也留着。
- **为什么不换 passthrough 处理器**:热换处理器要销毁/重建引擎实例(丢收敛状态、有 glitch 风险),
  bypass 只是输出选择,一个分支搞定,可逆且无状态损失。

## 契约(与前端/GUI 对接,P1 按此实现 UI)

- stdin 命令:`{"cmd": "set_bypass", "enabled": true|false}`。
- 回吐事件:`{"type": "bypass_changed", "bypassed": bool}`(风格对齐既有 `output_level_changed`)。
- status JSON 增加字段 `bypassed: bool`(常驻,默认 false)。
- 启动初值:`run` 配置加可选 `bypass = true`(toml,默认 false)——GUI 未来可「启动即直通、
  用户开电源才启用 AEC」;CLI validate 同步接受该键。
- Tauri 侧(`app/src-tauri/src/lib.rs`):加薄封装 command `set_bypass(enabled: bool)`(参考既有
  set_output_level 的转发写法),`api.ts` 前端封装由 P1 分支自己加,**本任务只做到 Tauri command**。

## 切换平滑性

- ON↔OFF 切换点做 **10-20ms 线性 crossfade**(处理输出 ↔ 原始输出),避免爆音;
  两路信号本就同长同相位(keep-warm 时都在手上),交叉淡化零成本。
- keep-warm=false 时切回 ON 会有重收敛期(~1s 回声漏),事件里如实回吐即可,不做掩饰。

## 范围外(明确不做)

- 前端电源开关语义改造、srail/状态字文案(如 BYPASS)、「启动即直通」的 GUI 流程 → P1(UI 重构分支)。
- 一键 mute(D2)→ 另行小任务(复用 set_output_level 通道)。
- probe 子进程不受影响(它自带 passthrough 配置,不经 bypass)。

## 测试与验收

1. 单测(control.rs 既有测试风格,参考 :583 起的 parse 测试):`set_bypass` 解析、非法参数拒绝。
2. 集成测(`crates/echoless-processors/tests/` 或 cli 侧现有测试设施):
   - bypass=true 时输出 ≈ 近端输入(经输出增益);
   - keep-warm:收敛 → bypass 3s → 恢复 ON,输出残余能量**立即**回到收敛水平(无重收敛窗口);
   - crossfade:切换样本点无阶跃(相邻 buffer 能量无尖峰)。
3. `cargo build --workspace && cargo test --workspace` 全绿;`cargo clippy -- -D warnings`。
4. 实时安全:bypass 分支与 crossfade **零堆分配**(音频线程纪律,参考 processor_chain_alloc 测试思路)。
5. 输出:diff 摘要 + 契约字段确认(写入 `docs/frontend/FRONTEND_STATE_HANDOFF.md` 追加一节)+ 测试结果。

## 排期约束

- P3(内化改名)会重排 control.rs / registry.rs 等文件的 aec3 字样,本任务**等 P3 合入 main 再开**,
  避免无谓冲突。与 P4 并行时注意:P4 也动 `realtime/stats.rs`(aec3_delay_blocks 透出)与 probe;
  本任务动 realtime.rs 主循环 + control.rs——文件交集小,可并行,合并时先 P4 后 D1(或反之均可)。
