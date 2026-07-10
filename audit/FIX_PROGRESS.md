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
| 1 | B-26 | 第一 | done | `c61195e` | focused 10 passed；CLI 81 passed；clippy clean | ELTP v1 ready + EOF/read-error single fatal report；真实系统音频权限/硬件未自动触发 |
| 2 | B-28 | 第一 | done | `5dab0f1` | Tauri 23 passed；frontend 28 passed + tsc；clippy clean | active generation 将 RunState/tray/status/exit 副作用收口在同一锁内；stderr log 事件仍无 run_id，不影响运行状态所有权 |
| 3 | B-27 | 第一 | done | `0de5877` | processors 44 passed / 3 ignored；clippy clean | native error 立即 reset 并清空 near/far/out；recovery warm-up 期 near passthrough；故障注入覆盖连续/瞬态/重复恢复 |
| 4 | B-25 | 第一 | done | `9ad04cb` | CLI 88 passed；clippy clean | CPAL/Process Tap 共用完整 frame push；adaptive/direct consumer 只弹整帧；stale skip 按声道对齐；drop 计数单位留给 B-29 收口 |
| 5 | B-29 | 第一 | done | `360eb06` | CLI 90 passed；clippy clean | reference/output loss 统一 frame 单位；真实 underrun 帧数；有符号 ±22.4% 双向检测；live/summary 共用 snapshot 与 direction；告警阈值不变 |
| 6 | T-11 | 第一 | implemented-pending-remote-ci | `b3f8e90` | Tauri 23 passed；clippy clean；workflow YAML parsed | Windows + macOS matrix 新增独立 backend test step；保留 clippy/build/smoke；远程日志待推送后验收 |
| 7 | A-09 | 第二 | done | `9d20ca7` | frontend 31 passed + tsc + build | 补齐 17 个 runtime discriminator 的 union；覆盖 stream/skew/serde error、status correlation/direction 与 null control command；复用 run_id |
| 8 | S-13 | 第二 | done | `c1b33c8` | frontend 35 passed + tsc + build；Chromium 3 payload regression | anime.js 在普通对象上生成 scramble 帧，DOM 仅接收 textContent；无新增 element、导航或外部请求 |
| 9 | B-30 | 第二 | done | `8c4faac` | Tauri 26 passed；clippy clean | remove success 才释放 context；失败保留 state/OSStatus 并阻止重复注册；poisoned-state fallback 保持 context 存活 |
| 10 | B-31 | 第二 | done | `30859df` | frontend 48 passed + tsc；CLI config validation ok | TOML basic string 对 C0/DEL 使用标准短转义或 Unicode 转义；quote/backslash/Unicode round-trip 覆盖 |
| 11 | B-32 | 第二 | done | `f6d9aac` | Tauri 28 passed；clippy clean | 纳秒 stamp + PID + attempt + create_new；8-worker 同 stamp 冲突与独立 cap 覆盖；8 MiB/7 天/20 文件策略不变 |
| 12 | S-14 | 第二 | done | `de2d1a4` | Tauri 28 passed；clippy clean | tauri::Url 规范化 scheme/host/port；拒 credentials/non-443；覆盖 backslash/userinfo/encoded delimiter/case/trailing dot |
| 13 | T-12 | 第二 | implemented-pending-remote-ci | `a709ab3` | workflow YAML parsed | pull_request 仅覆盖 main/dev；现有 jobs 与 Linux 发布行为不变；测试 PR 待最终推送阶段验收 |
| 14 | D-12 | 第二 | done-pending-commit | this commit | root 137 passed / 3 ignored；fmt clean | 基线既有 diff 仅纠正 rubato FFT SRC 状态与 GUI native/模型下载/CLI archive 资产分发事实 |
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
