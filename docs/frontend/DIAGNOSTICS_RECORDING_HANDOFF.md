# Handoff — 诊断录制(diagnostics recording)后端做干净

**From:** 前端(GUI)
**To:** Codex(后端 / realtime 引擎)
**状态:** 核心契约已实现(2026-06-08);就地 start/stop 控制通道仍未做
**优先级:** 中高 —— 当前前端靠定时器猜「录制结束」,且录制 I/O 压在实时线程上有 glitch 风险。

---

## 1. 背景 / 前端现状

诊断页有「录制」开关 + 「最长秒数 `max_seconds`」。需求:**录到 `max_seconds`
后自动关掉录制开关并打开会话目录**。

由于后端到上限后**不发任何结束信号**(详见 §2),前端目前只能**用定时器猜**:
在 `started` 事件(带 `diagnostics_session_dir`)后按 `max_seconds*1000 + 600ms`
起一个 timer,到点 `setRec(false)` + `openPath(session_dir)`。
(实现见 `app/src/App.tsx` 的 `recTimerRef` / started handler。)

这是**临时妥协**:`+600ms` 缓冲是为了等后端 finalize 文件;时长全靠前端计时不准;
不知道后端到底写完没。后端做干净后,**前端会删掉这套 timer**,改为监听结束事件。

## 2. 旧后端现状(已修核心问题)与「不干净」的点

`crates/echoless-cli/src/realtime.rs`:

- `write_frame()`(:556)在 **10ms 处理线程**里逐帧写 mic/ref/out 三个 wav 样本
  + 一行 stats.csv(:565-611)。
- 到 `max_frames` 时**同线程**直接 `finish()`(:561 / :623);`finish()`(:629)里
  `WavWriter::finalize()` 要 **seek 回去重写 RIFF 头**,是阻塞磁盘 I/O,卡在处理节拍上
  → 可能 output underrun。
- 自停后**不发事件**;`diagnostics_session_dir` 在 status(:376 / :1015)是**静态值**,
  不反映「录制已停」。前端无从得知。
- WAV 头靠结束回写,进程被 `SIGKILL` 就留下**头部错误、不可播放**的文件。
- 开/关录制走前端 `applyChange` → `stopRun + startRun`,**整条音频重启**(一声 blip)。

2026-06-08 后端已修复:

- 诊断 WAV/CSV 写入和 `finalize()` 已移到专用 writer 线程。
- 实时处理线程只做 `try_send` 到有界 channel;channel 满会增加 `diagnostics_drops`,不会阻塞音频线程。
- 录满 `max_seconds` 或 run 退出后,writer 在线程内完成 flush/finalize/rename,再发 `diagnostics_done`。
- 运行中 `status` 已包含 `recording` / `diagnostics_frames` / `diagnostics_elapsed_s` / `diagnostics_drops`。
- WAV/CSV 先写 `*.part`,finalize 后 rename 成 `mic.wav` / `ref.wav` / `out.wav` / `stats.csv`。
- 尚未实现:运行中通过 IPC 就地 `start_diagnostics` / `stop_diagnostics`;当前开启/关闭录制仍随 run 配置重启。

## 3. 干净的设计

### 3.1 录制 I/O 全部移出实时线程(最关键)
处理线程**只把每帧** `(near, ref, out, stats_row)` 推进一个**有界 channel**
(crossbeam / SPSC ring),由**专用 writer 线程**做全部 wav/csv 写入与 finalize。
- channel 满(磁盘太慢)→ **丢帧 + 计数**(新增 `diagnostics_drops`),**绝不阻塞音频**。
- 实时路径从此 lock-free、无磁盘抖动;`finalize()` 的 seek 也只发生在 writer 线程。

实现备注:当前用标准库 `sync_channel` + `try_send`,容量为 128 帧。`diagnostics_drops`
按丢弃的诊断帧计数,不是音频输入 drop。

### 3.2 发显式结束事件(让前端别再猜)
writer 线程在**文件 flush + finalize 完成之后**发一条 JSONL 事件(走与 status 同一
stdout 流,前端经 `echoless://status` / 或新事件名转发):
```json
{
  "type": "diagnostics_done",
  "session_dir": "/…/session-XXXX",
  "frames": 96000,
  "seconds": 10.00,
  "reason": "max_seconds | user_stop | run_exit",
  "drops": 0,
  "ok": true
}
```
前端收到 → 关开关 + 打开目录。**无需 timer、无需缓冲**,文件保证已写完可播。

