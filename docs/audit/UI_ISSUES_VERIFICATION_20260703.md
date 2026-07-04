# UI 问题核实报告(2026-07-03)

来源:用户 Telegram 截图反馈(macOS + Windows 双平台实测),共 29 条。
每条已对照代码逐一验证,给出结论、证据(file:line)与修复方向。
路径基准:仓库根 `echoless/`。

> **2026-07-03 决策更新(用户确认)**:
> - B9 圆形箭头浮层 = 截图伪影,**不处理**。
> - D6/D8 方向变更:**localvqe 不再随包分发,模型(含 v1.3)与 native runtime 全部走 HuggingFace 下载**。详见 D6/D8 条目内的更新说明。

**结论图例**:✅ 存在(确认是 bug/缺陷) · 🟡 部分存在(有细节出入或属设计缺口) · ❌ 不存在(非本应用问题) · 🆕 功能缺失(用户新需求,确认未实现) · 🎨 设计决策(值可调,需产品确认)

---

## 汇总表

| # | 问题 | 结论 | 优先级建议 |
|---|---|---|---|
| A1 | 权限横幅每次开屏都显示 | 🟡 存在(根因:从不查真实 TCC) | 高 |
| A2 | Reference 列出自家 BlackHole 输出(自环) | ✅ | 高 |
| A3 | Windows Reference 列出全部设备 | ✅ | 中 |
| A4 | 切换 reference 后状态标签抽搐,无防抖 | ✅ | 中 |
| A5 | Process Tap「仅支持 48k」是否属实 | 🟡 实现硬编码,非技术限制 | 中 |
| A6 | 16↔48 自动重采样标识缺失 | 🟡 后端有重采样;前端标识从未存在 | 低 |
| B1 | 窗口可拉低于默认高度导致溢出 | ✅ | 高(一行改) |
| B2 | localvqe 高级页参数溢出 | ✅ | 高 |
| B3 | 拖底部 resize 出现白边 | 🟡 成因吻合,缺原生背景色兜底 | 中 |
| B4 | model 路径输入框截断样式 | ✅ | 中 |
| B5 | NVAFX「pair with…」黄色 + 「Win · only」 | ✅ | 低(文案) |
| B6 | hero 状态行颜色层级太高 | 🎨 | 低 |
| B7 | RUN PROBE 方块位置 | 🟡 | 低 |
| B8 | NVAFX runtime 检查区布局 | ✅ | 中 |
| B9 | 高级页圆形左右箭头浮层 | ❌ 截图伪影(用户已确认),不处理 | — |
| C1 | 滚轮方向(Win 上滚增 / mac 双指上滚增) | 🟡 mac 自然滚动下方向相反 | 高 |
| C2 | noise_gate_threshold_dbfs = -45 是否正确 | 🟡 值正确,但 gate OFF 时不该露 | 低 |
| C3 | localvqe 参数太多 | ✅ 建议隐藏 library/backend/device | 中 |
| C4 | 帮助弹窗缺「意义+推荐」 | ✅ device/backend 连 Hint 都没有 | 中 |
| C5 | NVAFX 参数评估 + 与引擎页重叠 | ✅ runtime_dir 重复暴露 | 中 |
| C6 | 「无需修正 · 0ms」歧义 + 自动填 init 延迟 | ✅ Windows 从不自动填 initial_delay_ms | 高 |
| D1 | OFF 应为 passthrough(穿透) | 🆕 当前 OFF=整机停转 | 高 |
| D2 | 一键 mute | 🆕 | 中 |
| D3 | 左上角返回 | 🆕 仅左下 `<<<` 和 Esc | 低 |
| D4 | 虚拟麦向导 Windows 不工作 | 🟡 只会开浏览器,权限态恒 unknown | 高 |
| D5 | HEALTH 面板是否正常 | 🟡 4 项正常;LocalVQE 的 errors/diverged 未接线 | 中 |
| D6 | 随包 v1.3 不在「打开模型目录」中 | ✅ → 决策:取消随包,v1.3 改 HF 下载,问题随之消失 | 中 |
| D7 | localvqe 与 nvafx 数据目录不一致 | ✅ `app.echoless.desktop` vs `Echoless` | 中 |
| D8 | localvqe 打包方式 | 决策:**不随包,模型+runtime 全走 HuggingFace 下载** | 高 |

---

## A. 权限 / 设备 / 引用

### A1. 「REQUEST SYSTEM AUDIO PERMISSION »」每次开屏都显示 — 🟡 存在

