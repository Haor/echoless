# Handoff — macOS far-end reference 走 Core Audio Process Tap(替代 BlackHole loopback)

**From:** 前端(GUI / 调研侧)
**To:** Codex(后端 / realtime 引擎)
**状态:** 后端最小集成已完成;`reference="system"` 在 macOS 走 Process Tap helper;Tauri app 已补 macOS 权限 plist 与 helper env 注入
**优先级:** 高 —— 这是 mac 上 AEC 实际可用性的卡点(当前 BlackHole 参考断断续续)。

---

## 1. 现象(用户实测)

mac 上用 **BlackHole 2ch 作 far-end reference** 时,录到的参考信号 **断断续续**,
导致 AEC 没有稳定参考、消回声效果差。用户的直觉提问:
> 「mac 的 ref,为什么不是软件请求录音权限?」

—— 即用户期望 app 弹一次系统音频录制权限、直接抓系统播放声音作参考,
而不是靠 BlackHole 这种虚拟声卡绕路。

## 2. 现状(后端代码核实)

实时引擎是**纯 cpal**,mac 上没有系统 loopback 能力:

- `crates/echoless-cli/src/realtime.rs:118-127` —— `reference="system"` 在 mac 上是去开
  **默认输出设备做 loopback**(`select_default_device(Output)`),但 cpal 在 mac 上不支持
  输出设备 loopback。
- `crates/echoless-cli/src/realtime.rs:1467` doctor 文案已自认:
  `macOS has no generic CPAL system loopback here; use BlackHole/VB-CABLE MAC ...`
- 因此 mac 上能用的参考只剩「把 reference 指到 BlackHole 这个**输入设备**」
  (`select_render_device(sel)`),当普通麦克风流读。
- 三股独立 cpal 流(mic / reference / output)各自时钟域;drift 处理很糙:
  `realtime.rs:386` 仅「积压超 4 帧丢旧的」+ 计 `ref_underrun`,**无异步重采样补偿**。

**这就是断续的根因:**
1. **时钟漂移**:BlackHole 与 mic 是独立时钟域,处理线程按固定速率消费 far,
   BlackHole 时钟稍偏 → ref ring under/overrun → 间歇丢帧。
2. **路由依赖聚合设备**:要让系统声进 BlackHole 同时用户还能外放听见,需建
   Multi-Output / Aggregate Device;mac 聚合设备成员间漂移,非时钟主设备(BlackHole)
   丢样本。即便开 Drift Correction 也不根治。
3. 只路由部分 app / 当下无播放时,本就是真空隙。

> 注意:架构本来就定了走 Process Tap —— `crates/echoless-core/src/lib.rs:76` 注释写着
> `far-end 参考源:… mac="system"(Process Tap)`。**只是还没实现**,当前落到 BlackHole 退路。

## 3. 期望方案:Core Audio Process Tap(macOS 14.4+)

用 **Core Audio Process Tap** 直接抓系统 / 进程 render 流作 far-end reference:

- API:`AudioHardwareCreateProcessTap` + `CATapDescription`(可整机 tap,也可按进程 tap),
  配 aggregate device 读出 tap 的 PCM。(ScreenCaptureKit 的 `SCStream` 音频是备选,
  但 Process Tap 更轻、延迟更可控、不拉起屏幕录制 UI。)
- **优点对应消除上面三个断续根因**:render 流与设备解耦,不需要 BlackHole 路由,
  不需要 Aggregate 多输出;tap 与输出可走同一时钟域 / 拿到时间戳便于对齐。
- **权限**:Process Tap / 系统音频采集需要 TCC 授权(系统音频录制)。需在
  `Info.plist` 配相应 usage description / entitlement,并在首次使用时触发系统授权弹窗。
  这正是用户期望的「软件请求录音权限」。

### 3.1 PoC / 集成进展(2026-06-08)

已新增独立 PoC / dev helper:

- `tools/macos-process-tap-poc/`
- 研究记录:`docs/research/mac_process_tap_findings.md`

本机 macOS 26.5.1 上验证通过:

- Process Tap 成功创建;
- 捕获格式为 `48000 Hz, 2 ch, 32-bit float, interleaved`;
- 播放 `/System/Library/Sounds/Glass.aiff` 时录到非零 ref:
  `frames=196608 callbacks=384 peak=0.19832 rms=0.01682`;
- 输出文件:`/private/tmp/echoless-process-tap-ref-with-afplay.wav`。

新增主链路集成:

- Rust realtime 在 macOS `reference="system"` / `"default"` 时不再尝试 CPAL output
  loopback,而是启动 Process Tap helper;
