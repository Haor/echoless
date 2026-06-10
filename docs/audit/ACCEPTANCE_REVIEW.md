# Echoless 验收复核报告(phase-1/usable)

> 日期:2026-06-10。
> 基线:`4d12c43`。验收对象:`phase-1/usable`(HEAD `e4acd17` + 本文档 §3 的跟进修复)。
> 验收方式:**独立对抗性核验**(逐条看实际 `git diff` / 源码,而非仅信 `PROGRESS.md` 台账)
> 叠加**客观编译门实测**(三个 workspace 的 `clippy -D warnings` + `test`,本机亲跑)。
>
> 配套文档:逐条 finding 定义见 [`CODE_AUDIT.md`](./CODE_AUDIT.md);实施台账见 [`PROGRESS.md`](./PROGRESS.md)。

---

## 0. 总体结论

**验收通过。** Codex 在 `phase-1/usable` 上的工作**质量高且诚实**:客观门全绿、**无假修、无趁拆分改坏、台账无虚报 done**。我交付前标记的头号阻塞 **P0.0(基线 13+2 个 clippy 错误)已被第一步 `7406a14` 解除**。

验收中发现的几个真问题已在本次**跟进修复**(§3)中处理。唯一无法本地闭环的是 **P1 打包的 Windows 实机冒烟**(需 Windows runner / 实机),其余 macOS 侧已闭环。

---

## 1. 客观门实测结果(亲跑,非台账自述)

| 门 | 结果 | 含义 |
|---|---|---|
| `cargo clippy --workspace --all-targets --locked -D warnings`(root) | **EXIT=0** ✅ | P0.0 基线刷绿确认 |
| `cargo clippy ...`(`app/src-tauri`,独立 workspace) | **EXIT=0** ✅ | ROB-4 那两处会让 Tauri 编译失败的 `flatten()` 已修 |
| `cargo clippy ...`(`vendor/sonora`) | **EXIT=0** ✅ | SON-3 gate 生效 |
| `cargo test`(root / app / sonora) | **全部 0 failed** ✅ | root ~280+、sonora ~670+ 测试全过 |

关键**回归测试真实存在且通过**(非占位):零分配证明(`processor_chain_alloc`、`vendor/sonora .../realtime_alloc`)、诊断 writer join 无 `.part` 残留、sonora 后端错误记账、命令超时杀子进程、毒化锁恢复。唯一 warning:`block v0.1.6` future-incompat(已知,纳入 TEST-2 依赖治理)。

---

## 2. 验收发现(5 组对抗性核验净结论)

> 方法:按文件聚集度分 5 组,逐条核验「是否真修 / 假修 / 改一半 / 趁机改坏 / 引入新问题」。

| Phase / 组 | finding | 判定 | 备注 |
|---|---|---|---|
| P0 CI 护栏 | TEST-1 / CFG-1 / TEST-3 | ✅ 真到位 | Tauri 后端用 `working-directory` 独立 clippy/build,正确绕开 root workspace 边界;LocalVQE 固定到 `de56a174…`;TEST-3 测试真断言 |
| P0 CI 护栏 | TEST-2 | 🟡 部分 | fmt/audit 范围合理,但 `cargo audit` 用 `set -euo pipefail` = 失败即阻断(激进);**CI 从未触发过 Actions**,首推可能红 |
| P1 端到端 | RUNTIME-1 | ✅ 真修 | `started` 事件带 `cli_version`+`supported_controls`,前端真 guard 并给明确「请重建 CLI」提示;有测试锁列表一致性 |
| P1 端到端 | RUNTIME-2 / PKG-1 | 🟡 部分(~75-80%) | macOS 代码层闭环(`pnpm smoke:tauri-bundle` 全绿);`externalBin`/`resources` 已配;`CARGO_MANIFEST_DIR` 仅剩 dev 回退末位;**剩余 gap 见 §4** |
| P2 拆巨石 | ARCH-1 | ✅ 干净机械拆分 | `main.rs` 剩 41 行;10 个拆分 commit 等量删/增,**无夹带逻辑修改** |
| P3 实时音质 | PERF-1 | ✅ 真修(证据稍窄) | 计数 allocator 真断言稳态 `process()` 零分配;但只覆盖 chain 本体 + Identity 节点 |
| P3 实时音质 | QUAL-1 | ⚠️→**已补**(§3) | 有状态 rubato 替换无状态线性(改对了,立体声保留);但 SRC 延迟未计入 + 缺连续性测试 → 本次已补 |
| P3 实时音质 | QUAL-3 / QUAL-4 | ⚠️ 真修但增益有限 | QUAL-3 `make_contiguous` 仍 O(n) memmove(省了每帧 `vec!`);QUAL-4 桶边界用预估帧数,drop 时小视觉偏差(峰值数值仍等价) |
| P3 实时音质 | ROB-2 / SON-1 / SON-2 | ✅ 真修 | writer join 无 `.part`;sonora 错误记入 stats(注:capture 出错时 bypass=直通原麦含回声,需前端展示 `last_backend_error`);realtime_alloc 真计数断言 0 |
| P4 安全健壮 | SEC-1~6 / ROB-1 / ROB-3 / QUAL-2 / DOC-1 | ✅ 真修 | 校验逻辑代码+测试双确认;内置 pin 优先(`ensure!` 立即失败);O_EXCL 唯一临时文件;库不搜 CWD;模型下载校验 size+SHA256 |
| P4 安全健壮 | ROB-4 | ❌→**已补**(§3) | Tauri 侧改了,但 `probe_delay.rs` 仍 `map_while(Result::ok)` 吞错,且台账自报「无残留」不实 → 本次已补 |
| P5 前端延迟 | FE-1 / FE-2 / QUAL-5 | ✅ 真修 | rAF 隐藏/停机暂停 + ResizeObserver;`useSyncExternalStore` 隔离;DSP helper 去重 |
| P5 前端延迟 | LAT-1 | 🟡 部分 + 诚实改名 | 补了 `mic_q`,仍漏设备硬件缓冲/IO 重采样残值;label 收窄为「Pipeline / 管线延迟」 |
| P6 架构 | ARCH-2 / ARCH-3 / FE-3 | ✅ 真生效非空桩 | core 死代码删净;stdin 热控**真落地**——vendored Sonora `apply_config` 真在线 reinit NS/AGC2 子模块,**不重建 APM** |

