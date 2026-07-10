# Echoless `dev` 分支深度审计与多维评分

> 审计日期：2026-07-10
> 审计基线：`dev@1aa747708d4d8eac8d732c7ed8c827e22d744508`，与 `origin/dev` 同步，审计开始时工作区干净
> 仓库规模：371 个 tracked files；Rust workspace + 独立 AEC3 workspace + Tauri 2 / React 18 前端
> 审计范围：`crates/`、`aec3/`、`app/src`、`app/src-tauri`、`app/scripts`、`tools/macos-process-tap-poc`、`.github/workflows/build.yml`、`configs/`、公开文档与发布资产
> 历史基准：逐条对照 `docs/internal/audit/AUDIT_REPORT_2026-07-05.md`、`FIX_PROGRESS.md` 与 `DECISIONS.md`；本报告只记录未关闭的新问题或已发生的新漂移，不重复 B-01～B-24、S-01～S-11、P-01～P-04、A-01～A-07、C-01～C-06、T-01～T-10、D-01～D-08
> 审计方式：graphify 定位 + 七维并行源码审计 + 主审交叉复核 + 本地全套质量门 + exact-HEAD GitHub Actions 核验 + 依赖 advisory 可达性分析；`repo-audit` 技能的外部报告模板在安装目录中缺失，因此沿用本仓库上一轮报告结构并补充评分、验收与证据字段

## 结论

`dev@1aa7477` **可构建、可测试，数值内核和跨平台打包链已有较强基础，但暂不建议直接升为 stable release**。本轮确认 **32 项**：P0×0、P1×7、P2×14、P3×11。

主要发布阻断不是“测试全红”，而是测试尚未覆盖到的边界：立体声参考的帧原子性、Process Tap 失联后的零参考假运行、LocalVQE 错误态无界缓冲、旧 sidecar 退出事件污染新会话、安装包签名/公证、第三方许可证清单、以及 Tauri 后端测试未进入 CI。

正向证据同样明确：本地共执行 886 个通过测试（另 3 个 ignored），root/AEC3/Tauri 的 fmt、clippy、build 全绿，前端 typecheck/test/build 全绿，当前 HEAD 的 Windows、macOS Apple Silicon、macOS Intel、Linux GitHub Actions 均成功。

## 多维评分

评分采用 10 分制；P0 通常使维度不高于 4.0，P1 通常使维度不高于 6.9。通过测试和跨平台构建会提高“证据置信度”，但不会抵消已证实的运行态或发布缺陷。综合分按权重加权，不按问题数量机械平均。

| 维度 | 权重 | 得分 | 结论 |
|---|---:|---:|---|
| B 正确性 / 可靠性 | 25% | **5.2** | 4 个 P1；参考链、模型错误态与会话代际存在实质缺陷 |
| S 安全 / 供应链 | 15% | **6.0** | 下载校验和 capability 较好，但发布包无可信签名且存在可达 HTML/导航注入 |
| P 性能 / 实时性 | 10% | **6.2** | 主体已做后台 I/O，但 10 ms 线程与设备 callback 仍有分配/序列化 |
| A 架构 / 契约 | 15% | **7.0** | workspace 边界清晰，GUI/CLI manifest 与事件契约出现多个真理源 |
| C 代码质量 / 类型 | 10% | **8.0** | 静态质量门干净，仍有类型退化和零消费者公开 API |
| T 测试 / CI | 15% | **6.0** | 测试总量可观，但 Tauri 21 测试不在 CI、无 PR 门、组件层缺失 |
| D 配置 / 文档 / 发布 | 10% | **5.2** | 许可证、Linux 可选资产、RC 版本和发布 smoke 尚不满足稳定发布 |
| **综合** | **100%** | **6.1 / 10** | **候选版工程质量；修完 P1 并裁决关键发布策略后再评 stable** |

## 严重度汇总

| 严重度 | 数量 | 条目 |
|---|---:|---|
| P0 | 0 | — |
| P1 | 7 | B-25、B-26、B-27、B-28、S-12、T-11、D-09 |
| P2 | 14 | B-29、B-30、S-13、P-05、A-08～A-10、T-12、T-13、T-15、D-10～D-12、D-15 |
| P3 | 11 | B-31、B-32、S-14、S-15、P-06、P-07、C-07、C-08、T-14、D-13、D-14 |

## 给修复 agent 的使用说明

1. 以 `dev@1aa7477` 为定位基线；行号漂移后按函数名和证据链定位，不要只按行号机械修改。
2. 默认一个审计条目一个 commit，并遵循仓库 Lore commit protocol；每项完成后在 `audit/FIX_PROGRESS.md` 记录 commit、验证与遗留风险。
3. 推荐顺序：
   - 第一批：B-25、B-26、B-27、B-28、T-11；这些直接影响运行可信度或回归门。
   - 第二批：S-12、D-09；涉及证书、发布权限和许可证工具，先读取 `audit/DECISIONS.md`。
   - 第三批：B-29、B-30、S-13、P-05、A-08～A-10、T-12/T-13/T-15、D-10～D-12/D-15。
   - 第四批：P3 卫生项。
4. B-25 与 B-29 都改 reference 计数/帧语义，应分别提交但在同一批联调；B-28 与 A-09 都碰事件契约，应共用 `run_id` / fixture 设计，避免两个临时协议。
5. 不要把 `quick-xml` 锁文件命中直接描述成产品可利用漏洞；当前只证明 `plist -> tauri*` 的传递依赖，未发现产品解析外部 XML 的入口。
6. 未经 `DECISIONS.md` 裁决，不新增许可证工具、schema/codegen 工具或改变自定义 LocalVQE 模型支持政策。