- helper 用 `--stream-stdout` 输出 raw little-endian Float32 PCM,stderr 保留日志;
- Rust 会传 `--exclude-pid <echoless pid>`,helper 将 PID 翻译为 Core Audio process
  object 后传给 `CATapDescription(...GlobalTapButExcludeProcesses:)`,避免把 Echoless
  自己送往虚拟麦的 clean output 混进 reference;
- `run --status-json` 的 started event 增加
  `reference_source:"macos_process_tap"`;
- `devices --json` 的 macOS `reference_sources` 暴露
  `System Audio (Process Tap)`,并不再把普通 output 设备列为 reference 候选;
- 短链路 smoke test 已通过:
  `ref_dbfs=-36.09`,`ref_underruns=0`,`runtime_errors=0`。

注意:Codex 沙箱内运行同一二进制会失败,沙箱外运行成功。产品集成时必须让
Tauri app 或固定 helper binary 成为系统音频录制权限主体。

### 请后端诊断 / 确认的点
1. 目标机 macOS 版本是否 ≥ 14.4(Process Tap 可用下限);<14.4 的回退策略(仍 BlackHole?)。
2. 第一版仅支持 48 kHz,因为本机 tap 输出为 `48000 Hz,2ch,float32`;
   若未来允许 16 kHz realtime,需要 helper 或 Rust 边界补 SRC。
3. Process Tap 抓到的流采样率 / 声道 / 时钟,与 mic、output 的对齐与重采样方案
   (顺带把现有 BlackHole 路径的 drift 也用同一套异步重采样修了)。
4. 整机 tap vs 按进程 tap 的取舍(当前仍是整机 global tap,但已排除 Echoless CLI
   进程自身输出;更细的按目标应用/目标设备 tap 仍是未来优化)。
5. 自身输出回授:已做 CLI PID 排除。若 Core Audio 无法把 PID 翻译成 process object,
   helper 会在 stderr 警告并继续运行,需要真机日志确认。

## 4. 前端需要的契约(实现后请回传)

前端会据此把 mac 的 reference 体验从「退 none + 让用户折腾 BlackHole」升级为
「一键系统音频 + 权限引导」。需要后端在 JSON 契约里补:

### `doctor audio --json`
- 新增**系统音频录制权限态**字段(类似现有 mic 的 `permission_state`),
  例如 `system_audio_permission: "granted" | "denied" | "undetermined" | "unknown"`。
  - 已补字段:regular `doctor audio --json` 不主动触发 Process Tap 授权弹窗。
  - 当前 macOS 上 helper 可发现时返回 `undetermined`;helper 缺失或非 macOS 返回 `unknown`。
  - 已补显式请求:`echoless doctor audio --request-system-audio --json` 会启动一次极短
    Process Tap probe,只应由前端在用户点击「请求权限」后触发。
  - 请求后 JSON 里 `system_audio_permission` 会按 probe 结果更新为
    `granted` / `denied` / `unknown`,并附带 `system_audio_permission_probe`:
    `{ requested, ok, state, detail }`。
- `reference_sources` 里在 mac 14.4+ 暴露一个可用的 `system` 源(Process Tap),
  `available: true`,`label` 形如 `System Audio (Process Tap)`;
  <14.4 时该源 `available: false` 并给 `hint` 指向 BlackHole 回退。
- macOS 不再暴露普通 output 设备作为 reference 候选;BlackHole 等 fallback 通过
  input 设备候选继续可选。
- **⚠️ 请勿把物理麦克风列进 `reference_sources`**(用户反馈):reference 概念 = 系统正在播放的
  声音(输出内容),物理麦(MacBook 麦 / USB 麦 / Virtual Desktop Mic 等)当参考无意义、且误导。
  `reference_sources` 应只含:`system`(Process Tap / loopback)、`none`、Windows 的 output
  回环、以及**承载系统声的虚拟声卡输入**(BlackHole / VB-CABLE)。当前后端把所有 input 设备
  都塞进来了 → 前端临时用名字正则(blackhole/vb-cable/cable/loopback/stereo mix/soundflower)
  过滤掉物理麦,但**正解是后端按设备类型筛选**(后端更清楚哪个是虚拟声卡)。
- (可选)`reference_health` / 在 run status 里标记 ref 是否在丢帧,便于前端显示「参考不稳」。

