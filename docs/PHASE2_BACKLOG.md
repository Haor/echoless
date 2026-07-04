# Phase 2 工作需求清单(分支 `phase-2/ui-refactor`,2026-07-04)

按优先级排列。设计真理来源:`AEC/Design/overview.html`(v17,文件头注释=同步决策清单);
调研真理来源:`docs/architecture/` 两份方案 + `AEC/research/windows_aec_research.md`。

## P1 — UI 重构(本分支主题)

按 `Design/overview.html`(v17 定稿)重构整个前端:

- `app/src/styles.css` 全量替换为设计稿 CSS(harness 段忽略),包括:暖碳黑色板、
  橙 #ff7235 唯一强调色、Martian Mono + Archivo(wdth 轴)字体、坐标纸网格+动态噪点、
  铭牌 plate 分格布局、四角 fiducial 方块、半调点阵字标(随电源亮灭+crton/crtoff 动画)、
  电源开关斜纹动画、scramble 乱码切换、srail/zmeta 实值状态字。
- 对应 tsx 组件改造:App 骨架(plate grid)、Controls、Scope、SlideSwitch、
  EnginePage/AdvancedPage/MicSetupPage/RtxSetupPage/DiagnosticsPage 按 7 视图稿逐页对齐。
- 同步决策清单以 overview.html 文件头注释为准(v3→v17 累计,含 B2/B4–B7/C2/C4/C6/A3 等顺手修复项)。

## P2 — UI 修改意见稿重新甄别

`docs/audit/UI_ISSUES_VERIFICATION_20260703.md`(29 条,基于旧 UI 截图核实):

- 逐条判定:已被新设计稿吸收 / 仍然生效需要修 / 已失效(旧 UI 特有)。
- 输出一份 triage 表附在该文档末尾,仍生效项并入 P1 执行。

## P3 — Vendor 重构:sonora 内化改名

按 `docs/architecture/AEC3_INTERNALIZATION_PLAN.md` 执行(8 步):

- sonora→aec3-apm/aec3-core 等改名映射;kind `"sonora_aec3"`→`"aec3"` 带兼容别名。
- vendor 有独立 `.git` 需先删除并入。
- **先内化改名、后延迟魔改**(保持 diff 分离)。

## P4 — AEC3 延迟魔改:惯性 + 负方向搜索

按 `docs/architecture/AEC3_DELAY_MOD_PLAN.md` 执行:

- 惯性 = core 三处小改:underrun 不扣 delay + 不软重置;estimate_delay 加 render 静音门
  (阈值沿用 active_render_limit=100.0,i16 量程);gate 期不增 consistent counter。
- 负方向 = 方案 N1:near_delay 升级为常开搜索偏置(Win 默认 0→20ms,
  probe 推荐公式改 `max(bias, -lag+safety)`),vendor 零改动、热更新。阶段 1 可与 P3 并行。
- 回归:非平稳激励 + >60s 长跑(顺带验证 internal map §11.6 退化疑云)。

## P5 — 重写 README

- 面向用户/测试者重写根 README(现状偏开发笔记):是什么、支持平台(Win10/11 + macOS 14.4+)、
  安装、快速上手(选设备→开 AEC→虚拟麦克风给 Discord/VRChat)、故障排查入口。
- 与 `docs/windows-testing/WINDOWS_FRIEND_INSTALL_TEST_GUIDE.md` 互链,避免内容重复。

## P6 — 收尾与核查(候选,补充项)

- `output_gain_db`:后端已见于 realtime/control/stats/diagnostics,确认前后端贯通并在新 UI 的
  footer 音量滚轮上验收。
- probe-delay(延迟侦测)在新 UI 的 Advanced 内嵌位置回归验收(12 点定时进度、自动填 near_delay、自动停机恢复)。
- 文档整理:`docs/audit/` 下多份一次性 PLAN/PROGRESS 已完成的归档或删除。
- CI:`.github/workflows/build.yml` 在 UI 重构后确认 DMG/NSIS 产物与 smoke 脚本仍过。
- i18n:新 UI 文案军规化后同步 `app/src/i18n.tsx` 中英词条。