## 修复后验收基线

```bash
# root workspace
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --workspace --locked

# AEC3 workspace
(cd aec3 && cargo fmt --all --check)
(cd aec3 && cargo test --workspace --locked)
(cd aec3 && cargo clippy --workspace --all-targets --locked -- -D warnings)

# Tauri backend（T-11 修复后也必须在 CI 执行）
(cd app/src-tauri && cargo fmt --check)
(cd app/src-tauri && cargo test --locked)
(cd app/src-tauri && cargo clippy --all-targets --locked -- -D warnings)
(cd app/src-tauri && cargo build --locked)

# frontend / integration
(cd app && pnpm exec tsc --noEmit)
(cd app && pnpm test)
(cd app && pnpm build)
(cd app && pnpm prepare:tauri-assets -- --dev --skip-helper-build)
(cd app && pnpm tauri build --debug --no-bundle --ci)

# dependencies
cargo audit
(cd aec3 && cargo audit)
(cd app/src-tauri && cargo audit \
  --ignore RUSTSEC-2024-0429 \
  --ignore RUSTSEC-2026-0186 \
  --ignore RUSTSEC-2026-0194 \
  --ignore RUSTSEC-2026-0195)
(cd app && pnpm audit --prod)
```

条目专项验收必须叠加在上述基线上；不能只靠现有测试全绿宣告修复。

---

## B. 正确性 / 可靠性

### B-25【P1】立体声 reference ring 不是帧原子；一次半帧读写即可永久串声道

- **位置**：`crates/echoless-cli/src/realtime.rs:348-355,934-979`；`crates/echoless-cli/src/realtime/macos_process_tap.rs:284-295`；`crates/echoless-cli/src/realtime/resample.rs:595-623`。
- **证据/触发**：ring 存的是交织 `f32` 样本；CPAL/Process Tap 逐样本 `try_push`。消费端先把 ch0 弹入私有 buffer，再发现 ch1 缺失并返回，已弹出的 ch0 不回滚。下一轮会把前一帧的 R 当 L、下一帧的 L 当 R；ring 只余一个 slot 时也会出现半帧提交。
- **影响**：用户启用受支持的 stereo reference 后，AEC 可静默接收跨帧/跨声道 far-end，抑制效果持续恶化，直到另一次偶然错位。
- **修复指引**：生产和消费都以完整 frame 为原子单位；容量不足时整帧丢弃，消费前确认 `occupied_len >= channels`，或使用 frame struct 的 ring。
- **专项验收**：单独发布 L 不得改变任一声道；仅余一个 slot 时整帧丢弃；高频交错调度后 L/R 序列长期保持对应。

### B-26【P1】macOS Process Tap EOF/启动失败后仍以零参考“正常运行”

- **位置**：`crates/echoless-cli/src/realtime/macos_process_tap.rs:152-200,223-301`；`tools/macos-process-tap-poc/Sources/main.swift:635-662`；`crates/echoless-cli/src/realtime.rs:633-659`。
- **证据/触发**：`start()` spawn helper 后不等 ELTP header/ready 就返回；reader 遇 EOF/错误只退出自己的线程，不清全局 `running`。权限失败、helper 崩溃或 pipe 错误后，主循环继续，用 reference underrun 的全零帧处理。
- **影响**：GUI/CLI 仍显示运行，但 reference-based AEC 实际失效；现有 CPAL stream error 修复不覆盖此 pipe 路径。
- **修复指引**：启动阶段等待合法 header 或 helper 提前退出；区分主动 Drop 与意外 EOF，意外退出发结构化 `stream_error` 并停止 run。
- **专项验收**：header 前退出不得发 `started`；意外 EOF 只报一次并使 `running=false`；主动关闭不误报。

### B-27【P1】LocalVQE 连续错误时 near/far 缓冲无界增长，恢复后还会回放旧音频

- **位置**：`crates/echoless-processors/src/localvqe.rs:142-171,229-239,703-709`。
- **证据/触发**：每次 `process_loaded` 先向两个 `VecDeque` 追加样本，native `process_frame()?` 错误发生在 pop 之前；上层只记录错误并直通。本次失败 hop 会持续被重试，新帧继续追加。
- **影响**：16 kHz/10 ms 下约增长 128 KB/s，即约 460 MB/小时；瞬态恢复后输出队列的时间轴落后于实时输入。
- **修复指引**：失败时消费/丢弃失败 hop，或清空 near/far/out 并重置流状态；恢复必须从当前时间点重新起流。
- **专项验收**：注入连续 native error 一小时等价循环，队列长度保持有界；恢复后的首帧不得包含错误前样本。

### B-28【P1】旧 sidecar reader 的过期 exit 会污染新 run 的 tray 与前端状态

- **位置**：`app/src-tauri/src/sidecar.rs:151-194,219-265,290-296`；`app/src-tauri/src/proc.rs:77-93`；`app/src/App.tsx:988-1008`。
- **证据/触发**：配置路径比较只阻止旧 reader 从 `RunState` 删除新 child；旧 reader 随后仍无条件 `update_tray_tooltip(false)` 并 emit exit。前端在判断 `intentional` 前先清空 live、I/O、reference source、CLI version 和 supported controls。
- **影响**：run B 仍占用音频设备，但 tray 显示 STOPPED，GUI 丢失热控制/诊断能力；“显示已停但仍采集”属于运行可信度问题。
- **修复指引**：每次 run 分配单调 `run_id`；status/exit 都携带 ID，只有 active ID 可更新 RunState/tray，前端忽略旧代事件。只让 `mark_run_exited` 返回 bool 不足以处理已排队的旧事件。
- **专项验收**：用 barrier 卡住 A reader，启动 B 后释放 A；B、tray、I/O、runtime controls 均保持有效，B 自身退出仍只发一次 exit。

