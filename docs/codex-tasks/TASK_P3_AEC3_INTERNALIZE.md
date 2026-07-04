# Codex 任务规格 P3:aec3 内化改名(全仓去 aec3 化)

日期:2026-07-04 · 执行者:Codex(gpt-5.5,bulk/mechanical)· 工作树:`echoless-aec3/`(分支 `phase-2/aec3-internalize` ← main)
背景方案:`docs/architecture/AEC3_INTERNALIZATION_PLAN.md`(2026-07-03)。**本文档的清点数字已于 2026-07-04 逐项核实校准,与方案冲突时以本文档为准。**

## 目标

`vendor/aec3`(WebRTC AEC3 Rust 移植 fork)彻底内化为项目自有引擎。完成后全仓旧品牌字样扫描只允许两类残留:
1. kind 兼容别名字面量(带注释说明);
2. `docs/architecture/AEC3_INTERNALIZATION_PLAN.md` 内的历史记录。

这是**纯机械改名**,不改任何逻辑。改名与逻辑改动(后续延迟魔改)必须分开提交。

## 现状快照(2026-07-04 已核实)

- `vendor/aec3/.git` **不存在**,205 个文件已被主仓正常 track,无 submodule/gitlink。**方案第 1 步(处理 .git)跳过**,只需在本文档记录上游基线 commit aacadf0。
- vendor 是独立子 workspace(根 Cargo.toml `exclude = ["vendor/aec3"]`),7 个 member:`aec3`, `aec3-core`, `aec3-common-audio`, `aec3-agc2`, `aec3-ns`, `aec3-simd`, `aec3-fft`;`[workspace.dependencies]` 同名 7 项(path 依赖)。
- vendor 内 `use aec3*` 语句共 **78 处**(aec3_simd 18 / aec3 14 / aec3 13 / aec3_common_audio 12 / aec3_agc2 10 / aec3_ns 6 / aec3_fft 5);vendor aec3 字样总计 326 处(66 文件)。
- fork 自有改动带 `// Echoless:` 标注,集中在 config 注入口(`vendor/aec3/crates/aec3-apm/src/audio_processing_impl.rs:169, 206-208, 1291-1302`、`vendor/aec3/crates/aec3-apm/src/lib.rs:34-35` 重导出)——与改名正交。
- UI 显示名已是 "AEC3",用户可见层零改动;前后端均不持久化 kind 字符串。

## 改名映射(定案)

| 项 | 现值 | 新值 |
|---|---|---|
| vendor 目录 | `vendor/aec3` | `vendor/aec3` |
| 顶层 crate | `aec3` | `aec3-apm`(避免与子 crate `aec3` 撞名) |
| 子 crate | `aec3-core` / `aec3-common-audio` / `aec3-agc2` / `aec3-ns` / `aec3-simd` / `aec3-fft` | `aec3-core` / `aec3-common-audio` / `aec3-agc2` / `aec3-ns` / `aec3-simd` / `aec3-fft` |
| crate 目录 | `crates/aec3`, `crates/aec3-core`, … | `crates/aec3-apm`, `crates/aec3-core`, …(目录随 crate 名) |
| feature(echoless-processors) | `aec3-engine` | `aec3-engine`(default 同步) |
| 依赖名(echoless-processors) | `aec3 = { path = "../../vendor/aec3/crates/aec3", optional = true }` | `aec3-apm = { path = "../../vendor/aec3/crates/aec3-apm", optional = true }` |
| 处理器文件/类型 | `src/aec3.rs` / `Aec3Engine` / `mod aec3` | `src/aec3.rs` / `Aec3Engine` / `mod aec3` |
| **kind 字符串** | `"aec3"` | `"aec3"` + 兼容别名(见下) |
| 测试文件 | `tests/aec3_direct.rs` | `tests/aec3_direct.rs` |
| 内部地图文档 | `docs/research/aec3_internal_map.md` | `docs/research/aec3_internal_map.md`(内容术语同步替换) |

注意:前端 `aec3_ns_changed` 等既有 `aec3_*` 事件名**不动**(本来就没有 aec3)。Rust 里 `use aec3_apm::...` → `use aec3_apm::...`(crate 名带连字符,代码里是下划线 `aec3_apm`)。

## kind 兼容策略

`"aec3"` 为新 kind;以下比较处**同时接受** `"aec3"` 作别名(每处加注释 `// legacy alias, remove after 2 releases`):
- `crates/echoless-processors/src/registry.rs`(match 构造器)
- `crates/echoless-cli/src/config_validate.rs` 的 kind 校验
- `crates/echoless-cli/src/run_command.rs` 的 kind 比较
- `crates/echoless-core/src/lib.rs:232` 附近的比较

其余所有地方(前端、manifest、example.toml、CLI 帮助文本、测试)全部切到 `"aec3"`。

## 改动点清单(2026-07-04 核实的精确位置)

### A. vendor 本体(66 文件 326 处)
1. 目录改名 `vendor/aec3` → `vendor/aec3`,7 个 crate 子目录按映射表改名。
2. 各 Cargo.toml 的 `package.name` + workspace `members` + `[workspace.dependencies]`。
3. 全量 `use aec3_*` / `use aec3_apm::`(78 处)+ 代码内路径引用,sed 批量。
4. Makefile.toml / codecov.yml / README / benches / examples / `crates/aec3/tests/realtime_alloc.rs`(该文件是零分配集成测试,包名引用要跟上)。
5. vendor 各 Cargo.toml 里指向 `research/aec3_internal_map.md` 的注释路径同步改。
6. 根 `Cargo.toml` 的 `exclude = ["vendor/aec3"]` → `vendor/aec3`。
7. LICENSE 文件(7 个,BSD-3,署名 WebRTC Project Authors / Arun Raghavan)**内容不动**(不含 aec3 字样),保留原位。