实现备注:当前 reason 取值为 `max_seconds` / `run_exit` / `error`。`user_stop`
预留给未来就地 `stop_diagnostics` 控制通道。

### 3.3 status 补录制态(给实时进度)
status 增加:
- `recording: bool` —— 录制中为 true,自停后转 false(前端据此显示进度/收尾)。
- `diagnostics_frames: u64` / `diagnostics_elapsed_s: f32` —— 已录时长,供 UI 显示
  「录制中 3.2s / 10s」倒计时。
- (可选)`diagnostics_drops: u64` —— writer 丢帧计数,非 0 提示磁盘跟不上。

### 3.4 崩溃安全的收尾
任选其一(建议组合):
- 写到 `*.wav.part`,finalize 后**原子 rename** → 半成品永不被当成完整文件;
- 或**每 N 帧增量回写 wav 头**,`SIGKILL` 也至少留下可解析数据。
- `stats.csv` 同理 flush + rename。

当前采用第一种:`*.part` finalize 后 rename。`SIGKILL` 时可能留下 `.part`,但不会伪装成完整
`*.wav` / `stats.csv`;未做增量回写 WAV 头。

### 3.5 就地开关录制,不重启 run
后端已在 `run --status-json` 模式启用 stdin JSONL 控制通道:
```json
{"cmd":"start_diagnostics","record_dir":"…","max_seconds":10}
{"cmd":"stop_diagnostics"}
```
引擎就地起/停 writer 线程,**不拆音频管线** → 开关录制不再有 run 重启 blip;`max_seconds`
纯由后端把控。前端改为向 sidecar stdin 写一行 JSON,不要靠 `applyChange` 重启 run。

事件:

- 成功启动后 stdout JSONL 追加:
  `{"type":"diagnostics_started","session_dir":"…","max_seconds":10,"recording":true}`
- 收到 stop 后 stdout JSONL 追加:
  `{"type":"diagnostics_stopping","session_dir":"…"}`
- 文件 finalize/rename 完成后仍由 `diagnostics_done` 通知。手动 stop 的
  `diagnostics_done.reason` 为 `"stopped"`。
- 命令错误为:
  `{"type":"control_error","cmd":"start_diagnostics","message":"…"}`

### 3.6 时长以样本数为准
`max_frames`(:521-523)已是源真相,保留。上报 `seconds = written_frames / sample_rate`
保持诚实。**前端彻底不再计时。**

## 4. 前端实现后会怎么改(配合契约)

✅ 前端已接入(2026-06-08):

- 已删除 `App.tsx` 的 `recTimerRef` / started-handler 定时猜测逻辑。
- 已监听 §3.2 的 `diagnostics_done`:**仅 `reason === "max_seconds"`** 时
  `setRec(false)` + `openPath(ev.session_dir)`;`run_exit` / `error`(手动停或改
  设置触发)**不**弹目录,避免每次重启 run 都打开文件夹。
- 已用 §3.3 的 `recording` / `diagnostics_elapsed_s` / `diagnostics_drops` 在诊断页
  SESSION 行显示实时进度徽章 `● 3.2s / 10s` + writer 丢帧告警。
- `types.ts` 增 `DiagnosticsDoneEvent` + RuntimeStatus 录制字段;`RunEvent` 联合已扩。

✅ 后端 §3.5 已实现:开/关录制可走 stdin JSONL `start_diagnostics` /
`stop_diagnostics`,前端可去掉录制开关时的 `applyChange` 重启。

## 5. 验收

- 录满 `max_seconds`:后端在**文件已 finalize 后**发 `diagnostics_done`;
  期间实时线程无磁盘阻塞、`output_underruns` 不因录制上升。
- 进程被强杀:已录数据仍可解析 / 可播(头不损坏或可恢复)。
- status 全程有 `recording` + 已录时长;自停后 `recording=false`。
- (若做 §3.5)开/关录制不产生音频 blip。
- 前端据此移除定时器,行为更准更稳。

## 6. 关联

- 录制产物字段(stats.csv 列)见 `realtime.rs:537-539`;延迟分项字段同源,
  另见 `docs/frontend/` 内延迟相关说明。
- 与 `MAC_REFERENCE_PROCESS_TAP_HANDOFF.md` 独立,但同属 realtime I/O 线程模型的整顿,
  可一并规划线程/channel 抽象。
