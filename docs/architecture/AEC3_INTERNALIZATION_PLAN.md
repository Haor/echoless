# AEC3 引擎内化方案(去 sonora 化)

状态:已执行(2026-07-04),上游基线 aacadf0,详见 docs/codex-tasks/TASK_P3_AEC3_INTERNALIZE.md。

日期:2026-07-03 · 状态:方案(待执行)
目标:将 `vendor/sonora` 彻底内化为项目自有引擎,**全仓不再出现 `sonora` 字样**。
前提:本地自用,无 license 顾虑([[aec-self-use-no-license]]),fork 可任意改名。

## 0. 现状快照

- `vendor/sonora` 是 WebRTC AEC3 的 Rust 移植 fork(方案 A 已落地,commit 919796c):独立子 workspace(7 个 crate),echoless 顶层 `exclude` + path 依赖。
- fork 改动全部带 `// Echoless:` 标注,集中在 config 注入口(`audio_processing_impl.rs:169,206-207,1291-1302`、`audio_processing.rs` builder、`lib.rs:31` 重导出)——**与改名正交,不受影响**。
- **UI 显示名已经是 "AEC3"**(label/name/i18n 均无 sonora),用户可见层零改动。
- **前后端均不持久化 kind 字符串**:后端只写临时 toml,前端 localStorage 只存设备/语言。`"sonora_aec3"` 硬切的破坏面仅限仓库内 `configs/example.toml` 和用户手写配置。
- `vendor/sonora` **自带独立 `.git`**,内化前必须处理。

## 1. sonora 字样分布(清点结果)

| 区域 | 规模 | 关键位置 |
|---|---|---|
| vendor 本体 | 7 个 crate 名 + **~123 处跨 crate `use sonora_*`** + Makefile.toml/codecov.yml/README/benches/examples | `vendor/sonora/Cargo.toml`(workspace.dependencies 7 项) |
| Rust 集成层 | 依赖名/feature/文件名/类型名,~50 处 | `echoless-processors/Cargo.toml`(dep+feature `sonora-engine`)、`src/sonora_aec3.rs`(文件名+`SonoraAec3`+~40 处 `sonora::`)、`registry.rs:4,11,15,22`、`lib.rs:15` |
| kind 字符串 `"sonora_aec3"` | 跨层契约,~25 处 | `echoless-core/src/lib.rs:232`、`config_validate.rs:341,349,678,697,753`、`processor_manifest.rs:21,69,191`、`run_command.rs:85-229`、`realtime/control.rs:417-457`、`cli.rs:91-144`、`configs/example.toml:26` |
| 前端 | 8 处 kind 值 | `types.ts:61`、`App.tsx:147,283,837,852,864`、`EnginePage.tsx:45`、`AdvancedPage.tsx:88,121` |
| 测试/CI | 2 个测试文件 + CI job | `tests/sonora_direct.rs`(文件名+10)、`echo_cancellation.rs`(8)、`.github/workflows/build.yml:78-84,115` |
| 文档 | ~20 个 md,~160 处 | `docs/research/sonora_aec3_internal_map.md`(文件名+27)等 |
| 生成物 | 自动跟随 | 两处 Cargo.lock、app/dist、graphify-out、编译二进制 |

版权情况:LICENSE(BSD-3,署名 WebRTC Project Authors + Arun Raghavan)**不含 sonora 字样**,建议保留;60 个 .rs 的 doc 注释含 "webrtc" 属技术出处描述,不在去名范围。

## 2. 改名映射(定案)