**没有发现任何「台账标 done 实则未做 / 张冠李戴 / 空提交」。** 抽查的 closing SHA 全部存在且改动对得上 finding。

---

## 3. 本次跟进修复(已实测验证)

针对 §2 中标 ❌/⚠️ 的问题做了 4 项修复,全部通过 `cargo clippy --workspace -D warnings` + `cargo test --workspace`(EXIT=0):

### 3.1 ROB-4 残留 — `probe-delay` reader 不再静默吞错
- **问题**:`crates/echoless-cli/src/probe_delay.rs` 的 `spawn_probe_line_reader` 仍用 `lines().map_while(Result::ok)`(此前 ROB-4 只修了 Tauri 侧,CLI 侧遗漏;台账自报「无残留」不实)。
- **改法**:改为显式 `match line { Ok => sender, Err => eprintln! 警告 + break }`,IO 错误/非 UTF-8 行不再无声丢失。
- **验证**:`grep` 确认全 `crates/echoless-cli/src/` 无 `map_while(Result::ok)`/`lines().flatten()` 残留。

### 3.2 QUAL-1 延迟补偿 — 节点边界 SRC 延迟计入管线延迟
- **问题**:节点边界 rubato `FftFixedIn` 的固有延迟(LocalVQE 48k↔16k 往返约 10ms)未计入 `total_latency_ms()`(`chain.rs` 注释自承待补)→ 首页延迟偏低、AEC3 stream-delay 提示偏。
- **改法**(`crates/echoless-processors/src/chain.rs` + `crates/echoless-cli/src/realtime.rs`):
  1. 新增 `BoundaryAdapter::latency_ms()`,读 rubato `Resampler::output_delay()`(以输出侧帧数计,折算 ms;同采样率/未建立时为 0)。
  2. `total_latency_ms()` 累加**主信号路径**(`near_in` 进节点 + `near_out` 回 base)的 SRC 延迟;`far_in`(并行参考路径)不计入 mouth-to-ear 输出延迟。
  3. 新增 `ProcessorChain::warm_up(frames)`:实时启动前跑一帧静音让 resampler 按 `frame_size` 建立(随后 `reset` 清除预热影响、保留 resampler 实例),使 `total_latency_ms()` **在首帧前即准确**;`realtime.rs` 启动处调用。
- **下游自动受益**:该值经 `started` 事件 / `RealtimeStatsConfig` / `estimated_user_latency_ms` 传播,**无需改 stats 路径或现有测试**。
- **验证**:新增单测 `total_latency_includes_boundary_src_delay_after_warmup`(预热前 0、预热后 >0)与 `chain_resampler_preserves_block_boundary_continuity`(连续 440Hz 正弦逐块送,稳态相邻样本差 `max_step < 0.05`)——**均通过**,客观证明有状态 rubato 的块边界确实连续。