### `run`(reference 选择)
- `reference="system"` 在 mac 走 Process Tap(不再尝试输出设备 loopback)。
- helper 发现顺序:`ECHOLESS_PROCESS_TAP_HELPER` → 与 `echoless` 同目录的 helper →
  repo dev path `tools/macos-process-tap-poc/.build/echoless-process-tap-poc`。
- 首次需要权限时:要么 CLI 触发系统弹窗,要么返回明确的「权限未授予」错误码,
  让前端引导用户去「系统设置 › 隐私与安全性 › 系统音频录制」。

### 权限请求入口(前端可调)
- 前端权限按钮调用 `echoless doctor audio --request-system-audio --json`;
  普通刷新仍调用 `echoless doctor audio --json`,不要隐式弹系统授权窗。
- 当前触发 Process Tap 的权限主体是实际运行的 helper/sidecar 二进制。Tauri 打包后若 helper
  路径、bundle id、签名主体变化,macOS TCC 可能把它当作新的授权主体,需要重新请求一次。

### ⚠️ 实测发现(2026-06-08,dev / `tauri dev`)—— 系统音频录制权限根本没被请求
用户在 `tauri dev` 下开 ON:**只弹了「麦克风」权限,没有弹「系统音频录制」权限**;
结果 Process Tap 静默无信号 → 一直「无参考信号」。诊断:

- **麦克风权限正常**:mic 采集由 `echoless` CLI(sidecar)发起,macOS 照常弹麦克风授权。
- **系统音频录制权限没触发**:Process Tap 在**独立 helper 二进制**里创建;该 helper 是
  无 bundle / 无 `Info.plist` / 未签名的裸可执行文件,**没有 TCC 主体身份和 usage description**,
  所以 macOS 既不弹「系统音频录制」授权窗,也不会在隐私面板出现可勾选项 → 静默拒绝。
- Codex 之前 sandbox 外 smoke test 能拿到 ref 信号,是因为**那台终端早已被授予过**系统音频录制权限,
  helper 继承了「责任进程」的授权;用户机器从未授予 → 无信号。
- 现状核实:`app/src-tauri/tauri.conf.json` **完全没有** `bundle.macOS` plist /
  `NSMicrophoneUsageDescription` / `NSAudioCaptureUsageDescription` / entitlements,
  也没有任何 `Info.plist`。

**Codex 已补(2026-06-08):**
1. `app/src-tauri/Info.plist` 已加 `NSMicrophoneUsageDescription` +
   **`NSAudioCaptureUsageDescription`**(系统音频录制用途文案)。Tauri v2 会将该文件合并进
   macOS app bundle。
2. `app/src-tauri/src/lib.rs` 已统一通过 `echoless_command()` 启动 CLI,并在能定位 helper 时
   注入 `ECHOLESS_PROCESS_TAP_HELPER`。这覆盖 `doctor audio --request-system-audio --json`
   和 `run --status-json`,保证权限 probe 与 realtime 使用同一个 helper 路径。

**仍需打包验证 / 后续处理:**
1. helper 随 app 打包、**代码签名**(最好与 app 同签名 / 嵌入 app bundle),让它成为合法 TCC 主体;
   或干脆把 Process Tap 移进主 app 进程,避免裸 helper 的授权黑洞。
2. 确保 TCC「责任进程」= Echoless.app,使麦克风 + 系统音频录制两个授权都归属到 app。
3. dev(`cargo run` 未打包)下系统音频录制权限可能仍无法完整触发 —— 这是预期;
   需要 **签名后的 `.app`** 才能完整验证。dev 临时验证只能靠那台机器先前已授权。

> 注:前端 TS 侧无法修这个 —— 权限触发在 CLI/helper + bundle plist/签名,属后端/打包范围。

## 5. 验收

- mac 14.4+:`reference="system"` 经 Process Tap 拿到**连续、无断续**的参考流,
  AEC 收敛、`ref_underruns` 不再持续增长。
- 首次使用弹出系统音频录制授权;拒绝后有可恢复的引导路径。
- doctor 暴露上述字段;前端据此显示 `System Audio (Process Tap)` 而非退 none。
- BlackHole 路径保留为 <14.4 回退,且 drift 用上同一套重采样后断续明显改善。

## 6. 相关参考(调研工作区)

- `research/windows_aec_research.md` —— 延迟估计 / 时钟漂移 / 对齐章节(同样适用 mac tap 流对齐)。
- `reference_repos/`(obs-studio WASAPI 采集、pipewire/pulseaudio 架构)可作 I/O 抽象参考。
- 苹果文档:Core Audio Taps(`AudioHardwareCreateProcessTap` / `CATapDescription`,macOS 14.4+)。