- 横幅显示条件是 `system_audio_permission === "undetermined"` 且正在用 system reference(`app/src/App.tsx:1063-1066`,渲染在 `RuntimeStatusStrip.tsx:93-100`),并非无条件。
- **根因**:后端从不真正查询 TCC。`crates/echoless-cli/src/realtime/devices.rs:639-649` 的 `system_audio_permission_state()` 只判断 helper 二进制是否存在——存在就永远返回 `"undetermined"`。真实探测路径 `request_system_audio_permission()`(`devices.rs:651-676`)只在用户主动点横幅时才走。
- 结果:mac 上只要选了 system reference,**每次冷启动横幅必现**;运行中拿到真实信号后前端就地改成 granted(`App.tsx:524-538`),但只在内存里,重启复原。

**修复**:helper 增加轻量 TCC 预检(如试建 Process Tap 后立即销毁)并在默认 doctor 中调用,返回真实 granted/denied/undetermined;或持久化上次 granted 结果。注意 `CGPreflightScreenCaptureAccess` 是屏幕录制权限,不适用于 audio-capture TCC。

### A2. Reference 下拉中的 BlackHole 是 Echoless 自己的输出(自环) — ✅ 存在

- 后端把所有 input 设备都加进 `reference_sources`(`devices.rs:382-392`),BlackHole 在 mac 上以 input 身份进入候选。
- 前端过滤反而**主动保留**它:`App.tsx:992-1000` 的 `VIRTUAL_REF` 正则显式含 `blackhole`。
- 而输出默认逻辑恰好优先选 BlackHole 作 near-end 输出(`App.tsx:86-88`)。**没有任何逻辑排除「当前被选为输出的设备」**——选它当 ref 即把自己的处理输出当远端参考,形成自环。

**修复**:`availRefs` 过滤时排除 `stable_id` 等于当前 `selOutput` 的设备;mac 上 system 参考应走 Process Tap,BlackHole input 至多作 fallback 并明确标注风险。

### A3. Windows Reference 下拉列出全部 render/capture 端点 — ✅ 存在

- 后端全量枚举:`devices.rs:382-405`(system + none + 全部 input + 非 mac 时全部 output)。
- 前端过滤保留了**所有 `kind === "output"`**(`App.tsx:998`),所以扬声器 / Realtek / NVIDIA HD Audio / Steam Streaming Speakers 等全部 render 端点都显示。

**修复**:Windows 上收敛为 `system`(render loopback)+ `none` + 少量虚拟回环(CABLE / Stereo Mix);其余物理端点折叠进高级选项。

### A4. 切换 reference 后状态标签抽搐,需几秒迟滞 — ✅ 存在

- `runtimeTelemetry.ts:73-96` 每帧 status 事件直接覆盖快照并 emit,**无任何防抖/迟滞**。
- `RuntimeStatusStrip.tsx:38-48` 的 `noRef`/`unstable` 由当帧 `live.healthy`、`live.ref <= -100` 直接推导;切换 ref 后短时间 ref_dbfs 在 -100 上下跳 → 标签逐帧闪烁。

**修复**:切换 ref 源后设 2-3 秒稳定窗口抑制状态降级;或对 `noRef`/`unstable` 用「连续 N 帧才翻转」的滞后判定。

### A5. 「macOS Process Tap 仅支持 48000 Hz」是否属实 — 🟡 实现限制,非技术限制

- 报错来自 `crates/echoless-cli/src/realtime.rs:162-167`:tap 模式下 `sample_rate != 48000` 直接 bail。48k 是硬编码常量(`macos_process_tap.rs:14`)。
- helper(Swift)实际**跟随设备原生格式**(`main.swift:113-120, 324`),Rust 侧只是「假定 = 48k」且未做重采样——设备若非 48k 甚至会静默错位。
- 管线本身具备任意采样率桥接能力(`chain.rs` 的 `BoundaryAdapter` + rubato),技术上完全可以 tap 后重采样到 16k 而非报错。

**修复**:helper 上报实际采样率,Rust 侧对 tap 流插入到 pipeline 采样率的重采样器,即可支持 16k 管线,同时消除这条报错。

### A6. 「localvqe 是 16k,管线自动 16→48」标识不见了 — 🟡 标识从未在前端实现过

- 后端自动重采样**确实存在**:`localvqe.rs:18`(16k 常量)+ `chain.rs:207,276-303`(`BoundaryAdapter` 自动建 rubato 重采样),有测试覆盖(`chain.rs:531`)。
- 前端只有静态标签 `sr: "16k only"`(`EnginePage.tsx:61`)。git 历史查证(`git log -S "resample"/"16k"`)**没有任何「自动重采样」标识被删除的痕迹**——用户记忆中的标识可能是与 `App.tsx:1155-1158` 的设备级重采样 badge(`.rsmp`,如 `44.1k→48k`)混淆。
- 这也解释了 A5 的错误弹窗:用户把采样率切到 16000 想配合 LocalVQE,实际上不需要(48k 管线下会自动 48→16→48),但 UI 没有传达这一点。

