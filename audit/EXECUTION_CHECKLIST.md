# Echoless 审计修复待执行清单（第一、第二梯队）

> 基线：`dev@1aa747708d4d8eac8d732c7ed8c827e22d744508`
> 来源：`audit/AUDIT_REVIEW.md`、`audit/DECISIONS.md`
> 状态：**用户已批准执行（2026-07-10）；按本清单逐项修复并以 RC4 发布闭环**
> 执行原则：一个审计条目一个 commit；B-25/B-29 同批联调但分别提交；B-28/A-09 共用事件设计但分别收口
> 平台边界：面向外部公开发布；接受 unsigned 且不改现有文案；不承担 Linux 代码维护或发布质量保证，但纠正公开 Linux 产物的事实性文档

## 范围

### 本清单包含

- 第一梯队：B-26、B-28、B-27、B-25、B-29、T-11。
- 第二梯队：A-09 缩水版、S-13、B-30、B-31、B-32、S-14、T-12、D-12、D-13、D-14。
- 每项的回归测试、批次质量门、提交边界和停止条件。

### 本清单明确不包含

- 第三梯队全部条目，包括 D-09、S-12、P-05、T-15、D-10、D-15、C-07、C-08、S-15、T-13/T-14、A-10、A-08。
- D-11：用户已裁决跳过，直接发布正式版 `v1.1.0`。
- Linux 代码专项修复、Linux LocalVQE fail-closed、Linux 发布质量保证；D-13 的两处事实性文档纠错除外。
- 签名、公证、SBOM/cargo-about、许可证自动化、schema/codegen、新依赖。
- 发版、打 tag、推送、创建 PR 或改 GitHub required checks；这些属于修复完成后的独立动作。

## 执行前保护

- [x] **PREFLIGHT-01** 固定工作区基线并记录现有改动。
  - 当前已有未提交 D-12 修改：`Cargo.toml`、`configs/example.toml`。
  - 当前 `audit/` 为 untracked 文档目录。
  - 执行时不得覆盖、重置或混入无关用户改动。

- [x] **PREFLIGHT-02** 创建 `audit/FIX_PROGRESS.md`，为每个条目记录：状态、commit、验证命令、结果、剩余风险。

- [x] **PREFLIGHT-03** 跑一次执行前基线并保存结果。
  - `cargo test --workspace --locked`
  - `(cd aec3 && cargo test --workspace --locked)`
  - `(cd app/src-tauri && cargo test --locked)`
  - `(cd app && pnpm exec tsc --noEmit && pnpm test)`

---

## 第一梯队

### 1. B-26【P1】Process Tap 失联/拒权后的零参考假运行

- [x] 在 `crates/echoless-cli/src/realtime/macos_process_tap.rs` 增加启动 ready/header 握手；helper 在 header 前退出时，`start()` 返回错误，不允许上层发出有效 `started`。
- [x] 区分主动关闭与意外 EOF/read error；意外失联设置共享停止状态，并发出可诊断的结构化 stream error。
- [x] 确保 Drop/正常停止不产生误报警，reader 与 child 生命周期只结束一次。
- [x] 增加 fault-injection 测试：header 前退出、header 后意外 EOF、read error、主动 Drop。
- [x] 验证：`cargo test -p echoless-cli --locked`，并检查 macOS 正常 Process Tap 启动路径不回归。
- [x] 独立 commit：`B-26`。

### 2. B-28【P1】旧 sidecar exit 污染新 run

- [x] 在 `app/src-tauri/src/proc.rs` / `sidecar.rs` 为每次 run 分配单调 `run_id`，并保存 active generation。
- [x] 让 status/exit 事件携带 `run_id`；只有 active run 可更新 RunState、tray tooltip 和全局运行状态。
- [x] 在 `app/src/App.tsx` 保存当前 active `run_id`，忽略旧代 status/exit；intentional exit 的清理也必须通过代际检查。
- [x] 增加确定性 barrier 测试：卡住 run A reader，启动 B 后释放 A，确认 B、tray、I/O、runtime controls 不变。
- [x] 验证：`(cd app/src-tauri && cargo test --locked)`、`(cd app && pnpm exec tsc --noEmit && pnpm test)`。
- [x] 独立 commit：`B-28`。

### 3. B-27【P1】LocalVQE 错误态无界缓冲与旧音频回放

- [x] 在 `crates/echoless-processors/src/localvqe.rs` 明确 native error 策略：失败 hop 必须被消费，或清空 near/far/out 并 reset 流状态。
- [x] 保证错误期间继续 near passthrough，但内部队列长度始终有界。
- [x] 保证恢复后从当前时间点重新起流，不输出错误发生前积压的样本。
- [x] 增加连续 error、瞬态恢复、重复恢复测试，并断言 queue 上限与样本时间顺序。
- [x] 验证：`cargo test -p echoless-processors --locked`、`cargo clippy -p echoless-processors --all-targets --locked -- -D warnings`。
- [x] 独立 commit：`B-27`。

### 4. B-25【P1】stereo reference 半帧提交与奇偶破坏

