# Echoless `dev` 审计修复与 RC4 发布任务书

## 目标

在 `dev` 分支按 `audit/EXECUTION_CHECKLIST.md` 完成第一、第二梯队全部条目，逐项验证并独立提交；全量质量门和最终审查通过后，推送 `dev`，创建并推送 `v1.1.0-rc.4`，确认远端 CI、GitHub Release 与发布资产完整。

## 权威输入

优先级从高到低：

1. 用户在 2026-07-10 的最新裁决与本任务目标。
2. `audit/EXECUTION_CHECKLIST.md`。
3. `audit/DECISIONS.md`。
4. `audit/AUDIT_REVIEW.md`。
5. `audit/AUDIT_REPORT_2026-07-10.md`。

若文档与当前源码或 Git 状态冲突，以当前源码、测试、`git log`、`git status` 和远端状态为事实证据，并把差异写入 `audit/FIX_PROGRESS.md`。

## 固定基线

- 仓库：`/Users/harukishiina/workspace/codex/AEC/echoless`。
- 分支：`dev`，不新建分支或 worktree。
- 开始提交：`1aa747708d4d8eac8d732c7ed8c827e22d744508`。
- 开始时 `HEAD == origin/dev`，ahead/behind 为 `0/0`。
- 开始时最新本地与远端 RC tag：`v1.1.0-rc.3`；`v1.1.0-rc.4` 尚不存在。
- 既有未提交 D-12 改动：`Cargo.toml`、`configs/example.toml`。不得重置、覆盖或混入其他条目。
- 开始时 `audit/` 为未跟踪的审计交付目录。

## 执行纪律

- 一个审计条目一个 commit；审计任务书/账本可用独立的 audit-only commit 固定。
- B-25/B-29 同批联调但分别提交；B-28/A-09 共用 `run_id` 设计但分别提交。
- 每条先添加能在旧实现上暴露问题的回归覆盖，再实施最小修复；不得删除既有测试。
- 每条完成后立即运行专项验证，提交，并更新 `audit/FIX_PROGRESS.md`。
- 所有提交遵循仓库 Lore commit protocol，正文记录约束、拒绝方案、风险、测试和未测试项。
- 不新增依赖、schema/codegen、签名、公证、SBOM 或 Linux 质量承诺。
- D-13 仅纠正公开 Linux 产物的两处事实性路径文档，不扩大 Linux 代码维护范围。
- 不打 tag、不推送，直到所有条目、全量质量门和最终 diff 审查均通过。

## 执行顺序

### 第一梯队

1. B-26：Process Tap 启动握手与异常失联。
2. B-28：sidecar `run_id` 代际隔离。
3. B-27：LocalVQE 错误态有界缓冲与恢复。
4. B-25：stereo reference 完整帧原子语义。
5. B-29：clock-skew 双向检测与 frame 单位统一。
6. T-11：Windows/macOS Tauri 后端测试进入 CI。

### 第二梯队

7. A-09：补齐 TypeScript 运行事件契约。
8. S-13：ScrambleText 禁止 `innerHTML` 注入。
9. B-30：CoreAudio listener remove 失败生命周期。
10. B-31：TOML basic string 控制字符转义。
11. B-32：日志文件并发唯一性。
12. S-14：URL allowlist 标准解析。
13. T-12：增加 `pull_request` CI 触发。
14. D-12：收口既有 example/SRC 注释修复。
15. D-13：Linux 数据目录大小写文档。
16. D-14：sidecar 解析顺序文档。

## 验收与发布

专项验证、批次门和停止条件逐字执行 `audit/EXECUTION_CHECKLIST.md`。最终至少完成：

- root workspace fmt/test/clippy。
- `aec3` workspace fmt/test/clippy。
- Tauri workspace fmt/test/clippy。
- frontend TypeScript/test/build。
- Tauri debug no-bundle build。
- `check` 深度 diff 审查无未解决 hard stop。
- `git status --short` 只剩允许的审计账本更新，提交后工作树干净。
- 推送 `dev` 后远端 HEAD 与本地一致。
- 创建并推送 `v1.1.0-rc.4`，确认 tag 指向最终验证 commit。
- 等待 GitHub Actions 结束；所有 required/release jobs 通过。
- 确认 GitHub Release 存在、正文非空，且预期 Windows、macOS、Linux 与 CLI 资产已上传。

以上证据齐全才允许把目标标记完成。