### B-29【P2】clock-skew 检测漏掉反方向，stereo 与 mono 的计数单位也不一致

- **位置**：`crates/echoless-cli/src/realtime/stats.rs:286-315,450-503`；`crates/echoless-cli/src/realtime.rs:640-644,940-979`；`crates/echoless-cli/src/realtime/diagnostics.rs:408-435,497-511,617-643`。
- **证据/触发**：detector 只使用 output underrun 与 reference overflow/drop，未使用已采集的 output overrun/ref underrun；reference drop 按交织 samples 计，output 按 mono frames 计；实际欠载帧数被压成布尔 1。默认 T3 又把部分信号迁移到另一计数器。
- **影响**：mic 快于 output 的漂移方向完全失明；stereo 下相关性可因 2× 单位差被误判；live 与 diagnostics summary 可能相互矛盾。
- **修复指引**：统一为 frames；双向检测；保留真实欠载帧数；live 与 summary 共用同一算法并报告方向。
- **专项验收**：±22.4% 两方向均告警；2N stereo samples 与 N frames 相关；T3 开关前后 live/summary 一致。

### B-30【P2】CoreAudio listener 移除失败后仍释放 callback context，存在 UAF

- **位置**：`app/src-tauri/src/device_watch.rs:45-53,77-108`。
- **证据/触发**：callback 无条件解引用裸 `AppHandle`；`AudioObjectRemovePropertyListener` 的非零 `OSStatus` 被忽略，随后立即 `Box::from_raw`。移除失败意味着 listener 可能仍注册，后续设备通知会访问已释放 context。
- **影响**：罕见系统 API 失败可转化为 use-after-free / 崩溃；当前测试无法注入该分支。
- **修复指引**：仅在 remove 成功且回调排空后释放；失败时保留 ownership、记录 OSStatus 并重试，或把 callback context 改成明确的共享生命周期状态。
- **专项验收**：注入 remove 失败时 context 仍存活；成功路径恰好释放一次；失败可观测。

### B-31【P3】前端 TOML basic string 漏转义控制字符

- **位置**：`app/src/api.ts:324-363`。
- **证据/触发**：`tomlString` 只转义反斜杠和双引号；实际 LF/CR/BS/FF/NUL 会生成非法单行 basic string，例如 `backend = "cuda<LF>fallback"`。
- **影响**：恢复的持久化参数、程序化输入或极端设备/路径字符串会令 config validation 失败；当前证据不支持配置注入，因为引号已正确转义。
- **修复指引**：用完整 TOML serializer；若保持当前输入形状，至少使用兼容的完整转义并测试所有控制字符。
- **专项验收**：LF、CR、BS、FF、NUL、ESC、DEL、quote、backslash、Unicode 均能 round-trip 或被明确拒绝。

### B-32【P3】秒级日志文件名碰撞会合并会话并绕过 8 MiB 上限

- **位置**：`app/src-tauri/src/logging.rs:31-47,60-80,143-146`。
- **证据/触发**：文件名精度只有秒，碰撞时 append；每个进程的 `written` 都从 0 开始。仓库未启单实例插件，双实例或一秒内重启会共享文件。
- **影响**：两个进程各可再写 8 MiB，破坏单文件 cap 与“一启动一文件”的取证边界。
- **修复指引**：文件名加入高精度时间、PID、attempt，使用 `create_new(true)` 循环；不要用 append 作为主路径。
- **专项验收**：同秒并发 init 生成不同文件，每个文件单独受 cap 约束。

---

## S. 安全 / 供应链

### S-12【P1】公开安装包无平台签名/公证，README 反而要求用户移除 Gatekeeper 隔离