**修复**:LocalVQE 卡片文案改为「16k(管线自动 48↔16 适配)」,或选 LocalVQE 且管线 48k 时在 overview 显示 `48k → engine 16k` badge(可复用 `.rsmp` 样式)。

---

## B. 窗口 / 布局 / 样式

### B1. 窗口可拉得比默认更矮,拉矮后元素溢出 — ✅ 存在

- `app/src-tauri/src/lib.rs:1123-1130`:默认 `inner_size(1040, 640)`,但 `min_inner_size(960, 600)`——高度可再压 40px、宽度可再压 80px,压缩后固定布局溢出。`tauri.conf.json` 的 `windows: []` 为空,尺寸全由 builder 决定。

**修复**(一行):`min_inner_size(1040.0, 640.0)`,默认即最小,只能放大。

### B2. 选 localvqe 后高级页参数溢出 — ✅ 存在

- 高级页容器 `.page` 是「一屏放下、不滚动」的固定布局:`styles.css:210-217`(`overflow: visible`,为了不裁 tooltip)。
- LOCALVQE 分组 7 个参数注入 `.acols`(`AdvancedPage.tsx:349-351, 459-464`)后超出中间行高度(`.window` 是 `40px 1fr 30px` 固定三行 grid,`styles.css:76`),溢出且无滚动条。
- 对比:引擎页 `.page.engine` 有 `overflow-y: auto`(`styles.css:220-224`),高级页没有。

**修复**:高级页容器加 `overflow-y: auto`;tooltip 改 fixed/portal 定位以免被裁。配合 C3 减参后可能一屏内放得下,两者一起做。

### B3. 拖底部 resize 出现白边 — 🟡 成因吻合,需运行时确认

- 前端所有背景均为 `#08090b`(`styles.css:52-79`),无白色;但 `lib.rs` 的 WebviewWindowBuilder **未设置** `background_color`,原生窗口默认白底在 resize 时先于 webview 重绘露出——正是截图现象的典型成因。

**修复**:builder 加 `.background_color(#08090b)`(Tauri v2 支持)。

### B4. model 路径输入框截断(只见 `/Users/haruk`) — ✅ 存在

- 渲染链 `AdvancedPage.tsx:463 → 386-393 → 376-384` 未传 `wide`,input 用 `.afield` 仅 **120px 宽**(`styles.css:316-335`),且无 `text-overflow` / `direction` 规则,长路径只能看到开头,文件名不可见。

**修复**:路径类字段用 `wide`(340px)或更宽变体 + `direction: rtl`(让末段文件名可见)+ title 悬浮全路径。若采纳 C3 的建议(model 只在引擎页管理),此字段可直接移除。

### B5. NVAFX「pair with NVIDIA Broadcast…」黄色改白 + 「Win · only」 — ✅ 存在

- 颜色:`.epair` 用 `color: var(--warn)`(琥珀 `#d8b45a`,`styles.css:875-885`)。
- 文案:`i18n.tsx:81-83` `engPair`;渲染 `EnginePage.tsx:198-200`。
- 「Win · only」是手写字符串 `os: "Win · only"`(`EnginePage.tsx:72`)。其他卡片是 `"Win · mac"`(`:52,62`),`·` 本是双 OS 分隔符,「only」误套了模板。

**修复**:`.epair` 改 `var(--t-soft)`;英文文案改 tips 语气(如 `tip: pair with NVIDIA Broadcast to remove residual noise`);`"Win · only"` → `"Win only"`。

### B6. hero 状态行(NO REFERENCE / PIPELINE / STABLE)颜色层级调低 — 🎨 设计项

- 渲染 `App.tsx:1117-1128` → `RuntimeStatusStrip.tsx:57-104`;`.box.idle` 用 `var(--idle)` 蓝 + 边框 + ghost 背景(`styles.css:1376-1380`),是页面对比最强的元素之一。

**修复**:降低 `.status .box` 饱和/亮度(减边框、去 ghost 背景),`.m`/`b` 调向 `--t-soft`/`--t-mut`。

### B7. RUN PROBE 进度方块位置 — 🟡 部分存在

- `.pdots`(12 个 9×9 方块)在 `.apright` 纵向列中(`AdvancedPage.tsx:268-274`,`styles.css:348-394`),左缘对齐依赖 `.pbtn` `padding:0` 约定;`min-height:92px` 预留区在无结果时会让方块垂直位置发飘。具体偏移观感需运行时确认。

