# Echoless 审计修复进度

> 执行分支：`dev`
> 固定基线：`1aa747708d4d8eac8d732c7ed8c827e22d744508`
> 远端基线：`origin/dev@1aa747708d4d8eac8d732c7ed8c827e22d744508`
> 目标发布：`v1.1.0-rc.4`
> 开始时间：`2026-07-10`（Asia/Shanghai）

## 基线记录

- `HEAD == origin/dev`，ahead/behind 为 `0/0`。
- `.git` 与 common gitdir 均可写。
- 既有未提交 D-12 改动：`Cargo.toml`、`configs/example.toml`。
- `audit/` 在执行开始时为未跟踪目录。
- 最新本地/远端 RC 为 `v1.1.0-rc.3`；RC4 尚不存在。
- 平台裁决：公开发布；接受 unsigned 且不改现有文案；不承担 Linux 代码维护或发布质量保证，D-13 仅作事实性文档纠错。

## 条目账本

| 顺序 | 条目 | 梯队 | 状态 | Commit | 专项验证 | 备注 |
|---:|---|---|---|---|---|---|
| 1 | B-26 | 第一 | pending | — | — | Process Tap 握手/异常 EOF |
| 2 | B-28 | 第一 | pending | — | — | sidecar run generation |
| 3 | B-27 | 第一 | pending | — | — | LocalVQE error recovery |
| 4 | B-25 | 第一 | pending | — | — | stereo frame atomicity |
| 5 | B-29 | 第一 | pending | — | — | clock-skew frame units/directions |
| 6 | T-11 | 第一 | pending | — | — | supported-platform Tauri tests in CI |
| 7 | A-09 | 第二 | pending | — | — | complete frontend event types |
| 8 | S-13 | 第二 | pending | — | — | text-only scramble animation |
| 9 | B-30 | 第二 | pending | — | — | CoreAudio listener lifetime |
| 10 | B-31 | 第二 | pending | — | — | TOML control escaping |
| 11 | B-32 | 第二 | pending | — | — | unique log files |
| 12 | S-14 | 第二 | pending | — | — | normalized URL allowlist |
| 13 | T-12 | 第二 | pending | — | — | pull request CI trigger |
| 14 | D-12 | 第二 | pending-existing-diff | — | — | preserve baseline diff; commit separately |
| 15 | D-13 | 第二 | pending | — | — | docs-only Linux path casing |
| 16 | D-14 | 第二 | pending | — | — | sidecar resolution docs |

## 批次门

| 阶段 | 状态 | 证据 |
|---|---|---|
| 执行前基线 | passed | root 118 passed/3 ignored；aec3 722 passed；Tauri 21 passed；frontend 25 passed + `tsc --noEmit` |
| 第一梯队质量门 | pending | — |
| 第二梯队质量门 | pending | — |
| 最终 `check` 审查 | pending | — |
| 推送 `dev` | pending | — |
| 推送 `v1.1.0-rc.4` | pending | — |
| 远端 CI/Release/资产 | pending | — |

## 阻塞与偏差

- 无。