- [x] 将 reference producer/consumer 语义改为完整 frame 原子操作；容量不足时整帧丢弃，不允许只提交 L 或只消费 ch0。
- [x] 修复旧 `skip_stale` 路径：stereo 丢弃数必须按完整 frame 对齐，不能丢奇数 samples。
- [x] 统一 CPAL reference 与 Process Tap reference 的 frame push/drop 规则。
- [x] 增加单 L 提交、仅余一个 slot、奇数 stale count、并发交错、高频 overflow 测试。
- [x] 验证：`cargo test -p echoless-cli --locked`，确认 mono 路径行为不变。
- [x] 独立 commit：`B-25`。

### 5. B-29【P2】clock-skew 单向失明与 stereo 计数单位错误

- [x] 将 output/reference 的 underrun、overrun、drop 全部统一为 frames，不再混用 interleaved samples。
- [x] 同时检测两个漂移方向；使用真实欠载帧数，不把计数压缩为布尔值后再参与相关性计算。
- [x] 让 live detector 与 diagnostics summary 共用计算逻辑，并输出方向。
- [x] 增加 ±22.4% 双向漂移、stereo 2N samples→N frames、T3 开关一致性测试。
- [x] 与 B-25 完成集成联调，确认 frame 对齐修复不会改变告警阈值语义。
- [x] 验证：`cargo test -p echoless-cli --locked`。
- [x] 独立 commit：`B-29`。

### 6. T-11【P1】Tauri 后端 21 tests 接入 CI

- [ ] 在 `.github/workflows/build.yml` 的 Windows/macOS build matrix 中加入 `(cd app/src-tauri && cargo test --locked)`。
- [ ] 不新增 Linux 质量承诺；不为 Linux 单独补测试或 fail-closed 门。
- [ ] 保留现有 clippy/build，不能用 test 替代静态检查或构建 smoke。
- [ ] 本地验证 21 tests 全执行，并确认 workflow YAML 可解析。
- [ ] 推送后验证 supported-platform jobs 的 Tauri tests 日志；预计净增约 5 秒。
- [ ] 独立 commit：`T-11`。

### 第一梯队批次门

- [ ] `cargo fmt --all --check`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `(cd aec3 && cargo fmt --all --check && cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings)`
- [ ] `(cd app/src-tauri && cargo fmt --check && cargo test --locked && cargo clippy --all-targets --locked -- -D warnings)`
- [ ] `(cd app && pnpm exec tsc --noEmit && pnpm test && pnpm build)`
- [ ] 检查 `git status --short`，确认只有计划内改动。

---

## 第二梯队

### 7. A-09 缩水版【P3】补齐前端事件类型

- [ ] 在 `app/src/types.ts` 补完整事件成员：`stream_error`、`clock_skew_warning`、`clock_skew_resolved`、serde 序列化失败兜底的 `error`。
- [ ] 为 clock-skew 事件声明 `output_skew_pct`、`ref_skew_pct`、`ref_correlated`、`hint`；为 `RuntimeStatus` 补可选 `clock_skew_ref_correlated`。
- [ ] 将 `control_error.cmd` 改为 `string | null`，并给 null command 提供明确显示文案。
- [ ] 复用 B-28 的 `run_id`，不引入 schema/codegen/golden fixtures。
- [ ] 保持现有 unknown-event 白名单与 ErrorBoundary 行为不变。
- [ ] 增加类型/消费测试，覆盖四类新增事件、`control_error.cmd=null` 与包含 `clock_skew_ref_correlated` 的 status。
- [ ] 验证：`(cd app && pnpm exec tsc --noEmit && pnpm test)`，并逐项对照 Rust 当前发出的全部 `type` discriminator。
- [ ] 独立 commit：`A-09`。

### 8. S-13【P2】ScrambleText 设备名 HTML 注入

- [ ] 修改 `app/src/components/ScrambleText.tsx`，动画全程只写安全文本，不再以 `innerHTML` 为动画目标。
- [ ] 保留 scramble 动画、语言切换与中断清理行为。
- [ ] 用 `<>&"'`、`<style>`、`<meta http-equiv=refresh>` payload 做手工 WebView/浏览器回归，确认只显示文本、无新增 DOM element、无外部请求。
- [ ] 验证：`(cd app && pnpm exec tsc --noEmit && pnpm test && pnpm build)`。
- [ ] 独立 commit：`S-13`。

### 9. B-30【P3】CoreAudio listener remove 失败后的 context 生命周期

- [ ] 在 `app/src-tauri/src/device_watch.rs` 检查 remove 的 `OSStatus`；只有确认移除成功后才释放 callback context。
- [ ] 失败路径保留 ownership 并记录 OSStatus，避免 UAF；不扩大为新的常驻泄漏循环。
- [ ] 增加 remove success/failure 的可注入测试或最小纯逻辑测试。
- [ ] 验证：`(cd app/src-tauri && cargo test --locked && cargo clippy --all-targets --locked -- -D warnings)`。
- [ ] 独立 commit：`B-30`。

### 10. B-31【P3】TOML string 控制字符转义