**修复**:统一 `.pdots`/`.pbtn`/`.presult` 左缘与行高,或方块与状态文字并入同一水平行。

### B8. NVAFX runtime 检查区(RECHECK)布局 — ✅ 存在(与 D5 的引擎页问题同源)

- `.echks` `max-height: 96px; overflow-y: auto`(`styles.css:903-909`)→ 检查项 7 行必出内部滚动条(即截图中的滚动条)。
- 行内 `.cpill` 固定 70px、`.cname` 固定 130px、`.cdetail` ellipsis(`styles.css:910-952`),Windows 上 dll 路径长文本被裁。
- 另发现:RECHECK 按钮传 `params.runtime_dir || undefined`,但按钮 title 显示的是后端解析后的实际目录(`EnginePage.tsx:231-244`),二者可能不一致,用户会误解在检查哪个目录。

**修复**:放宽/弹性化固定宽度,行加 `title` 悬浮全文;增大 `.echks` 高度或让整卡随页滚动;RECHECK 传参与 title 统一。

### B9. 高级页上的圆形左右箭头浮层 — ❌ 截图伪影,不处理(用户已确认)

- 全 `app/src` 无任何轮播/翻页/圆形浮层组件;且全局强制 `border-radius: 0 !important`(`styles.css:44-46`),应用内不可能渲染出圆形黑底控件。代码核实与用户确认一致:是截图工具带入的伪影,**无需任何改动**。

---

## C. 参数 / 交互 / 文案

### C1. 滚轮方向确认(Win 上滚=增,mac 双指上滚=增) — 🟡 mac 自然滚动下相反

- `VolumeWheel.tsx:79-87` 用原始 `e.deltaY`,`deltaY < 0` 视为增大:
  - Windows 滚轮上滚 → deltaY<0 → 增大 ✅ 符合期望。
  - macOS **自然滚动(默认开)**双指上滑 → deltaY 为正 → **减小** ❌ 与期望相反。
- 代码无平台/自然滚动归一化处理。

**修复**:macOS 分支按自然滚动做符号归一(可结合 `webkitDirectionInvertedFromDevice` 或平台判断反转),保证两平台「向上=增大」。

### C2. noise_gate_threshold_dbfs = -45 是否正确 — 🟡 值正确,暴露方式不对

- 前后端一致:`localvqe.rs:19`(`DEFAULT_NOISE_GATE_THRESHOLD_DBFS = -45.0`)与 manifest `processor_manifest.rs:143-147` 均为 -45,数值本身没问题(常规人声噪声门范围)。
- 但 `noise_gate` 默认 **OFF**(`localvqe.rs:68`),gate 关闭时阈值无任何效果,却始终显示——这正是「-45 对不对」困惑的来源。

**修复**:给该字段加 `requires: { noise_gate: true }`(复用现有 requires 隐藏机制),仅 gate 开启时显示;Hint 注明「仅噪声门开启时生效,-45 dBFS 为推荐值」。

### C3. localvqe 参数太多,评估可忽略项 — ✅ 建议隐藏 3 项

| 参数 | 作用(后端) | 默认 | 建议 |
|---|---|---|---|
| `model` | GGUF 模型路径(`localvqe.rs:210`) | 必填 | **移出高级页**——引擎页已有完整模型选择/下载 UI(`EnginePage.tsx:317-368`),重复暴露 |
| `library` | 推理动态库路径,留空自动探测(`localvqe.rs:634-659`) | auto | **隐藏**(纯部署细节) |
| `backend` | GGML 后端字符串(`localvqe.rs:315-319`) | auto | **隐藏** |
| `device` | GGML device id(`localvqe.rs:321-325`) | auto | **隐藏** |
| `threads` | CPU 线程数 | auto | 可保留,折叠 |
| `noise_gate` | 噪声门开关(唯一可实时生效项) | false | **保留** |
| `noise_gate_threshold_dbfs` | 门限 | -45 | 保留,依赖 gate 显示(见 C2) |

实现:在 `AdvancedPage.tsx:349-351` 的 `backendParams` 过滤中,像已排除 `reference_channels`/`ns` 那样排除 `library`/`backend`/`device`/`model`。**这同时直接缓解 B2 的溢出问题。**

### C4. 所有帮助弹窗要有「意义 + 推荐」 — ✅ 部分缺失