- **位置**：`.github/workflows/build.yml:340-365,524-549`；`app/src-tauri/tauri.conf.json:19-38`；`tools/macos-process-tap-poc/build.sh:32-38`；`README.md:125-136`；`README.zh-CN.md:113-122`。
- **证据/触发**：release job 直接构建/stage DMG、NSIS、deb/AppImage；没有 Developer ID/notarytool/stapler、Authenticode/SignTool 或 provenance gate。helper 找不到本地 `Echoless Dev` 时使用 ad-hoc 签名。公开说明让用户执行 `sudo xattr -rd com.apple.quarantine /Applications/Echoless.app`。
- **影响**：麦克风/系统音频应用缺少发布者身份和来源完整性信任锚；asset 被替换时用户无法用 OS 信任链区分，且官方步骤主动削弱 Gatekeeper。
- **修复指引**：macOS 对 app、CLI、helper、dylib 分层 Developer ID 签名，启用 hardened runtime，notarize + staple DMG；Windows 对主程序、sidecar、NSIS 做 Authenticode；发布 job 对签名、公证和 provenance 失败即阻断，删除 xattr 指令。Apple 官方要求分发软件使用 Developer ID 并建议公证；Microsoft SignTool 可验证文件未被篡改且来自可信来源。
- **专项验收**：`codesign --verify --deep --strict`、`spctl -a -vv`、`stapler validate`、`SignTool verify /pa /v` 全通过；干净 VM 无需绕过安全机制即可安装。
- **外部依据**：[Apple notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)、[Microsoft SignTool verification](https://learn.microsoft.com/en-us/windows/win32/seccrypto/using-signtool-to-verify-a-file-signature)、[GitHub artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)。

### S-13【P2】设备名经 animejs `innerHTML` 动画形成可达 HTML/CSS 注入与外部导航

- **位置**：`crates/echoless-cli/src/realtime/devices.rs:454-480,896-925` → `app/src-tauri/src/commands.rs:16-24` → `app/src/App.tsx:1896-1902,1983-1989` → `app/src/components/Dropdown.tsx:57-59` → `app/src/components/ScrambleText.tsx:55-64`；CSP：`app/src-tauri/tauri.conf.json:15-17`。
- **证据/触发**：OS/驱动设备名原样进入 `scrambleText({ text })`，并作为 `innerHTML` 动画目标。animejs 4.4.1 最终/中间帧返回未转义 target text，renderer 直接设置 `target.innerHTML`。用仓库实际 animejs + Chrome 动态复现：`<style>` payload 在动画帧中生效，`meta refresh` 发起外域导航；inline handler 被 CSP 拦截。
- **影响**：能控制本机 USB/蓝牙/虚拟音频设备名的攻击者可覆盖/伪装 UI，或把主 WebView 导航到外域并使应用失效。远端页默认没有 Tauri remote capability，当前证据不支持 RCE。
- **修复指引**：动画每帧只写 `textContent`，绝不以 `innerHTML` 为目标；Tauri window 增加只允许本地 app origin（dev 时 localhost）的 navigation guard；CSP 补 `form-action 'none'; object-src 'none'; base-uri 'none'; frame-src 'none'`。
- **专项验收**：`<>&"'`、style、meta payload 永远显示为纯文本，DOM 中不新增 element，不产生外部请求。

### S-14【P3】HTTPS host allowlist 可被 WHATWG 反斜线规范化绕过

- **位置**：`app/src-tauri/src/platform.rs:96-166`；命令注册：`app/src-tauri/src/lib.rs:69`。
- **证据/触发**：手写 parser 只把 `/ ? #` 当 authority 终止符。`https://evil.example\@github.com` 被它识别为 allowlisted `github.com`，标准 URL parser 则规范化为 host `evil.example`、path `/@github.com`。
- **影响**：当前前端调用均为硬编码 URL，且 remote IPC 默认不开放，因此属于纵深防御；未来一旦 URL 来源变为数据驱动，会形成 allowlist 绕过。
- **修复指引**：使用 `tauri::Url` / `url::Url`，拒 credentials，按规范化后的 scheme/hostname/port 精确匹配。
- **专项验收**：覆盖 backslash、userinfo、encoded delimiter、port、大小写和尾点。

### S-15【P3】崩溃日志原样保存设备名/路径，文档鼓励直接附整文件但无脱敏提示

- **位置**：`app/src-tauri/src/logging.rs:31-48,60-80,116-125`；`app/src-tauri/src/sidecar.rs:268-288`；`crates/echoless-cli/src/run_command.rs:11-24`；`CHANGELOG.md:29-33`。
- **证据/触发**：CLI stderr 含 mic/ref/output 名称、用户路径和诊断目录，日志保留 7 天/20 文件/8 MiB；CHANGELOG 建议 bug report 直接附文件。
- **影响**：公开 issue/客服转发时可泄露用户名、蓝牙设备名和本机目录。未发现 token/私钥，故为 P3。
- **修复指引**：提供“导出已脱敏诊断”；默认哈希/遮盖 home path 与设备标识，并提示用户复核；确认 Unix 0600 / Windows 用户 ACL。
- **专项验收**：含用户名、设备名、绝对路径的 fixture 导出后不含原文。

### 安全正向证据与依赖例外

- 用户值均经 `Command::args/arg` 或 Node `spawnSync(..., shell:false)` 分参数传递，未发现 shell 拼接注入。
- NVAFX ZIP 使用 `enclosed_name()`；LocalVQE/NVAFX 下载均有 filename allowlist、大小/SHA 校验和原子 rename。
- Tauri capability 只开放 core window/event 与 dialog，无 shell/fs/updater；`open_path` canonicalize 后限制在 brand data root。
- Actions 已钉 commit SHA；Cargo 使用 `--locked`，pnpm 使用 frozen lockfile + minimumReleaseAge；未发现 token/private key。
- `app/src-tauri/Cargo.lock` 命中 `quick-xml 0.39.4` 的 [RUSTSEC-2026-0194](https://rustsec.org/advisories/RUSTSEC-2026-0194.html) 与 [RUSTSEC-2026-0195](https://rustsec.org/advisories/RUSTSEC-2026-0195.html)，但当前只经 `plist -> tauri*`，未发现外部 XML 入口；`glib`/`memmap2` 也是函数/平台限定的 accepted risk。保留 CI ignore 时应跟踪上游升级，不把它们包装成“零风险”。

---

## P. 性能 / 实时性

### P-05【P2】10 ms 处理线程仍持续分配并执行状态序列化

- **位置**：`crates/echoless-cli/src/realtime.rs:661-700`；`crates/echoless-processors/src/chain.rs:71-77`；`crates/echoless-processors/src/localvqe.rs:261-273`；`crates/echoless-processors/src/nvafx.rs:310-325`；`crates/echoless-cli/src/realtime/stats.rs:603-681`；`crates/echoless-cli/src/realtime/emit.rs:21-35`。
- **证据/触发**：每帧无条件 `chain.stats()` 并新建 Vec；模型/error 字符串被 clone；status 周期在处理线程构造 JSON 和三组 waveform Vec；emitter 首次调用还会创建 channel 并 spawn thread。
- **影响**：仅 stats Vec 就至少每分钟 6000 次 heap allocation；首次 telemetry 可能在实时线程创建 OS thread，增加 deadline miss/爆音风险。
- **修复指引**：启动阶段预初始化 emitter；实时线程只写预分配 fixed snapshot，worker 负责 JSON/waveform；缓存静态字符串；无 telemetry/diagnostics 时跳过 node stats。
- **专项验收**：warmup 后完整 steady-state iteration 零分配；第一次 status 不 spawn；阻塞 stdout 不影响处理循环。

### P-06【P3】CPAL callback 内惰性构造 FFT resampler 并扩容

- **位置**：`crates/echoless-cli/src/realtime/resample.rs:35-88,212-280,448-510`；调用点 `crates/echoless-cli/src/realtime.rs:924-944,1052-1082`。
- **证据/触发**：首个 callback 或 buffer size 变化时构造 `FftFixedIn/FftFixedOut`、planes、scratch、Vec/VecDeque。
- **影响**：设备实时线程在启动首帧和尺寸变化时承担分配及 FFT plan 初始化。
- **修复指引**：建 stream 前预热/预留；对合法 buffer size 使用预分配 adapter，或把 SRC 移出 callback。
- **专项验收**：首个 callback 与尺寸变化路径均零分配，并无启动 underrun 回归。

### P-07【P3】diagnostics recycle pool 初始为空，首帧和池耗尽时在线程内分配

- **位置**：`crates/echoless-cli/src/realtime/diagnostics.rs:165-205,281-316,344-375`。
- **证据/触发**：recycle channel 未预填；空池时 `Box::new`，首帧 near/far/out 三个 Vec 扩容；writer 落后再次耗尽时继续扩池。
- **影响**：开启录制的第一个 10 ms 帧必然分配，背压期间又会分配。
- **修复指引**：`DiagnosticRecorder::new` 按 frame/reference channels 预建固定池；池空时计数并丢 frame，不在线程内扩池。
- **专项验收**：首个 `write_frame` 与池耗尽路径零分配，drop 计数准确。

---

## A. 架构 / 契约

### A-08【P2】LocalVQE 模型目录有两个真理源，后端支持的 v1/自定义模型无法在 GUI 选择

- **位置**：`app/src-tauri/src/localvqe.rs:28-49,144-158,202-225,244-267`；`app/src/pages/EnginePage.tsx:38-47,431-472`；`app/src/pages/AdvancedPage.tsx:44-45`。
- **证据/触发**：后端 pin 4 个模型并枚举目录全部 `.gguf`，README 也承诺可选择任意 `.gguf`；前端却只遍历固定 3 项，且 Advanced 隐藏 `model` 参数。
- **影响**：后端 pin 的 `localvqe-v1-1.3M-f32.gguf` 与用户自备模型在产品 UI 中不可达。
- **修复指引**：由后端返回统一 catalog 元数据 + installed/discovered 状态，前端直接渲染；未知模型至少显示通用行。先裁决 `DEC-04`。
- **专项验收**：临时数据根放入 v1 和任意自定义 `.gguf`，两者均可见、可选；每个后端 pin 都映射到 UI。

### A-09【P2】CLI JSONL → Tauri → TypeScript 运行事件契约已真实漂移

- **位置**：`crates/echoless-cli/src/realtime/stats.rs:571-583`；`crates/echoless-cli/src/realtime/control.rs:249-250,564-575`；`app/src-tauri/src/sidecar.rs:219-227`；`app/src/api.ts:225-226`；`app/src/types.ts:167-171,232-245`；`app/src/App.tsx:892-900,944-949`。
- **证据/触发**：CLI 已发 `clock_skew_warning/resolved`，TS union 没有；解析错误时 `control_error.cmd=null`，TS 声明必填 string。Tauri 用无类型 `Value` 透传，`listen<RunEvent>` 只是假静态保证。
- **影响**：当前 UI 可显示 `null: message`；后续新事件无法被编译器或测试阻断，已有“未知事件导致黑屏”的历史证据。
- **修复指引**：优先共享 Rust DTO/枚举并生成受版本控制 fixtures/schema；低成本方案是 CLI golden JSONL + Vitest dispatcher 契约。与 B-28 的 `run_id` 一并设计，先裁决 `DEC-03`。
- **专项验收**：每种 CLI event 至少一条 fixture；前端全部分类，未知事件安全忽略，`cmd:null` 有明确文案。

### A-10【P2】processor manifest 的 pipeline/数值约束被前端裁掉，UI 又维护硬编码副本

- **位置**：`crates/echoless-cli/src/processor_manifest.rs:25-57,99-116`；`app/src/types.ts:49-57,77-79`；`app/src/pages/AdvancedPage.tsx:314-320,449-481`；`app/src/App.tsx:315-319`；`app/src/api.ts:303-308`。
- **证据/触发**：CLI manifest 输出默认值、min/max 和 output curve；TS 类型没有完整 `max/pipeline`，通用数值控件不传已有 `spec.min`，前端另写默认值和 500 ms 上限。
- **影响**：用户可输入 `tail_ms=0`、`initial_delay_ms=501`，到后端 validate/hot-control 才失败；后端改边界时 UI 不同步。
- **修复指引**：补全 manifest 类型，从 manifest 初始化 pipeline，并把 min/max/integer/curve 全量透传到 UI。
- **专项验收**：遍历 manifest 所有 number 参数，UI 控件应用同一边界；越界在提交前拒绝。

---

## C. 代码质量 / 类型

### C-07【P3】已有 `ProcessorKind` 联合类型，核心状态链却退化成 `string`

- **位置**：`app/src/types.ts:59-63`；`app/src/App.tsx:294-305,357-405,580-590`；`app/src/api.ts:314-320`。
- **证据/触发**：localStorage 任意字符串可进入当前 engine；`engineReady()` 找不到 processor 时仍返回 true；最终 TOML 也接受任意 kind。
- **影响**：损坏/旧存档形成“UI 已就绪、启动才报 unknown processor”的状态，类型检查无法覆盖分支。
- **修复指引**：状态全链使用 `SelectableProcessorKind`；读取持久化数据后按 manifest guard/迁移，未知 kind 不 ready。
- **专项验收**：未知 kind 回退默认；legacy alias 正确迁移；合法 kind 往返持久化。

### C-08【P3】`echoless-audio-io` 保留零消费者的公开占位 API

- **位置**：`crates/echoless-audio-io/src/null.rs:7-50`；`crates/echoless-audio-io/src/lib.rs:80-119`；`docs/internal/architecture/audio_io_scope.md:14-22`。
- **证据/触发**：`NullSource/NullSink/MonotonicClock/StdClock/DeviceKind/DeviceInfo` 除定义和文档外无调用，测试也未实例化；注释写明未来/占位。
- **影响**：扩大公开 API 与维护面，文档让读者误以为已有 fallback/test 路径。
- **修复指引**：优先删除并同步文档；若保留，必须指定 owner/启用条件和真实消费测试。先裁决 `DEC-06`。
- **专项验收**：删除方案跑 root 全门；保留方案新增真实消费点和行为测试。

---

## T. 测试 / CI

### T-11【P1】21 个 Tauri 后端测试在 CI 中从未执行

- **位置**：`.github/workflows/build.yml:86-123,109-115`；`app/src-tauri/src/lib.rs:23-24`；`app/src-tauri/src/tests.rs:77-433`；`app/src-tauri/src/logging.rs:166-175`；`app/src-tauri/src/platform.rs:232-257`。
- **证据/触发**：root workspace exclude Tauri；matrix 对 Tauri 只有 clippy/build，`clippy --all-targets` 只编译 test target，不执行断言。本地实际有 21 tests。
- **影响**：URL 白名单、路径限制、配置原子写、子进程回收、日志日期等回归可在 CI 继续为绿。
- **修复指引**：Windows/macOS matrix 与 Linux job 都执行 `(cd app/src-tauri && cargo test --locked)`。
- **专项验收**：CI 日志明确显示测试运行；故意翻转路径 allowlist 断言必须使 job 失败。

### T-12【P2】workflow 没有 PR/功能分支自动门

- **位置**：`.github/workflows/build.yml:3-7`。
- **证据/触发**：仅 `push main/dev`、tag 和 manual dispatch；无 `pull_request`，普通功能分支 push 也不触发。
- **影响**：fmt/clippy/test/前端契约问题只能在合入 dev/main 后发现。
- **修复指引**：增加 PR 轻量 quality job；昂贵 LocalVQE/native bundle 可只在目标分支/tag 执行。
- **专项验收**：测试 PR 自动出现 required check；文档 PR 可跳昂贵打包但不跳基础门。

### T-13【P2】前端只有 Node 纯函数测试，关键组件与事件生命周期零覆盖

- **位置**：`app/vite.config.ts:14-16`；`app/package.json:23-30`；`app/src/App.tsx:851-1008`；`app/src/components/ErrorBoundary.tsx:24-104`；现有 `engineLogic.test.ts`、`runtimeControls.test.ts`、`telemetryGuard.test.ts`。
- **证据/触发**：25 tests 全是纯函数；没有 `.test.tsx`、jsdom 或 Testing Library。上一轮 `docs/internal/audit/DECISIONS.md` 已裁决引入组件测试，但未完成。
- **影响**：S-13、A-08、A-09、B-28 这类“数据存在但组件接线/生命周期错误”可长期全绿。
- **修复指引**：落实既有裁决，引入 jsdom + Testing Library；首批覆盖 ScrambleText 安全文本、未知事件、部分 status、ErrorBoundary、LocalVQE discovered model、订阅卸载。
- **专项验收**：`pnpm test` 包含 DOM 渲染/卸载；恶意设备名无 element/navigation；旧 `run_id` event 不改变状态。

### T-14【P3】品牌数据根三平台路径契约没有直接测试

- **位置**：`crates/echoless-paths/src/lib.rs:7-51`；消费者 `app/src-tauri/src/localvqe.rs:110-121`、`app/src-tauri/src/logging.rs:32-39`。
- **证据/触发**：该 crate 零测试；env override 与 Windows/macOS/Linux 目录同时影响模型、日志、诊断和 open-path 安全边界。
- **影响**：环境变量优先级或大小写/目录拼接漂移只能人工发现。
- **修复指引**：把纯路径推导提成接收 env map 的函数；串行测试 override/空 override，并让三平台 matrix 覆盖本平台默认根。
- **专项验收**：`cargo test -p echoless-paths --locked` 有明确测试数，三平台 CI 执行。

### T-15【P2】待发布 release artifacts 没有安装/资产 smoke

- **位置**：`.github/workflows/build.yml:282-293,340-365,485-523`；`app/scripts/smoke-tauri-bundle.mjs:89-109`。
- **证据/触发**：smoke 检查的是 `target/debug/bundle`；tag 随后另做 release build并直接 stage。Linux release 也直接上传 deb/AppImage。
- **影响**：debug gate 绿不能证明 release profile、签名后路径或最终 installer 资源完整。
- **修复指引**：对将要 stage 的 DMG/NSIS/deb/AppImage 解包/安装，执行 asset presence、CLI processors/version、native FFI load 后才发布。
- **专项验收**：故意删除 release resource，release job 必须失败。

---

## D. 配置 / 文档 / 发布

### D-09【P1】第三方许可证清单实质不完整，且对 Cargo 许可证范围的说明错误

- **位置**：`THIRD-PARTY-LICENSES.md:1-5,266-267`；`app/package.json:16-21`；`app/src-tauri/tauri.conf.json:25-30`；`.github/workflows/build.yml:295-313,499-514`。
- **证据/触发**：清单只列 AEC3、LocalVQE、NVAFX、外部虚拟声卡，却声称 Rust deps 只有 MIT/Apache/BSD 且“enumerated in Cargo.lock”。Cargo.lock 不含 license 字段；实际 `cargo metadata` 包含 MPL-2.0、ISC、Unicode-3.0、Zlib、0BSD、Unlicense 等，React/anime/Tauri production JS 也未列 notice。
- **影响**：当前公开二进制的版权/许可证通知不完整，是 stable 发布合规阻断。
- **修复指引**：用 target-aware Rust license collector（如 cargo-about）+ JS production lockfile collector 生成 notices/SBOM，合并手工 LocalVQE/NVAFX 条目；unknown/denied/missing license 在 CI fail。先裁决 `DEC-02`。
- **专项验收**：解包 DMG/NSIS/deb/AppImage/CLI；notice 与各 target production transitive graph 逐项对账。

### D-10【P2】Linux release 允许 LocalVQE native build 失败后继续发布，README 却无条件承诺 runtime 随包

- **位置**：`.github/workflows/build.yml:440-483,485-523`；`app/scripts/prepare-tauri-assets.mjs:259-296`；`README.md:68-88`；`README.zh-CN.md:59-78`。
- **证据/触发**：Linux native step `continue-on-error:true`；失败只 warning，随后仍构建并发布。默认 `prepare-tauri-assets` 不带 require flag，明确会在运行时失败；README 无平台例外。
- **影响**：CI 可绿色发布一个 advertised LocalVQE 引擎必失败的 Linux installer。
- **修复指引**：tag/release Linux fail closed，强制 `--require-localvqe-assets`；或构建时从 manifest/UI/docs 移除该 engine。先裁决 `DEC-05`。
- **专项验收**：故意让 native build fail，release 必须失败或产物明确禁用引擎；解包做 FFI load smoke。

### D-11【P2】RC tag 产物内部版本仍是 stable `1.1.0`

- **位置**：`Cargo.toml:16-21`；`app/src-tauri/Cargo.toml:1-5`；`app/package.json:1-5`；`app/src-tauri/tauri.conf.json:1-5`；`.github/workflows/build.yml:340-365,550-552`。
- **证据/触发**：`v1.1.0-rc.3` 被标 prerelease，但公开 assets 名为 `Echoless_1.1.0_*`，CLI/bundle metadata 也不含 rc；仓库尚无 stable `v1.1.0` tag，CHANGELOG 已标最终 1.1.0。
- **影响**：RC/正式版在版本排序、升级、缓存和支持日志中不可区分。
- **修复指引**：加 tag↔manifest CI gate；RC 使用合法 prerelease version，并为 Windows installer 映射单调 file/product version。
- **专项验收**：文件名、CLI、CFBundle、Windows file version 与 tag 对应，stable 可正确覆盖 RC。

### D-12【P2】随 CLI 发布的 `configs/example.toml` 对资产与 SRC 实现均已陈旧

- **位置**：`configs/example.toml:35-41`；`Cargo.toml:39-40`；实际实现 `crates/echoless-processors/src/chain.rs:1-9`；打包 `.github/workflows/build.yml:295-313,499-514`。
- **证据/触发**：sample 声称 Windows CI artifact 同时带 DLL+v1.3 模型，实际 CLI archive 只含 CLI/config/licenses；sample 和 root 注释还称 processor boundary 是 placeholder linear，当前已是 rubato FFT。
- **影响**：用户按公开 sample 找不到模型/DLL，并对音质路径作错误判断。
- **修复指引**：区分 GUI（runtime 随包、模型按需下载）与 standalone CLI 前置条件；更新 rubato 描述及 emergency fallback。
- **专项验收**：干净解包按 sample quickstart；docs grep 不再命中旧线性占位说法。

### D-13【P3】Linux 数据目录大小写写错

- **位置**：实现 `crates/echoless-paths/src/lib.rs:4,39-47`；文档 `docs/CLI.md:144-145`、`docs/CLI.zh-CN.md:140-141`。
- **证据/触发**：实现固定 `$XDG_DATA_HOME/Echoless` / `~/.local/share/Echoless`，文档写 `~/.local/share/echoless`。
- **影响**：Linux 大小写敏感，用户会找不到模型/日志或创建影子目录。
- **修复指引**：修正文档并说明 XDG_DATA_HOME；最好让 doctor/paths 输出真实目录。
- **专项验收**：路径单测 + 文档路径检查。

### D-14【P3】app README 的 sidecar 解析顺序与实现不一致

- **位置**：`app/README.md:27-32`、`app/README.zh-CN.md:25-30`；实现 `app/src-tauri/src/bin_resolve.rs:39-104`。
- **证据/触发**：文档只写 `ECHOLESS_BIN -> root release`，并误称打包后由 env 注入；实现依次检查 env、当前 exe 邻接、Tauri resource、target-triple binaries、root release/debug。
- **影响**：开发与打包故障排查会沿错误路径进行。
- **修复指引**：文档复用真实顺序或链接实现，避免复制漂移。
- **专项验收**：README 顺序与 resolver table test 对齐。

### D-15【P2】`tauri-plugin-decorum` 传递依赖已被 Rust 标记为 future-incompatible

- **位置**：`app/src-tauri/Cargo.toml:25`；`app/src-tauri/Cargo.lock` 中 `tauri-plugin-decorum 1.1.1 -> cocoa 0.25 -> block 0.1.6`。
- **证据/触发**：`cargo build`/`cargo clippy` 当前成功，但 `cargo report future-incompatibilities --id 1` 明确指出 `block 0.1.6` 的 uninhabited static 代码未来 Rust 将拒绝。
- **影响**：一次稳定 Rust 升级即可把 macOS Tauri build 从 warning 变为 hard error；当前 CI 没有 future-incompat gate。
- **修复指引**：评估升级/替换 decorum 或上游 cocoa 链；在依赖替换前把 future-incompat report 纳入周期性 CI，防止静默累积。
- **专项验收**：依赖树不再含 `block 0.1.6`，`cargo report future-incompatibilities` 无该项，窗口装饰功能回归通过。

---

## 当前验证证据

| 验证项 | 当前结果 |
|---|---|
| `cargo test --workspace --locked` | 118 passed，3 ignored |
| `cd aec3 && cargo test --workspace --locked` | 722 passed |
| `cd app/src-tauri && cargo test --locked` | 21 passed（但 CI 未执行，见 T-11） |
| `cd app && pnpm test` | 3 files / 25 tests passed |
| root / AEC3 / Tauri fmt | 全通过 |
| root / AEC3 / Tauri clippy `-D warnings` | 全通过；Tauri 另有 `block 0.1.6` future-incompat warning |
| root release build / Tauri backend build | 通过 |
| frontend typecheck / production build | 通过；114 modules，JS 312.74 kB（gzip 101.50 kB） |
| `pnpm tauri build --debug --no-bundle --ci` | 通过，产出 `app/src-tauri/target/debug/echoless-app` |
| `pnpm audit --prod` | 无已知漏洞 |
| root `cargo audit` | 无 vulnerability/warning |
| AEC3 `cargo audit` | `rand 0.9.2` informational/unsound warning，仅由 `proptest` dev-dependency 可达；未见产品调用路径 |
| Tauri raw `cargo audit` | quick-xml 2 个 DoS advisory + glib/memmap2 2 个 informational unsound；CI 以有注释 ignore 接受，产品可达性见安全例外 |
| GitHub Actions exact HEAD | [run 29027163989](https://github.com/Haor/Echoless/actions/runs/29027163989)：Windows、macOS ARM64、macOS Intel、Linux 全成功 |
| Git 状态 | 写报告前 `dev...origin/dev` clean；产品代码零修改 |

## 已扫无新增高置信问题的方面

- AEC3 wrapper 的 render/capture fallback、NVAFX effect/DLL RAII、LocalVQE/NVAFX 下载 hash/staging、output preroll 与 CPAL stream-error 基础回收。
- root/AEC3 数值测试、fmt/clippy matrix；未发现新的 panic/unwrap 热点或 shell command injection。
- capability 最小化、`open_path` brand-root 限制、ZIP traversal、Windows DLL safe search、Actions SHA pin。
- Cargo workspace 依赖方向无循环或平台层反向依赖；产品 TS 无 `any` / `@ts-ignore` 逃逸。

## 审计限制

- 本轮没有真实 Windows USB 麦克风/虚拟声卡、macOS Process Tap 权限失败、持续 LocalVQE native error 的硬件长跑；B-25～B-30 依据可达源码时序与现有 API 契约确认，修复时必须加确定性 fault-injection tests。
- 未持有 Developer ID / Windows code-signing certificate，无法对现有公开资产补签；S-12 依据 workflow、配置、README 与公开 release 资产核验。
- Linux release artifact 未在本机实际安装；D-10/T-15 由 workflow 控制流与脚本搜索路径确认。
- graphify 图谱生成于较早 commit，仅用作导航，最终结论均回到 `1aa7477` 源码复核。

## 待决项

需要用户裁决的 6 项见 `audit/DECISIONS.md`：

1. S-12：签名/公证到位前是否暂停 stable release。
2. D-09：许可证/SBOM 生成工具和 CI policy。
3. A-09 + B-28：共享事件 schema/codegen 还是 golden fixtures。
4. A-08：自定义 LocalVQE 模型是正式支持还是收紧承诺。
5. D-10：Linux LocalVQE fail closed 还是显式禁用引擎。
6. C-08：删除零消费者 API 还是明确保留。

本阶段按 audit-only 约束停止；未生成修复 prompt，未修改产品代码。
