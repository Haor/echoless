# Phase 2 工作需求清单(2026-07-04)

按优先级排列。设计真理来源:`AEC/Design/overview.html`(v17,文件头注释=同步决策清单);
调研真理来源:`docs/architecture/` 两份方案 + `AEC/research/windows_aec_research.md`。

> **2026-07-04 更新**:P3/P4/P5 的 Codex 任务规格已写成自包含文档(现状锚点已逐项核实校准),
> 见 `docs/codex-tasks/`(README 含派发方式与合并纪律)。派发时以规格文档为准,本文件只管优先级。

## 执行规划(worktree + 分工)

前端(Claude)与后端(Codex)完全解耦,用 git worktree 并行:

| 工作树 | 分支 | 负责 | 内容 |
|---|---|---|---|
| `echoless/`(主) | `phase-2/ui-refactor` | Claude | P1 UI 重构 + P2 意见甄别 + P5 托盘前端侧 + P6 README |
| `../echoless-aec3/` | `phase-2/aec3-internalize` ← main | Codex(gpt-5.5) | P3 内化改名(规格=INTERNALIZATION_PLAN,bulk/mechanical) |
| 同上(P3 合入 main 后重建) | `phase-2/aec3-delay-mod` ← main | Codex(gpt-5.5 xhigh) | P4 惯性 + N1 负方向(规格=DELAY_MOD_PLAN) |
| `../echoless-tray/`(可选) | `phase-2/win-tray` ← main | Codex(gpt-5.5) | P5 托盘 Rust 侧(规格明确的小任务) |

规则:

- 后端分支一律从 `main` 切出、PR 合回 `main`;UI 分支定期 merge main 跟进。
  **P4 必须等 P3 合入后再开**(N1 偏置改的 wrapper 文件会被 P3 改名,惯性改 vendor 也在 P3 的移动范围内,
  并行必然冲突;原计划「阶段 1 可与 P3 并行」作废)。
- Codex 调用走 `/codex:rescue`(companion),`--cd` 指向对应 worktree,workspace-write;
  把方案 md 全文作为任务规格喂入,要求按步骤输出 diff 摘要 + 跑 `cargo test -p` 相关包。
- P3/P4 验收由 Claude 复核:P3 全仓 grep 无 `aec3` 残留 + 兼容别名测试;
  P4 回归 = 非平稳激励 + >60s 长跑(顺带验证 internal map §11.6 退化疑云)。

## P1 — UI 重构(主工作树)

按 `Design/overview.html`(v17 定稿)重构整个前端:

- `app/src/styles.css` 全量替换为设计稿 CSS(harness 段忽略),包括:暖碳黑色板、
  橙 #ff7235 唯一强调色、Martian Mono + Archivo(wdth 轴)字体、坐标纸网格+动态噪点、
  铭牌 plate 分格布局、四角 fiducial 方块、半调点阵字标(随电源亮灭+crton/crtoff 动画)、
  电源开关斜纹动画、scramble 乱码切换、srail/zmeta 实值状态字。
- 对应 tsx 组件改造:App 骨架(plate grid)、Controls、Scope、SlideSwitch、
  EnginePage/AdvancedPage/MicSetupPage/RtxSetupPage/DiagnosticsPage 按 7 视图稿逐页对齐。
- 同步决策清单以 overview.html 文件头注释为准(v3→v17 累计,含 B2/B4–B7/C2/C4/C6/A3 等顺手修复项)。

## P2 — UI 修改意见稿重新甄别 ✅ 已完成(2026-07-04)

Triage 表已附在 `docs/audit/UI_ISSUES_VERIFICATION_20260703.md` 末尾。结论:
- **已被设计稿吸收 8 条**(B2/B4/B5/B6/B7/C2 + A3/C6 的样式文案部分):P1 照稿实现即消。
- **P1 逻辑随迁 9+ 条**(A2 自环排除、A4 防抖、B1 尺寸、B3 背景色、B8 RECHECK 传参、
  C1 滚轮归一、C3 减参、C5 去重复、C4 Hint 补全、A6 文案):**换皮不自动解决,P1 checklist 必带**。
- **后端/产品项 10 条** → 新增 P8。
- C6 的「0ms 歧义」会随 P4 的 probe 公式改动自然消失,P1 文案按 P4 新语义写。

## P3 — Vendor 重构:aec3 内化改名(Codex)

按 `docs/architecture/AEC3_INTERNALIZATION_PLAN.md` 执行(8 步):

- aec3→aec3-apm/aec3-core 等改名映射;kind `"aec3"`→`"aec3"` 带兼容别名。
- vendor 有独立 `.git` 需先删除并入。
- **先内化改名、后延迟魔改**(保持 diff 分离)。

## P4 — AEC3 延迟魔改:惯性 + 负方向搜索(Codex,依赖 P3)

按 `docs/architecture/AEC3_DELAY_MOD_PLAN.md` 执行:

- 惯性 = core 三处小改:underrun 不扣 delay + 不软重置;estimate_delay 加 render 静音门
  (阈值沿用 active_render_limit=100.0,i16 量程);gate 期不增 consistent counter。
- 负方向 = 方案 N1:near_delay 升级为常开搜索偏置(Win 默认 0→20ms,
  probe 推荐公式改 `max(bias, -lag+safety)`),vendor 零改动、热更新。
- 回归:非平稳激励 + >60s 长跑。

## P5 — Windows 最小化进系统托盘

Tauri 2(已在用,启用 `tray-icon` feature):

- Rust 侧(`app/src-tauri/src/lib.rs`):创建托盘图标 + 菜单(显示/隐藏、退出);
  拦截窗口最小化/关闭事件 → `window.hide()` 进托盘;左键单击托盘恢复显示。
  Windows 专属行为用 `#[cfg(target_os = "windows")]` 门控,macOS 保持现状(Dock 常规行为)。
- 前端侧:设置页加「最小化到托盘 / 关闭到托盘」开关(持久化到配置),默认最小化=托盘。
- 注意:AEC 引擎运行中隐藏窗口时音频链路必须不中断;托盘 tooltip 显示运行状态(RUNNING/STOPPED)。

## P6 — 重写 README

- 面向用户/测试者重写根 README(现状偏开发笔记):是什么、支持平台(Win10/11 + macOS 14.4+)、
  安装、快速上手(选设备→开 AEC→虚拟麦克风给 Discord/VRChat)、故障排查入口。
- 与 `docs/windows-testing/WINDOWS_FRIEND_INSTALL_TEST_GUIDE.md` 互链,避免内容重复。
- 时机:P1/P3 落地后写(名称与截图都会变)。

## P7 — 收尾与核查(候选,补充项)

- ~~`output_gain_db` 前后端贯通~~:已在 Windows 实测正常(用户构建版验证,2026-07-04),
  仅需在新 UI footer 滚轮上做回归确认。
- probe-delay(延迟侦测)在新 UI 的 Advanced 内嵌位置回归验收(12 点定时进度、自动填 near_delay、自动停机恢复)。
- 文档整理:`docs/audit/` 下多份一次性 PLAN/PROGRESS 已完成的归档或删除。
- CI:`.github/workflows/build.yml` 在 UI 重构后确认 DMG/NSIS 产物与 smoke 脚本仍过。
- i18n:新 UI 文案军规化后同步 `app/src/i18n.tsx` 中英词条。

## P8 — 审计遗留功能项(2026-07-04 从 29 条 triage 补录,此前未入 backlog)

后端/产品向,与 UI 换皮正交。按价值排序:

1. **D1 OFF = passthrough 穿透模式(高优先)— 决策已定(2026-07-04 用户拍板)**:
   **不搞三态,OFF 即穿透**——关了用户麦克风必须还能用,「完全停机」不作为用户级操作(退出应用=停机)。
   后端规格已写:`docs/codex-tasks/TASK_P8_OFF_PASSTHROUGH.md`(chain 级 bypass 热命令 + keep-warm
   保收敛 + crossfade,不换处理器;等 P3 合入后开工)。
   **P1 联动**:电源 OFF 的 UI 语义从「整机停转」改为「AEC 旁路」——sysoff 调暗保留,
   srail 停机文案 MONITOR HELD → 直通语义(如 AEC BYPASS),前端 OFF 不再调 stop_run 而是 set_bypass。
2. **D2 一键 mute**:记忆音量 toggle 0↔恢复,复用 `set_output_level` 实时通道,小活;
   UI 挂点等 P1 footer 定稿。
3. **D6+D7+D8 localvqe 转 HF 下载 + 数据目录统一**(决策已定 2026-07-03):模型+native runtime
   全走 HuggingFace,删随包资源;目录统一 `%LOCALAPPDATA%\Echoless\` 根。三条一起做,
   实施要点见审计文档 D8 条目。
4. **D4 Windows 虚拟麦向导闭环**:显式状态机(检测→引导下载→装后未生效提示重启→完成),
   名字匹配加别名容错。
5. **D5 LocalVQE stats 接线**(小活):`localvqe.rs::stats()` 返回真实 errors/diverged。
6. **A1 权限横幅根治**:helper 加轻量 TCC 预检,返回真实 granted/denied/undetermined。
7. **A5 Process Tap 采样率解锁**:helper 上报实际采样率,Rust 侧 tap 流插重采样器。

## 建议节拍

1. **立即并行**:P1(Claude,主工作树)+ P3(Codex,`echoless-aec3` worktree)。
2. P3 合回 main → 重建 worktree 开 P4(Codex);同期 Claude 做 P2 甄别。
3. P5 托盘:P1 主体完成后插入(前端开关依赖新设置页结构;Rust 侧可提前由 Codex 做)。
4. P1+P3 都合入后:P6 README;最后 P7 收尾清单逐项勾。