- Hint 机制已有(`Hint.tsx` + `AdvancedPage.tsx:35-84` 的 `DESC` 字典),但:
  - **完全没有 Hint**:`device`、`backend`(DESC 无键)。
  - **只有功能描述、无推荐语**:`model`、`library`、`threads`、`noise_gate(+threshold)`、`intensity_ratio`、`runtime_dir`、`model_path`、`on_runtime_error`、`use_default_gpu`、`disable_cuda_graph`、`sample_rate`、`frame_ms`。
  - `near_delay` 的 Hint(`AdvancedPage.tsx:228`)没有引导「run probe」。

**修复**:为每个保留参数补推荐动作语——排障类写「推荐保持 auto/默认」,延迟类写「推荐运行 Delay Probe 自动测得」。若采纳 C3/C5 的隐藏清单,需补写的 Hint 数量大幅减少。

### C5. NVAFX 参数评估 + 与引擎页重叠 — ✅ runtime_dir 明确重叠

- **重叠**:`runtime_dir` 在引擎页已有完整「RUNTIME 选择 + RECHECK」UI(`EnginePage.tsx:222-257`),高级页 NVAFX 分组又渲染一次同一字段 → 应只留引擎页。
- 各参数评估(`nvafx.rs`):
  - `intensity_ratio`(默认 1.0,`nvafx.rs:238`):**唯一有调音价值**,保留。
  - `on_runtime_error`(silence/bypass,`nvafx.rs:205-217`):行为策略,可保留。
  - `model_path`(留空按 GPU 架构自动选,`nvafx.rs:522-527`)、`use_default_gpu`、`disable_cuda_graph`:底层排障项,**建议隐藏**。
- GPU「选择」并不重叠(后端只有布尔 `use_default_gpu`,无可枚举下拉),但 GPU 状态信息已在引擎页展示。

### C6. 「无需修正 · 0ms」在 Windows 有歧义 + 应自动填 init 延迟 — ✅ 存在

- 文案:`i18n.tsx:251` `probeNoFix`;当 `recommended_near_delay_ms === 0` 时显示(`AdvancedPage.tsx:298-311`)。
- 语义:后端只有当 mic **领先** ref(负 lag)时才推荐 near delay(`probe_delay.rs:711-716`);测得 echo **+24.5ms**(mic 落后 ref,正常回声路径)时推荐值就是 0 → 显示「无需修正」。语义上对,但用户看到「测出 +24.5ms 却说无需修正」必然困惑。
- **自动填 init 延迟**:`AdvancedPage.tsx:116-129` 的 `probeInitialDelay` 第一行就是 `if (platform !== "macos" || kind !== "aec3") return null;` → **Windows 上 `initial_delay_ms` 从不自动回填**(macOS + AEC3 会填)。这正是用户说「要有自动填入 init 延迟」的缺口。

**修复**:① 文案区分两种 0:「对齐无需修正 · 回声延迟 +24.5ms 为正常回声路径,已交给 AEC3 追踪」;② 放宽 `probeInitialDelay` 的平台限制,Windows + AEC3 也用实测 echo 回填 `initial_delay_ms`(需后端确认 AEC3 delay hint 在 Windows loopback 时序下安全)。

---

## D. 功能缺失 / 后端接线

### D1. OFF 应为 passthrough(穿透)模式 — 🆕 当前 OFF = 整机停转

- 主开关 OFF 调 `stop_run` 停掉整个 sidecar(`App.tsx:774-791 → 755-765 → api.ts:138-139`),虚拟麦直接无声——通话对方会以为麦克风坏了。
- passthrough 处理器已存在(`crates/echoless-processors/src/passthrough.rs`),但目前**仅用于 probe 子进程**(`probe_delay.rs:242-243`)。

**修复**:OFF 时保持 run,把处理链切为 passthrough(近端原样直通虚拟麦)。需要后端支持热切 processor 或以 passthrough 配置快速重启;「完全停机」可以做成长按/菜单里的次级操作。

### D2. 一键 mute — 🆕 未实现

- 全仓无 mute 功能;最接近的是 `set_output_level(0)`(`api.ts:161-163`,实时通道)。实现方式:记住上次音量,toggle 0 ↔ 恢复,复用现有通道即可,无需重启。

### D3. 左上角返回 — 🆕 未实现

- 返回仅左下 `<<< OVERVIEW`(`App.tsx:1399-1421`)和 Esc(`App.tsx:582-586`);左上 `AppIcon` 无 onClick(`App.tsx:1074-1099`)。
- **修复**:非 overview 页给左上 AppIcon 加返回行为 + 箭头视觉,复用 footer 的层级逻辑(rtxsetup→engine,micsetup→diagnostics,其余→overview)。