- [ ] 在 `app/src/api.ts` 完整处理 TOML basic string 不允许的控制字符；不只做模糊删除，必须保证合法字符 round-trip。
- [ ] 增加 LF、CR、BS、FF、NUL、ESC、DEL、quote、backslash、Unicode 测试。
- [ ] 确认引号/反斜杠仍安全，不引入配置注入或静默数据损坏。
- [ ] 验证：`(cd app && pnpm exec tsc --noEmit && pnpm test)`，并让生成结果通过后端 TOML validation。
- [ ] 独立 commit：`B-31`。

### 11. B-32【P3】日志文件名秒级碰撞

- [ ] 在 `app/src-tauri/src/logging.rs` 为日志文件名加入 PID 与高精度/冲突 attempt，使用 `create_new(true)` 保证一启动一文件。
- [ ] 保持单文件 8 MiB、7 天、20 文件的现有清理策略。
- [ ] 增加同秒并发 init/冲突测试，确认文件不共享且 cap 独立生效。
- [ ] 验证：`(cd app/src-tauri && cargo test --locked)`。
- [ ] 独立 commit：`B-32`。

### 12. S-14【P3】URL allowlist 标准化解析

- [ ] 用已有 Tauri/URL 类型替换 `app/src-tauri/src/platform.rs` 的手写 authority parser；不新增依赖。
- [ ] 拒绝 credentials，按规范化后的 scheme/hostname/port 检查 allowlist。
- [ ] 扩展现有测试：backslash、userinfo、encoded delimiter、port、大小写、尾点与现有合法 URL。
- [ ] 验证：`(cd app/src-tauri && cargo test --locked)`。
- [ ] 独立 commit：`S-14`。

### 13. T-12【P2】增加 PR 自动质量门

- [ ] 为 `.github/workflows/build.yml` 增加 `pull_request` 触发，覆盖目标为 `main`/`dev`。
- [ ] 不新增 Linux 专项保证；保持现有 job 结构，不在本条重构昂贵打包步骤。
- [ ] 本地检查 workflow YAML；推送测试 PR 后确认基础 quality jobs 自动出现。
- [ ] 独立 commit：`T-12`。

### 14. D-12【P2】收口已经存在的 example.toml/SRC 注释修复

- [ ] 复核当前 `Cargo.toml`、`configs/example.toml` diff，只包含 rubato FFT 与 GUI/CLI 资产分发说明纠错。
- [ ] 对照 `crates/echoless-processors/src/chain.rs`、`.github/workflows/build.yml` 与 `docs/CLI.md`，确认新文案准确。
- [ ] 运行 root fmt/test 或最小文档相关质量门，记录 D-12 已修。
- [ ] 将现有修改作为独立 commit 收口，不与其他条目混合。

### 15. D-13【P3】Linux 数据目录大小写文档

- [ ] 将 `docs/CLI.md`、`docs/CLI.zh-CN.md` 的 Linux 数据根从 `~/.local/share/echoless` 改为实现使用的 `~/.local/share/Echoless`，并保留 `XDG_DATA_HOME` 语义。
- [ ] 仅纠正公开 deb/AppImage/CLI 用户会读取的事实性路径；不修改 Linux 代码，不新增 Linux 支持或质量保证。
- [ ] 独立 commit：`D-13`，不与 D-14 混合。

### 16. D-14【P3】app README sidecar 解析顺序

- [ ] 按 `app/src-tauri/src/bin_resolve.rs` 的真实顺序更新 `app/README.md` 与 `app/README.zh-CN.md`：env、当前 exe 邻接、Tauri resource、target-triple binaries、root release/debug。
- [ ] 删除“打包后由 env 注入”的错误说法。
- [ ] 独立 commit：`D-14`。

### 第二梯队批次门

- [ ] 重跑第一梯队全部质量门。
- [ ] `(cd app && pnpm tauri build --debug --no-bundle --ci)`。
- [ ] 检查所有新测试确实被执行，不接受只编译未运行。
- [ ] 检查 `git log --oneline`：每个条目一个 commit，提交信息遵循 Lore protocol。
- [ ] 检查 `git status --short`：无未说明文件、无覆盖用户已有改动。
- [ ] 更新 `audit/FIX_PROGRESS.md`，逐项记录完成证据与未解决风险。

## 停止条件

- [ ] 任何修复需要新增依赖、改变公开协议范围、改变 unsigned/文案裁决、扩大到 Linux 代码或质量保证（D-13 文档纠错除外）或触碰第三梯队时，立即停止并回报。
- [ ] 任何现有质量门从绿变红时，不进入下一条；先修复当前条目或回滚该条 commit。
- [ ] B-25/B-29 若无法维持 mono 行为与现有告警阈值，停止联调并提交证据，不自行改产品阈值。
- [ ] B-28 若 `run_id` 需要 schema/codegen 才能可靠落地，停止并重新提交方案；当前裁决禁止引入 codegen。

## 审核结果

- [x] 用户批准按本清单执行（2026-07-10）。
- [ ] 用户要求调整清单后再审。

审核备注：`待填写`