### B. 集成层(Rust)
- `crates/echoless-processors/Cargo.toml`:依赖名/path/feature(15,16 行注释一并改)。
- `crates/echoless-processors/src/aec3.rs` → `src/aec3.rs`:`Aec3Engine`→`Aec3Engine`(60 行 struct + impl 70/84/90/324/334),`use aec3_apm::`(223, 273, 301),文件内 40 处字样;80, 92, 414 的 name/kind 字面量。
- `crates/echoless-processors/src/registry.rs`:4, 11, 15, 22(+别名)。
- `crates/echoless-processors/src/lib.rs`:3(注释), 15(`pub mod`), 20(注释)。
- `crates/echoless-core/src/lib.rs`:232(+别名)。
- `crates/echoless-cli`:`cli.rs` 91-144 区间的 doc/注释 6 处;`config_validate.rs` 341, 678, 697, 753(+别名);`processor_manifest.rs` 21, 69, 191;`realtime/control.rs` 417, 420, 435, 446, 457;`run_command.rs` 123, 124, 182, 200, 229(+别名);`main.rs`/`realtime.rs` 的零散 aec3 字样。
- `configs/example.toml:26` `kind = "aec3"` → `"aec3"`。

### C. 前端(9 处 4 文件,全部切新值,不留别名)
- `app/src/App.tsx`:147, 283, 837, 852, 864
- `app/src/types.ts`:61(联合类型成员)
- `app/src/pages/EnginePage.tsx`:45
- `app/src/pages/AdvancedPage.tsx`:88, 121

### D. 测试/CI
- `crates/echoless-processors/tests/aec3_direct.rs` → `aec3_direct.rs`(106 行 kind 等 10 处);`tests/echo_cancellation.rs` 1, 6, 18, 40, 111;`tests/processor_chain_alloc.rs` 若含 aec3 一并。
- `.github/workflows/build.yml`:79/83 `working-directory: vendor/aec3`、80 `cargo test -p aec3-apm -p aec3-core` → `-p aec3-apm -p aec3-core`、84 clippy 同理、115 `(cd vendor/aec3 && cargo audit)`。

### E. 文档(23 个 md,279 处)
- `docs/research/aec3_internal_map.md` 重命名 + 内容术语替换(32 处);所有引用该路径的文件(vendor Cargo.toml、processors Cargo.toml、aec3.rs 注释等)同步。
- 其余 md 按上下文替换:描述**本仓引擎**的 `aec3`→`aec3`;**唯一例外**:`docs/architecture/AEC3_INTERNALIZATION_PLAN.md` 保留原文(它是改名操作本身的历史记录),在其顶部加一行状态注记「已执行(2026-07-XX),上游基线 aacadf0,详见 docs/codex-tasks/TASK_P3_AEC3_INTERNALIZE.md」。
- `docs/architecture/AEC3_DELAY_MOD_PLAN.md` 中的路径/crate 名引用更新到新命名(它还要被后续任务使用,必须指向真实路径)。

### F. 生成物
- `cargo build` 刷新两处 Cargo.lock(根 + vendor);`app/dist`、`graphify-out`、编译产物不手改(会重生成)。

## 执行顺序

1. vendor 内部自洽改名(A 全部)→ 在 `vendor/aec3` 内 `cargo build && cargo test -p aec3-apm -p aec3-core` 自测通过。
2. 集成层(B)→ 根 workspace `cargo build --workspace` + `cargo test -p echoless-processors`。确认 `builder.aec3_config(...)` 仍指向 fork 注入口(`vendor/aec3/crates/aec3-apm/src/audio_processing_impl.rs` 的 `set_aec3_config_override`)。
3. kind 切换 + 别名(B 中标注处 + example.toml)。
4. 前端(C)→ `cd app && pnpm build`(或 npm,以 lockfile 为准)。
5. 测试/CI(D)。
6. 文档(E)。
7. 重生成 + 复核(F + 验收)。

分批提交建议:vendor 改名一个 commit,集成层+kind 一个,前端一个,测试/CI+文档一个。**任何一步都不改逻辑。**

## 验收标准

1. `grep -rin aec3 --include='*' . --exclude-dir={target,node_modules,.git,graphify-out,dist}` 输出仅剩:kind 别名字面量(带 legacy 注释)+ `AEC3_INTERNALIZATION_PLAN.md`。
2. `cargo build --workspace` + `cargo test --workspace` 全绿(根 workspace)。
3. `cd vendor/aec3 && cargo build && cargo test -p aec3-apm -p aec3-core` 全绿(含 realtime_alloc 零分配测试)。
4. `cd app && pnpm install && pnpm build` 通过;`tsc` 无错。
5. 兼容性冒烟:手写一个 `kind = "aec3"` 的最小 config 跑 `echoless validate`(或等价校验命令)不报错;`kind = "aec3"` 正常。
6. 输出改动摘要:每个 commit 的文件数/替换数统计。

## 上游基线记录

vendor 来源:aec3(WebRTC AEC3 Rust port,Arun Raghavan),内化前上游基线 commit `aacadf0`,fork 自有改动均带 `// Echoless:` 标注。