### D4. 虚拟麦向导 Windows 不正常 — 🟡 向导只会开浏览器

- Windows 分支存在(`MicSetupPage.tsx:11-15` vb-cable),但所有状态的 action 最终都只是 `openUrl(下载页)`(`MicSetupPage.tsx:159,182`),**没有安装/装后检测/重启提示闭环**;后端也无 vb-cable install 命令。
- 设备检测是**纯名字匹配**(`CABLE Input/Output`,`MicSetupPage.tsx:34,39`),名字对不上就检测不到。
- Windows 权限态恒 `"unknown"`(`mic.ts:63-64`),向导的 permission 节点在 Windows 上永远空转(`MicSetupPage.tsx:76-77` 被 `isMac &&` 短路)。
- 注:`mic.ts` 是 dev 模拟器,真实数据来自后端 `doctor_audio`。

**修复**:Windows 流程改为「检测→引导下载→检测已装未生效(提示重启)→完成」的显式状态机;名字匹配加别名容错;长期可参考 `nvafx download-install` 做自动安装。

### D5. HEALTH 面板是否正常工作 — 🟡 4 项正常,2 项对 LocalVQE 未接线

- **接线正常**(后端真实递增):`ref_underruns`(`realtime.rs:508-516`)、`input_drops`(`realtime.rs:666-702`)、`output_underruns`(`realtime.rs:782`)、`stale_drops`(`realtime.rs:499,510`)。截图里 ref underruns=8 是真实数据(Windows loopback 时序抖动),面板在工作。
- **盲点**:`runtime_errors` / `diverged` 来自处理器 `stats()` 聚合(`stats.rs:38-52`);AEC3(`aec3.rs:420-426`)和 NVAFX(`nvafx.rs:312-314`)会填,但 **LocalVQE 直接返回 `ProcessorStats::empty`**(`localvqe.rs:258-259`),即使内部记了 `last_error`(`:234`)也不上报 → 用 LocalVQE 时这两项恒 0/NO。

**修复**:`localvqe.rs::stats()` 返回真实 error 计数与 diverged,与 NVAFX 对齐。

### D6. 随包 v1.3 不在「OPEN MODEL FOLDER」目录中 — ✅ 存在(随 D8 决策一并解决)

- 随包 v1.3 在 tauri Resource 目录(`resources/localvqe/models/localvqe-v1.3-4.8M-f32.gguf`,18.4M,`lib.rs:675-679` 读取,标 `source:"bundled"`)。
- 「OPEN MODEL FOLDER」打开的是**下载目录** `app_local_data/localvqe/models`(`EnginePage.tsx:354`,`lib.rs:612-615`);GET 下载也存这里(`lib.rs:718-757`)。
- `localvqe_assets` 把两处模型合并成一个列表返回,但 `models_dir` 只指向下载目录 → 用户打开文件夹看不到 v1.3,与卡片显示矛盾。

**修复(按 D8 决策)**:取消随包后,v1.3 与 v1.1/v1.2 一样走 GET 下载进下载目录,bundled/downloaded 双目录合并逻辑(`lib.rs:640, 671-704` 的 `collect_gguf` 去重与 `source:"bundled"` 分支)可直接删除——所有模型物理上只在一个目录,本问题自然消失。

### D7. localvqe 与 nvafx 数据目录不一致 — ✅ 存在

- localvqe(GUI/Tauri 侧):`app_local_data_dir()` 用 identifier `app.echoless.desktop`(`tauri.conf.json:5`)→ `%LOCALAPPDATA%\app.echoless.desktop\localvqe\models`(`lib.rs:612-613`)。
- nvafx(CLI 侧):硬编码品牌根 → `%LOCALAPPDATA%\Echoless\nvafx\2.1.0`(`nvafx.rs:489-497`,`nvafx_install.rs:114`)。
- 两个子系统各自选目录,同机出现两个顶层目录。