| 项 | 现值 | 新值 |
|---|---|---|
| vendor 目录 | `vendor/sonora` | `vendor/aec3` |
| 顶层 crate | `sonora` | `aec3-apm`(高层 APM 封装,避免与子 crate `aec3` 撞名) |
| 子 crate | `sonora-aec3` / `sonora-common-audio` / `sonora-agc2` / `sonora-ns` / `sonora-simd` / `sonora-fft` | `aec3-core` / `aec3-common-audio` / `aec3-agc2` / `aec3-ns` / `aec3-simd` / `aec3-fft` |
| feature | `sonora-engine` | `aec3-engine` |
| 处理器文件/类型 | `sonora_aec3.rs` / `SonoraAec3` / `process_sonora` | `aec3.rs` / `Aec3Engine` / `process_aec3` |
| **kind 字符串** | `"sonora_aec3"` | `"aec3"`(+ 兼容别名,见 §3) |
| 测试文件 | `sonora_direct.rs` | `aec3_direct.rs` |
| 内部地图文档 | `sonora_aec3_internal_map.md` | `aec3_internal_map.md`(内容批量替换术语) |

注:前端 `aec3_ns_changed` 等既有 `aec3_*` 事件名**不动**(本来就没有 sonora)。

## 3. kind 字符串兼容策略

- `registry.rs` 的 match 同时接受 `"aec3"` 与 `"sonora_aec3"`(别名),映射同一构造器;`config_validate.rs`/`run_command.rs`/`core::lib.rs:232` 的比较处同样接受两者。
- 前端、manifest、`configs/example.toml`、CLI 帮助文本全部切到 `"aec3"`。
- 别名保留 1-2 个版本(防手写旧配置炸掉),之后删除。由于从未对外分发旧 kind 配置,如果确认不需要,可直接硬切跳过此步。

## 4. 执行顺序(8 步)

1. **处理 vendor `.git`**:删除 `vendor/sonora/.git`,并入主仓历史(彻底内化的语义;上游 commit 基线 aacadf0 记入本文档即可)。
2. **vendor 内部自洽改名**:目录 → 各 Cargo.toml package name → `[workspace.dependencies]` → 全量 `use sonora_*`/`sonora::`(~123 处,sed 批量)→ Makefile.toml/codecov.yml/README/benches/examples/tests。改完在 `vendor/aec3` 内 `cargo build && cargo test -p aec3-apm -p aec3-core` 自测。
3. **集成层**:`echoless-processors/Cargo.toml`(path/dep/feature)→ `sonora_aec3.rs` 重命名 + 类型/`use`/feature gate → `lib.rs`/`registry.rs`。确认 `builder.aec3_config(...)` 仍指向 fork 注入口。
4. **kind 切换**:§1 表中 ~25 处 + `configs/example.toml`,同时落 §3 别名。
5. **前端**:8 处 kind 值(显示名不动)。
6. **测试/CI**:测试文件重命名;`build.yml` job 名、`working-directory: vendor/aec3`、`-p` 包名、cargo audit 路径。
7. **文档**:`sonora_aec3_internal_map.md` 重命名+术语替换;其余 ~20 个 md 批量替换(`sonora` → `aec3`/`vendor aec3`,按上下文)。
8. **重生成 + 复核**:`cargo build`(刷两处 lock)、前端 build、重跑 graphify;最终 `grep -ri sonora --exclude-dir={target,node_modules,.git,graphify-out}` 归零(或仅剩 §3 别名字面量,别名处加注释说明)。

## 5. 风险与回滚

- **主要风险在第 2 步**(vendor 内 123 处引用):纯机械替换,靠 `cargo build` 全量兜底,编译过 = 改全了。
- kind 别名兜住配置兼容;probe 子进程用 `--processor passthrough`,不经过 kind,无影响。
- 回滚:整个改名是一个纯 rename commit,git revert 即可;建议与任何功能改动(如延迟魔改)**分开提交**,先改名后魔改,保证 diff 可读。

## 6. 与延迟魔改的顺序关系

见 `AEC3_DELAY_MOD_PLAN.md`。**先内化改名、后魔改**:魔改要往 `vendor` 核心加 config 字段和逻辑,在改名后的干净命名空间上做,避免 rename 与逻辑改动混在同一批 diff 里。