### 3.3 PKG-1 退化包 fail-fast 信号
- **问题**:`app/scripts/prepare-tauri-assets.mjs` 缺 LocalVQE 资产时仅 `console.warn`,普通 `pnpm tauri build`(`beforeBuildCommand` 未带 `--require-localvqe-assets`)会**静默**产出无 LocalVQE 的退化包。
- **改法**:非 dev(发布)且缺资产时打印醒目多行 banner(`RELEASE BUNDLE WILL SHIP WITHOUT LOCALVQE` + 缺失项 + 修复指引);保留退出码 0 以不破坏「只想打包测 UI」流程,`--require-localvqe-assets` 仍作硬失败开关。
- **验证**:`node --check` 通过。

### 3.4 文档/台账更正
- `PROGRESS.md` 新增「2026-06-10 验收复核与跟进修复」段,记录上述 3 项 + 一条澄清:`library_candidates`(`localvqe.rs`)**未**拓宽至 `app_local_data`/资源目录,打包态由 Tauri 端注入 `ECHOLESS_LOCALVQE_LIBRARY` 绝对路径解决(backend 不自搜 bundle)——避免误导后续审计。

---

## 4. 仍待办(交接给后续 agent / 人)

**必须 Windows 实机 / CI(本地 macOS 无法闭环):**
- Windows NSIS installer 安装后冒烟(`docs/windows-testing/WINDOWS_INSTALLED_APP_SMOKE_HANDOFF.md` 是 close path,链路从未实跑)。
- Windows LocalVQE DLL + ggml 后端的现场获取/签名(prepare 脚本只靠 env / `RUNNER_TEMP` 兜底,实机首次构建会卡)。
- Intel-mac(`x86_64-apple-darwin`)sidecar:无 CI 矩阵证据,仅同 host 拼。

**建议跟进(非阻塞,可排期):**
- **CI 首次跑通**:`PROGRESS.md` 自承 P0 从未触发 Actions;3-lock `cargo audit` + `set -euo pipefail` + 已知 `block v0.1.6` future-incompat → 首推大概率红。建议先 push 废分支或 `act` 本地验证再合 `main`。
- **LAT-1 余项**:设备硬件缓冲 / IO 重采样残值仍未计入「管线延迟」;至少在 UI tooltip 标注「不含设备缓冲」,或补 `device_*_buffer_ms` 估算项。
- **PERF-1 覆盖**:`LocalVqe::process` 无 alloc 计数测试(回归无拦截)。
- **SON-1 前端**:capture 后端错误时 bypass=直通含回声原麦,前端需醒目展示 `last_backend_error`。
- **次要**(SHA/等级已缓解,记录在案):SEC-5 的 `curl` 仍走 PATH(与 SEC-6 风格不一致);FE-2 共享 listener pool(4 个 memo 边界仍每 80ms 重渲,符合 Low 等级)。

---

## 5. 工程纪律观察(仅记录,非缺陷)

- **分支策略**:本轮 ~50 个 commit 全部落在 `phase-1/usable` 一条分支(用户知悉的工作方式),`main` / `phase-0/green-baseline` 仍停在 `47b3498`。代价:无法「按 Phase 单独回滚 / review」,回滚粒度 = 整支或单 commit。
- **提交信息**:前 ~35 个 commit 严守 `<type>(<ID>): …` 格式且原子;后 15 个热控系列(`edfa026` 起)丢了 ID 前缀、fix+feat 混提,溯源需反查 `PROGRESS.md`。
- **PERF-1 / QUAL-1** 共用 commit `59a9394`(commit ID 只标 QUAL-1),PERF-1 无独立 closing commit。

---

## 6. 后续同步指引

继续工作前先复现客观门(三个独立 workspace,各有自己的 `Cargo.lock`):

```bash
# root
cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
# Tauri 后端(独立 workspace)
(cd app/src-tauri && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked)
# vendored sonora(独立 workspace)
(cd vendor/sonora && cargo clippy -p sonora -p sonora-aec3 --all-targets --locked -- -D warnings && cargo test --locked)
# 前端
(cd app && pnpm exec tsc --noEmit)
# 打包(发布务必带 --require-localvqe-assets 以 fail-fast)
(cd app && pnpm prepare:tauri-assets --require-localvqe-assets && pnpm tauri build)
```

逐条 finding 状态以 [`PROGRESS.md`](./PROGRESS.md) 为准;问题定义与修复指令以 [`CODE_AUDIT.md`](./CODE_AUDIT.md) 为准。