**修复**:统一到 `%LOCALAPPDATA%\Echoless\` 一个根(localvqe 不走 Tauri identifier),或 CLI 接受 GUI 传入的 `--data-root`;改目录需迁移已下载模型。

### D8. localvqe 打包方式 — 决策定案:不随包,全部走 HuggingFace 下载

**核实到的现状**(决策依据):
- 模型 v1.3(18M)当前随包在 Resource 目录;推理库走 Resource 目录 + 回退链(`lib.rs:275-306`)动态 dlopen(无 onnxruntime 依赖)。
- `resources/localvqe/native/` 里只有 macOS 的 `liblocalvqe*.dylib` + `libggml*`(5.5M),**没有 Windows 的 `localvqe.dll`**(匹配逻辑 `lib.rs:259-260`)→ Windows 包上 `native_ready=false`,卡片显示 runtime missing。
- 打包脚本缺库只警告不失败(`prepare-tauri-assets.mjs:270,310` 退出码 0)。

**决策(2026-07-03 用户确认)**:localvqe **不随包**,模型与 native runtime 统一从 HuggingFace 下载。理由:随包的 v1.3 造成双目录混乱(D6),且 Windows native 库本来就缺,与其补齐随包,不如统一下载路径。

**实施要点**:
1. 模型:把 v1.3 加入下载清单(复用现有 GET 通道 `lib.rs:709-757`,已有 SHA256 pin 校验 `lib.rs:541-562`),删除 `resources/localvqe/models` 随包资源与 `prepare-tauri-assets.mjs:212-213` 拷贝步骤。
2. native runtime:参考 nvafx 的 download-install 模式(`nvafx_install.rs`),将各平台 `localvqe.dll`/`liblocalvqe.dylib` + ggml 后端上传 HF,首次选用 LocalVQE 引擎时引导下载(引擎页已有 READY/GET 状态机可复用)。
3. 简化 `localvqe_assets`(`lib.rs:671-704`):去掉 bundled 分支与 `collect_gguf` 双目录去重,`models_dir` 单一化,连带解决 D6。
4. 下载目录落点应结合 D7 一起定(统一到 `%LOCALAPPDATA%\Echoless\` 根),避免迁移两次。
5. 收益:安装包缩小约 24M;代价:LocalVQE 首次使用需联网,UI 需明确「未下载 → 下载中 → 就绪」状态与失败重试。

---

## 建议处理顺序(2026-07-03 原稿,已被下方 triage 取代)

1. **一行/纯文案即可修**:B1(min size)、B3(背景色)、B5(颜色+文案)、C2(requires 隐藏)。
2. **高价值小改**:C1(mac 滚轮)、A2(ref 排除自家输出)、B2+C3(高级页滚动 + 减参,连带解 B4)、C6(probe 文案 + Windows 回填)。
3. **中等改动**:A1(真实权限检查)、A3(Windows ref 收敛)、A4(状态迟滞)、D5(LocalVQE stats 接线)、B8(runtime 检查区)。
4. **需产品/后端设计**:D1(OFF 穿透)、D2(mute)、D4(Windows 向导闭环)、D6+D7+D8(localvqe 全面转 HF 下载 + 数据目录统一到 `Echoless` 根,三条一起做)、A5/A6(tap 重采样 + 16k 标识)。

---

## Triage:29 条 vs phase-2 UI 重构设计稿(2026-07-04)

对照基准:`AEC/Design/overview.html` **v17 定稿**(其文件头注释记录了 v3→v17 全部设计决策,
其中 v8 明确「顺手修复」了 B2/B7/C2/C4/C6/A3,累计含 B4/B5/B6 共 9 条审计项)。
判定图例:🎨 **已被设计稿吸收**(P1 照稿实现即消) · 🔧 **仍生效**(逻辑/后端问题,换皮不解决,需单独落实) · ⚫ 关闭。

| # | 问题 | 判定 | 去向与说明 |
|---|---|---|---|
| A1 | 权限横幅每次开屏 | 🔧 | **P8**(后端 TCC 真实预检,与 UI 无关) |
| A2 | BlackHole 自环 | 🔧 | **P1 逻辑随迁**:availRefs 过滤排除当前 selOutput,纯前端逻辑,新 UI 实现设备下拉时带上 |
| A3 | Win ref 列出全部设备 | 🎨+🔧 | 设计稿 v8 已定收敛目标态;过滤**逻辑**在 P1 实现(system+none+虚拟回环,物理端点折叠) |
| A4 | 状态标签抽搐 | 🔧 | **P1 逻辑随迁**:runtimeTelemetry 加 2-3s 稳定窗/连续 N 帧滞后;新 UI 状态盒有 scramble 动画,但底层翻转不防抖照样闪 |
| A5 | Process Tap 仅 48k | 🔧 | **P8**(后端:helper 上报实际采样率 + tap 流重采样) |
| A6 | 16↔48 自适配标识 | 🔧 | **P1 文案项**:LocalVQE 卡片写「16k(管线自动 48↔16 适配)」;可复用 `.rsmp` badge |
| B1 | 窗口可拉低于默认 | 🔧 | **P1**:新 UI 定稿尺寸后 `min_inner_size = 默认`(设计稿画布 1080×680 vs 现 1040×640,P1 需先定终值) |
| B2 | 高级页参数溢出 | 🎨 | v9.1 已去 `.apright min-height:92px` 预留、页面可滚;配合 C3 减参,P1 照稿实现 |
| B3 | resize 白边 | 🔧 | **P1**:builder 加 `.background_color(...)`,值用新色板底色(暖碳黑 #131312 系,不再是 #08090b) |
| B4 | model 输入框截断 | 🎨 | v3.1 已修(wide 340px);P1 照稿 |
| B5 | NVAFX pair 黄色/Win·only | 🎨 | v3 已修(中性色 + "Win only");P1 照稿 |
| B6 | hero 状态行层级过高 | 🎨 | v7 已调暗调小;P1 照稿 |
| B7 | RUN PROBE 方块位置 | 🎨 | v8/v9 probe 区重做(12 点定时进度、dopen padding 0);P1 照稿 |
| B8 | NVAFX runtime 检查区 | 🎨+🔧 | 样式随新稿重做;**RECHECK 传参与 title 不一致**是逻辑 bug,P1 随迁修 |
| B9 | 圆形箭头浮层 | ⚫ | 截图伪影,关闭 |
| C1 | mac 自然滚动方向反 | 🔧 | **P1 逻辑随迁**:VolumeWheel 符号归一化(新 UI 保留滚轮调音量交互) |
| C2 | gate 阈值恒显示 | 🎨 | 设计决策=gate OFF 时**调暗**(v8,非 requires 隐藏);P1 照稿 |
| C3 | localvqe 参数太多 | 🔧 | **P1 逻辑随迁**:backendParams 过滤掉 model/library/backend/device(设计稿页面布局按减参后排的) |
| C4 | Hint 缺意义+推荐 | 🎨+🔧 | v8 补了部分;**完整推荐语要在 P1 的 i18n 词条里逐个补全**(C3/C5 隐藏后数量大减) |
| C5 | NVAFX runtime_dir 重复暴露 | 🔧 | **P1 逻辑随迁**:高级页 NVAFX 分组滤掉 runtime_dir(引擎页已有完整 UI);model_path/use_default_gpu/disable_cuda_graph 隐藏 |
| C6 | 「无需修正·0ms」歧义 + Win 不回填 | 🎨+🔧 | 文案 v8 已改;**P4 的 probe 公式改动会让推荐值永远 ≥ 平台偏置,「0ms」场景自然消失**——P1 文案须按 P4 新语义写(「正 lag 由 AEC3 追踪;负向余量 D ms 已生效」);Windows 回填 initial_delay_ms 放宽 → 随 P4 一并确认安全性 |
| D1 | **OFF 应为 passthrough** | 🔧 | **之前未被任何任务覆盖,2026-07-04 补录为 P8 首项**。与 P1 有产品交叉:v17 设计稿 OFF=整机停转语义(sysoff 调暗 + MONITOR HELD),若改三态(ON=AEC / OFF=穿透 / 长按=全停),电源开关与状态字语义要重定 → **实现前需用户拍板三态交互**,详见 P8 |
| D2 | 一键 mute | 🔧 | **P8**(小活:记忆音量 toggle 0↔恢复,复用 set_output_level 实时通道;UI 挂点等 P1 footer 定稿) |
| D3 | 左上角返回 | 🔧 | **P1 决策项**:新 UI 导航结构(footer <<< + Esc)照旧,左上 AppIcon 是否加返回由 P1 定稿时顺手定 |
| D4 | Win 虚拟麦向导 | 🔧 | **P8**(显式状态机:检测→引导下载→装后未生效提示重启→完成;名字匹配加别名) |
| D5 | LocalVQE stats 未接线 | 🔧 | **P8**(小活:localvqe.rs stats() 返回真实 errors/diverged) |
| D6 | 随包 v1.3 不在目录 | 🔧 | **P8**(随 D8 一并消失) |
| D7 | 数据目录不一致 | 🔧 | **P8**(统一 `%LOCALAPPDATA%\Echoless\` 根) |
| D8 | localvqe 转 HF 下载 | 🔧 | **P8**(决策已定 2026-07-03,实施要点见 D8 条目) |

**统计**:已吸收(照稿实现即消)8 条;逻辑随迁(P1 实现时必须带上,换皮不自动解决)9 条;
后端/产品项(P8)10 条;随 P4 消解 1 条(C6 部分);关闭 1 条(B9)。

**P1 逻辑随迁清单**(照稿画皮时容易漏的,实现 checklist):A2 自环排除、A4 防抖、B1 尺寸锁定、
B3 背景色、B8 RECHECK 传参、C1 滚轮归一、C3 减参过滤、C5 去重复字段、C4 Hint 推荐语补全、A6 文案。
