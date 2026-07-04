# Echoless 代码审计清单

> 审计对象：`echoless/`(第一方代码,约 11k 行 Rust + 4.6k 行 TS/React + Tauri 后端)。
> **不含** `vendor/aec3`(第三方 vendored 引擎,只读分析对象)与 `reference_repos/`。
> 审计日期:2026-06-10。审计方式:逐文件人工通读 + 交叉验证。
>
> **用途**:本文档供人工审核,并交由 sub-agent 逐条修复。每条 finding 含
> `ID / 严重度 / 位置(file:line) / 问题 / 为什么重要 / 修复指令`,可独立认领。

---

## 0. 威胁模型与范围说明(校准严重度的前提)

Echoless 是**本地单机自用桌面工具**(见项目约定:本地自用、无 license 顾虑)。其安全态势:

- **无网络服务端**,不监听端口,不接受远程输入。
- 外部输入面仅:① 用户提供的 TOML/设备 selector;② 从固定 GitHub release(`Haor/echoless`)
  与固定 HF repo(`LocalAI-io/LocalVQE`)下载的 runtime/模型 zip(均经 TLS);③ Tauri webview 内
  前端 → 后端 IPC(前端代码本身可信,但 webview 若被注入则 IPC 成为提权面)。
- 主要受害场景是**多用户共享主机**(`/tmp` 抢占)与**本地 DLL 植入**(Windows 搜索顺序)。

因此多数"安全"问题落在 **Medium/Low**;真正影响产品质量的反而是**实时音频热路径的性能/正确性**
与**架构可维护性**。请按"修复优先级"(§7)而非纯严重度排期。

---

## 1. 严重度汇总

| ID | 类别 | 严重度 | 一句话 |
|----|------|--------|--------|
| PERF-1 | 性能 | **High** | 实时处理热路径每帧(10ms)多次堆分配,有 xrun/爆音风险 |
| ARCH-1 | 架构 | **High** | `realtime.rs`(3346)/`main.rs`(3306)巨石文件,职责混杂 |
| ARCH-2 | 架构 | Medium | `echoless-core` 的实时编排抽象(`run_realtime`/`ControlApi`)是死代码,核心被旁路 |
| QUAL-1 | 正确性 | **High** | 链路边界重采样为**无状态逐块线性插值**,LocalVQE(16k)路径有可闻块边界伪影 |
| SEC-1 | 安全 | Medium | `open_url` Tauri 命令无 scheme 校验,Windows `cmd /C start` 有参数注入面 |
| SEC-2 | 安全 | Medium | nvafx 安装:下载的 `SHA256SUMS.txt` **优先于**内置 pin 哈希,削弱固定版本完整性 |
| SEC-3 | 安全 | Medium | 固定 `/tmp` 配置文件名(`echoless-run.toml` 等)→ 符号链接/TOCTOU/并发覆盖 |
| SEC-4 | 安全 | Medium | LocalVQE 动态库从 CWD/exe 目录按名加载 → Windows DLL 植入 |
| SEC-5 | 安全 | Low | LocalVQE 模型下载(`download_localvqe_model`)无完整性校验,直接喂给 ggml |
| SEC-6 | 安全 | Low | `nvidia-smi` 与系统 DLL 经 `PATH` 查找,可被本地 PATH 劫持 |
| ROB-1 | 健壮性 | Medium | Tauri 长耗时命令(nvafx install/download、validate)同步阻塞 worker 线程 |
| ROB-2 | 健壮性 | Medium | `DiagnosticRecorder::request_finish` 分离 writer 线程,进程立即退出可能丢失 finalize |
| ROB-3 | 健壮性 | Low | `RunState` 全用 `Mutex::lock().unwrap()`,一次 panic 即毒化整把锁 |
| ROB-4 | 健壮性 | Low | `.lines().flatten()` 静默丢弃读失败/非 UTF-8 行 |
| QUAL-2 | 正确性 | Low | `PipelineConfig::frame_size()` 的 `sample_rate*frame_ms` 可 u32 溢出 |
| QUAL-3 | 性能 | Low | `LocalVqe` 缓冲 `Vec::drain(..hop)` O(n) + 每帧 `vec![0.0;hop]` 分配 |
| QUAL-4 | 性能 | Low | `RealtimeStats` 每帧缓存全部波形样本,仅为算 64 桶峰值 |
| QUAL-5 | 规范 | Low | 重复实现:`copy_into`/`rms_db*`/三套线性重采样器分散各处 |
| TEST-1 | 测试 | Medium | CI 不构建/不测试 Tauri 后端(`echoless-app` 独立 workspace,被 `--workspace` 漏掉) |
| TEST-2 | 测试 | Medium | 无 `cargo-audit`/`cargo-deny` 依赖漏洞扫描;无 `cargo fmt --check`;前端无 lint/test |
| TEST-3 | 测试 | Low | `chain.rs` 重采样/声道适配、`output_level` 溢出无单测覆盖 |
| CFG-1 | 配置 | Low | CI 从 `localai-org/LocalVQE` 的 `main` 非固定 `--depth 1` 克隆,构建不可复现 |
| DOC-1 | 文档 | Low | `nvafx download-install` 的硬编码 pin 哈希散落 `main.rs`,无集中校验/轮换说明 |
| **RUNTIME-1** | 现场 bug | **High** | 音量调节报 `unknown command set_output_level`:GUI 跑的是**过时 debug 二进制** |
| **RUNTIME-2** | 现场 bug | **High** | 启用 LocalVQE 报库加载失败:**原生库 `liblocalvqe.dylib` 缺失**,且无获取途径 |
| SON-1 | aec3 | Low | echoless `let _ =` 吞掉 `process_render/capture` 的 `Result`(错误静默) |
| SON-2 | aec3 | Low | 热路径 `assert!`(release 也触发)与 `process_capture` 潜在每帧分配未经 profile |
| SON-3 | aec3 | Low | aec3 独立 workspace(edition 2024),未进 echoless CI 的 clippy/audit gate |
| **PKG-1** | 打包 | **High** | 打包后 GUI **找不到 CLI**:无 `externalBin`/`resources`,dev 回退用编译期路径,`ECHOLESS_BIN` 未注入 |
| **LAT-1** | 正确性 | Medium | 首页 `LATENCY` **低估**真实延迟:漏算输入 ring(mic_q)+ 设备硬件缓冲 + I/O 重采样缓冲 |
| **ARCH-3** | 架构 | Medium | 前后端耦合 = shell CLI + 临时文件 + **改配置即重启**;`echoless-core` 共享层被旁路(非最优) |
| FE-1 | 前端性能 | Medium | 4 个 `requestAnimationFrame` 循环 60fps **常驻**(停机也画)+ 每帧 `getBoundingClientRect` |
| FE-2 | 前端性能 | Low | 每 80ms status 触发整个 `App`(1268 行)重渲染,live/health 未隔离 |
| FE-3 | 前端 UX | Medium | 任一配置改动(设备/NS/参考/采样率/引擎)都 stop+start 子进程 → 每次调档有**音频断点** |

---

## 2. 性能(实时音频热路径)

### PERF-1 — 实时处理链每帧多次堆分配 [High]
- **位置**:`crates/echoless-processors/src/chain.rs:62-104`(`ProcessorChain::process`)、
  `adapt()` `chain.rs:131-158`、`remap_channels` `:162-178`、`resample_linear` `:181-197`。
- **问题**:`process()` 每个 10ms 帧都执行 `near_base_mono.to_vec()`,并对每个节点的
  near/far/out 各调用 `adapt()`,后者内部再 `Vec::new()`、`vec![Vec::with_capacity…]`、
  `planes`/`resampled`/`out` 多次分配。一条链每帧产生 ~10+ 次堆分配。
- **为什么重要**:该函数运行在独立处理线程,每 10ms 必须按时产出一帧。堆分配会拿全局
  allocator 锁、可能触发 mmap/munmap,在负载抖动时造成处理超时 → `out_ring` 欠载 →
  输出 underrun → **可闻爆音/卡顿**。这是实时音频工程的头号反模式。
- **修复指令**:
  1. 在 `ProcessorChain` 内为每个节点预分配并复用 scratch buffer(`near_n`/`far_n`/`out_n`
     与 `cur_near` 的 ping-pong),`process()` 内只 `clear()`+`extend`,不 `to_vec()`/`Vec::new()`。
  2. 将 `adapt`/`remap_channels`/`resample_linear` 改为写入调用方传入的 `&mut Vec<f32>`(复用容量),
     而非返回新 `Vec`。
  3. 用有状态重采样器替换无状态版本(见 QUAL-1),scratch 随之常驻。
  4. 加一个 `#[test]`/bench 断言稳态下 `process()` 零分配(可用 `dhat`/计数 allocator,或至少
     回归测试链输出长度与数值)。

### QUAL-3 — LocalVqe 流式缓冲 O(n) drain 与每帧分配 [Low]
- **位置**:`crates/echoless-processors/src/localvqe.rs:124-172`(`process_loaded`)。
- **问题**:`self.near_buffer.push(...)` 后 `self.near_buffer.drain(..hop)`(`Vec` 头部 drain 是
  O(n) 整体搬移);`far_buffer` 同理;且每次 `process` 都 `let mut frame_out = vec![0.0; hop]`。
- **修复指令**:把 `near_buffer`/`far_buffer` 换成 `VecDeque<f32>`(或固定环形缓冲),`frame_out`
  提升为结构体字段复用。注意 `process_frame` 需要连续切片——`VecDeque` 可用
  `make_contiguous()` 或维护一个复用的连续 scratch。

### QUAL-4 — RealtimeStats 缓存全部波形仅为算 64 桶 [Low]
- **位置**:`crates/echoless-cli/src/realtime.rs:1735-1737`(`extend_from_slice`)、
  `peak_waveform` `:1597-1614`、`maybe_print` `:1775-1777`。
- **问题**:`observe()` 每帧把 near/far/out 全量 `extend_from_slice` 进三个 `Vec`,直到打印间隔
  (默认 1000ms,80ms for GUI)才清空并对全量算 64 桶峰值。属冗余内存与 CPU。
- **修复指令**:改为**在线**分桶——维护 64 个运行峰值累加器,`observe()` 时按当前累计样本
  位置更新对应桶,打印时直接读出,免存全量样本。

---

## 3. 架构

### ARCH-1 — 巨石文件,职责混杂 [High]
- **位置**:`crates/echoless-cli/src/realtime.rs`(3346 行)、`crates/echoless-cli/src/main.rs`(3306 行)。
- **问题**:
  - `realtime.rs` 一个文件混了:设备枚举/选择、流构建、两套重采样器、`process_loop`、
    诊断录制(WAV writer 子系统约 600 行)、统计聚合、JSON 状态、运行时控制协议。
  - `main.rs` 一个文件混了:clap 定义、配置校验(约 400 行)、nvafx doctor/install/
    download(含 PowerShell/curl 下载、zip 解压、SHA256)、probe-delay 的完整 DSP(蜂鸣生成、
    互相关延迟估计约 400 行)。
- **为什么重要**:单文件 3k+ 行 → 难定位、难 review、合并冲突高、单测边界模糊,违背"职责边界明确"。
- **修复指令**(纯机械拆分,不改行为,逐步提交):
  - `realtime.rs` 拆为模块:`realtime/devices.rs`(选择/枚举/JSON/doctor)、
    `realtime/resample.rs`(两套 resampler + 单测)、`realtime/diagnostics.rs`
    (`DiagnosticRecorder`/`DiagnosticWriter`)、`realtime/stats.rs`(`RealtimeStats`/`StatsSample`)、
    `realtime/control.rs`(runtime control 协议)、`realtime/mod.rs`(`run_with_options`+`process_loop`)。
  - `main.rs` 拆为:`cli.rs`(clap 结构)、`config_validate.rs`、`nvafx_install.rs`(下载/zip/sha)、
    `probe_delay.rs`(DSP)。`main.rs` 仅保留分发。
  - 每步拆分后跑 `cargo test --workspace` 保证零行为变化。

### ARCH-2 — 核心实时编排抽象是死代码,core 被旁路 [Medium]
- **位置**:`crates/echoless-core/src/lib.rs:155-162`(`ControlApi` trait)、`:288-301`(`run_realtime` 直接 `bail!`)。
- **问题**:`echoless-core` 设计目标是"平台无关编排 + ControlApi 控制面",但实时路径整体落在
  CLI 的 cpal 实现里,`run_realtime` 是永久 `bail!` 桩,`ControlApi` 无任何实现者。core 实际只
  承担了离线 `run_offline` + 配置类型 + output_level 数学。
- **为什么重要**:抽象与实现不一致会误导后续开发(以为 core 是编排入口),且 Tauri/daemon 复用
  路径无处落脚。属"抽象债"。
- **修复指令**:二选一并在代码注释/文档明确——
  (A) 若短期不抽回 core:删除 `run_realtime` 桩与 `ControlApi`(或移入 `docs` 作设计草案),
      避免误导;
  (B) 若要落地:把 `realtime.rs` 的 `process_loop`/运行时控制下沉为 core 的 `ControlApi` 实现,
      CLI 与 Tauri 都经它调用。推荐先做 (A),(B) 列入 roadmap。

---

## 4. 正确性 / DSP 质量

### QUAL-1 — 链路边界重采样为无状态逐块线性插值(LocalVQE 路径有伪影)[High]
- **位置**:`crates/echoless-processors/src/chain.rs:138`(注释自承占位)、`resample_linear:181-197`、
  `adapt:131-158`。`Cargo.toml` 已引入 `rubato` 但未使用。
- **问题**:`adapt()` 对**每个 10ms 块独立**做线性重采样,块之间无相位/历史延续。当链含 LocalVQE
  (天然 16k)时,realtime 主路径每帧做 48k→16k→48k 往返重采样,块边界产生不连续 → 可闻的
  周期性伪影/嘶声,且线性插值本身有混叠。`realtime.rs` 的设备边界重采样器
  (`InterleavedLinearResampler`/`OutputLinearResampler`)是**有状态**的(已较好),但 `chain.rs`
  的节点边界重采样**无状态**,二者不一致。
- **为什么重要**:这是 AEC 工具的核心信号质量。LocalVQE 是主推的 neural 残余抑制路径,边界
  伪影会直接劣化人声且干扰 AEC3 收敛。
- **修复指令**:
  1. 用 `rubato`(已在依赖)实现**有状态**的节点边界重采样,每个节点边界各持一对
     up/down resampler 实例,跨帧保留内部状态;`ProcessorChain` 持有它们(配合 PERF-1 的 scratch 复用)。
  2. 立体声 far 在降/升采样时**保留 L/R**,勿如 `remap_channels:162-178` 那样 downmix-then-spread
     (注释 `:161` 已标 TODO)。
  3. 补单测:对已知正弦/啁啾信号做 48k→16k→48k 往返,断言块边界连续性(相邻块端点差 < 阈值)
     与基频保真。

### QUAL-2 — frame_size() 整数溢出 [Low]
- **位置**:`crates/echoless-core/src/lib.rs:140-143`。
- **问题**:`(self.sample_rate * self.frame_ms / 1000)` 在 `u32` 域相乘,`sample_rate*frame_ms`
  超 4.29e9 即 debug 下 panic、release 下回绕。虽调用点多有上游校验,但该方法本身可被直接调用
  (如 `cmd_offline`/`cmd_nvafx_offline` 用它定帧)。
- **修复指令**:改为 `((self.sample_rate as u64 * self.frame_ms as u64) / 1000).max(1) as u32`,
  或在 `frame_size` 内 `checked_mul` 并对非法组合返回 `Result`/clamp。

---

## 5. 安全

### SEC-1 — `open_url` 无 scheme 校验,Windows 参数注入面 [Medium]
- **位置**:`app/src-tauri/src/lib.rs:335-348`(`open_url`)。前端调用点均为硬编码常量
  (`App.tsx:894`、`MicSetupPage.tsx`、`RtxSetupPage.tsx`),当前不可被外部控制。
- **问题**:命令 `cmd /C start "" <url>`(Windows)/`open <url>`/`xdg-open <url>` 直接把
  前端传入字符串当 URL,**未校验是否 http(s)**。`cmd start` 会执行本地路径/可执行(如传入
  `calc.exe` 即启动计算器)。一旦 webview 被注入(或将来 URL 改为来自 CLI JSON 的 `hint`/`action`
  字段),即成本地命令执行面。
- **修复指令**:在 `open_url` 入口校验 `url` 以 `https://` 或 `http://` 开头(拒绝其它 scheme 与
  相对/本地路径),并优先改用官方 `tauri-plugin-opener`(它内部做了校验,免自起 `cmd`)。

### SEC-2 — 下载 SHA256SUMS 覆盖内置 pin 哈希 [Medium]
- **位置**:`crates/echoless-cli/src/main.rs:1631-1641`(`expected_sha256_for_release_asset`)、
  内置 pin 见 `:1801-1820`(`expected_sha256_for_asset`)、下载逻辑 `:1391-1408`。
- **问题**:对**默认固定 tag**,本应以内置 pin 哈希为信任锚;但实现是
  `release_sha256sums.get(asset).cloned().or_else(内置)` —— **优先**用从同一 release 下载的
  `SHA256SUMS.txt`,仅在其缺该条目时才回落内置 pin。`SHA256SUMS.txt` 是可变文件、无签名,一旦
  release 资产被篡改(账号被盗/资产替换),攻击者同时改 zip 与 SHA256SUMS 即可绕过内置 pin,
  随后这些 DLL 被 `Library::new` 加载执行(`nvafx.rs:1114`)。
- **为什么重要**:这是供应链完整性的信任锚倒置。TLS 只防 MITM,不防 release 内容被替换。
- **修复指令**:对 `DEFAULT_NVAFX_RELEASE_TAG`,**内置 pin 优先**:若内置存在期望值则强制用之,
  并(可选)与下载的 SHA256SUMS 交叉校验、不一致即报错。仅对非默认 tag 才使用下载的 sums。
  长期:对 SHA256SUMS.txt 引入签名(minisign/cosign)。

### SEC-3 — 固定 `/tmp` 配置文件名(符号链接/TOCTOU/并发覆盖)[Medium]
- **位置**:`app/src-tauri/src/lib.rs:381`(`echoless-validate.toml`)、`:407`(`echoless-run.toml`)。
- **问题**:`std::env::temp_dir().join("echoless-run.toml")` 是**可预测、全局共享**路径。多用户主机上
  攻击者可预置同名符号链接,`std::fs::write` 跟随符号链接覆盖受害者文件;两个 Echoless 实例也会
  互相覆盖 run/validate 配置。
- **修复指令**:写到 app 私有目录(`app.path().app_local_data_dir()`,已在 `localvqe_models_dir`
  中用过)或用带进程/随机后缀的唯一临时文件(参考 `main.rs` probe 用 `process::id()+纳秒` 的命名)。
  创建时用 `create_new`(O_EXCL)拒绝已存在路径。

### SEC-4 — LocalVQE 动态库从 CWD 按名加载(DLL 植入)[Medium]
- **位置**:`crates/echoless-processors/src/localvqe.rs:483-512`(`library_candidates`)。
- **问题**:未显式配置 `library` 时,候选含 `current_dir().join("localvqe.dll")` 等。若 Echoless
  从攻击者可写目录启动,恶意 `localvqe.dll` 会被 `Library::new` 加载执行。对比之下 nvafx 用了
  `DllDirectoryGuard`+绝对路径(`nvafx.rs:1108-1133`)的安全做法。
- **修复指令**:默认仅在**可执行文件同目录**及其 `localvqe/` 子目录查找(去掉 CWD);CWD 查找
  仅在显式相对路径配置时允许。Windows 上对 localvqe 也套用 nvafx 的 `SetDefaultDllDirectories`/
  `AddDllDirectory` 安全加载模式。

### SEC-5 — LocalVQE 模型下载无完整性校验 [Low]
- **位置**:`app/src-tauri/src/lib.rs:234-255`(`download_localvqe_model`)。
- **问题**:filename 做了路径穿越防护(好),但下载后**无 SHA256/签名校验**即落盘,随后被
  ggml 解析加载。模型文件解析器(ggml)历史上有 parser 漏洞;HF repo 被替换或缓存投毒时无防护。
- **修复指令**:为受支持模型维护期望 SHA256 清单(类 nvafx),下载后校验;校验失败删除 `.part`
  并报错。理想上从 HF 同时取 `*.sha256`/etag 比对。

### SEC-6 — `nvidia-smi` 与系统 DLL 经 PATH 查找(本地 PATH 劫持)[Low]
- **位置**:`crates/echoless-processors/src/nvafx.rs:583`(`Command::new("nvidia-smi")`)、
  `find_windows_system_dll:566-580`(把 `PATH` 全部目录纳入查找)。
- **问题**:`nvidia-smi` 不带绝对路径,沿 `PATH` 解析;`find_windows_system_dll` 也搜 `PATH`。
  本地攻击者在更靠前目录放同名程序/DLL 可影响检测结果。属本地、低危。
- **修复指令**:`nvidia-smi` 优先从 `%ProgramFiles%\NVIDIA Corporation\NVSMI` / `System32` 绝对
  路径解析;系统 DLL 查找以 `%SystemRoot%\System32` 为先、`PATH` 仅作兜底并记录来源。

---

## 6. 健壮性 / 错误处理

### ROB-1 — Tauri 长耗时命令同步阻塞 worker [Medium]
- **位置**:`app/src-tauri/src/lib.rs`:`nvafx_install`(`:274-308`)、`nvafx_download_install`
  (`:314-332`)、`validate_config`(`:379-390`,内部 CLI 会跑 nvafx doctor→nvidia-smi)、
  `run_json`(`:85-92`)。对比:`probe_delay`(`:131-164`)正确用了 `spawn_blocking`。
- **问题**:这些 `#[tauri::command]` 同步执行 `Command::output()`(下载/解压可达数十秒),阻塞
  Tauri 命令线程;无超时。下载/安装期间该 invoke 卡住,异常 CLI 还可能永久挂起。
- **修复指令**:把所有会 spawn CLI 且可能长耗时/阻塞的命令改为 `async` + `spawn_blocking`
  (照搬 `probe_delay`);并对 `Command` 加合理超时(用 `wait_timeout` 或读超时),超时杀子进程。

### ROB-2 — 诊断 request_finish 分离 writer,可能丢失 finalize [Medium]
- **位置**:`crates/echoless-cli/src/realtime.rs:1233-1241`(`request_finish`)。
- **问题**:异步停止时 `self.writer.take()` 后 `let _ = ...`(丢弃 JoinHandle),仅 spawn 一个
  线程去发 `Finish`。若进程在 writer 线程完成 `finalize_wav`/rename 前退出,会残留 `.part` 文件、
  丢失本次诊断的 WAV 尾部/stats 提交。`Drop`(`:1258-1262`)此时 sender 已 `None`,不再 join。
- **修复指令**:保留 writer 的 JoinHandle;`request_finish` 只置 `recording=false` 并入队 `Finish`,
  在 `Drop`/`run_with_options` 收尾处**join** writer 线程确保落盘;或提供显式 `await_finish()` 供
  `process_loop` 退出前调用。

### ROB-3 — RunState 锁 `unwrap()` 毒化 [Low]
- **位置**:`app/src-tauri/src/lib.rs:399, 468, 481`(`state.0.lock().unwrap()`)。
- **问题**:任一持锁线程 panic 后,`Mutex` 毒化,之后所有 `lock().unwrap()` 直接 panic,GUI 后端
  整体失能。
- **修复指令**:改用 `lock().map_err(|e| e.to_string())?` 或 `lock().unwrap_or_else(|e| e.into_inner())`
  做毒化恢复;命令统一返回 `Result<_, String>`(已是),不要 `unwrap`。

### ROB-4 — `.lines().flatten()` 静默吞行 [Low]
- **位置**:`app/src-tauri/src/lib.rs:435, 455`、`crates/echoless-cli/src/main.rs:2325-2328`。
- **问题**:`BufReader::lines().flatten()` 把读错误/非 UTF-8 行**静默丢弃**。子进程输出含非 UTF-8
  或瞬时读错误时,状态/日志行会无声丢失,难排障。
- **修复指令**:显式 `match line { Ok(l)=>…, Err(e)=> 记一条 warn 日志 }`,至少不静默。

---

## 7. 测试 / CI / 可审计性

### TEST-1 — CI 不构建/不测试 Tauri 后端 [Medium]
- **位置**:`app/src-tauri/Cargo.toml:9`(独立 `[workspace]`)、`.github/workflows/build.yml`
  的 `cargo test/clippy/build --workspace`。
- **问题**:`echoless-app` 是**独立 workspace**,根 `--workspace` 不会编译它。CI 的 test/clippy/
  build 三步**完全不覆盖** `app/src-tauri/src/lib.rs`(607 行、含全部 shell/IPC 安全面)。
  > **基线校准(commit 4d12c43)**:通读完整 `build.yml`(252 行)确认——CI **整个 GUI 都不构建**:
  > 无 `vite build`/`tsc`(前端)、无 `tauri build`、无任何 `app/src-tauri` 的 cargo 调用。
  > "Package" 步骤打的是 **CLI 发行**(`target/release/echoless` + LocalVQE 库 + 模型 + `example.toml`),
  > **不是桌面 App**。即 Tauri Rust 后端与 React 前端**双双不在任何 CI gate 内**(比原稿更严重)。
- **修复指令**:CI 增加 ① 针对 `app/src-tauri` 的独立 `cargo clippy -D warnings` + `cargo build`;
  ② 前端 `pnpm tsc --noEmit`(可加 `vite build` 冒烟);③ 若发布 GUI,补一条 `tauri build`(或至少
  在 CI 编译 Tauri 后端)。当前 Package 产物仅含 CLI,需在文档明确"该 artifact 不是 GUI 安装包"。

### TEST-2 — 缺依赖漏洞扫描 / fmt 检查 / 前端 lint [Medium]
- **位置**:`.github/workflows/build.yml`。
- **问题**:CI 只有 test/clippy/build,**无** `cargo audit`/`cargo deny`(依赖 CVE/license/重复版本)、
  **无** `cargo fmt --check`、**无**前端 `tsc --noEmit`/eslint/test。可审计性与供应链卫生不足。
- **修复指令**:新增 job:`cargo install cargo-audit && cargo audit`(或 `cargo-deny`)、
  `cargo fmt --all --check`、前端 `pnpm tsc --noEmit`(若配 lint 则加 eslint)。允许 audit 警告
  不阻断但需可见。

### TEST-3 — chain 重采样/声道适配、output_level 溢出无单测 [Low]
- **位置**:`chain.rs`(全文件无 `#[cfg(test)]`)、`core/lib.rs` output_level 已有测试但无溢出/边界。
- **修复指令**:为 `adapt`/`remap_channels`/`resample_linear` 加数值与长度单测(配合 QUAL-1
  改造后改为有状态版的连续性测试);为 `frame_size()` 加极端入参测试(配合 QUAL-2)。

### CFG-1 — CI 从 LocalVQE main 非固定克隆 [Low]
- **位置**:`.github/workflows/build.yml`(`git clone --depth 1 --recursive …/LocalVQE.git`)。
- **问题**:克隆上游 `main` 而非固定 commit,LocalVQE C API 回归构建**不可复现**,且上游一旦
  改动/失陷,CI 即受影响。
- **修复指令**:固定到具体 commit/tag(`git checkout <sha>`),并定期人工升级;或缓存已验证的源码 tarball。

### DOC-1 — nvafx pin 哈希散落、无轮换说明 [Low]
- **位置**:`crates/echoless-cli/src/main.rs:1801-1820`(硬编码 4 个 model + 1 个 common 的 SHA256)。
- **问题**:pin 哈希直接埋在 `match` 里,与版本号(`SDK_VERSION` 在 `nvafx.rs`)分散两处;升级
  runtime 版本时易漏改、无集中说明。
- **修复指令**:把 `{asset → sha256}` 集中为一张带版本注释的常量表(或随 release manifest),
  并在 `docs/` 记录"如何在发新 runtime 时更新 pin"的流程。

---

## 8. 修复优先级建议(给排期/sub-agent 认领)

**第一梯队(质量/稳定性,直接影响产品体验)**
1. PERF-1 实时热路径零分配化(配合 QUAL-3/QUAL-4)。
2. QUAL-1 有状态重采样(rubato)+ 立体声保留 + 连续性单测。
3. ROB-2 诊断 writer 正确 join(避免丢录音)。

**第二梯队(安全加固,低成本高收益)**
4. SEC-1 `open_url` scheme 校验 / 改 opener 插件。
5. SEC-2 nvafx 内置 pin 优先。
6. SEC-3 私有目录/唯一临时文件。
7. ROB-1 Tauri 长命令 `spawn_blocking`+超时。

**第三梯队(架构/可维护性,适合独立大改)**
8. ARCH-1 巨石文件机械拆分(零行为变化,逐步提交)。
9. ARCH-2 决断 core 抽象去留。
10. SEC-4/SEC-5 动态库/模型加载收紧。

**第四梯队(工程卫生)**
11. TEST-1/TEST-2 CI 覆盖 Tauri 后端 + audit/fmt/前端 lint。
12. 其余 Low(QUAL-2/5、ROB-3/4、SEC-6、CFG-1、DOC-1)。

---

## 9. 审计未覆盖 / 后续建议

- **`vendor/aec3`**:按约定为只读第三方,本次未审计其内部正确性/性能;但它是音质核心,建议
  单独安排一次"vendored fork 与上游 diff 审查"(确认仅开放了 `aec3_config` 注入口、无其它改动)。
- **依赖版本**:`Cargo.lock` 已锁定,但本次未跑 `cargo audit`(见 TEST-2);上线前应跑一次确认无已知 CVE
  (尤其 `zip`/`libloading`/`cpal`/`tauri` 生态)。
- **FFI 内存安全**:`nvafx.rs`/`localvqe.rs` 的 `unsafe` FFI 边界经人工核对(缓冲长度均在
  Rust 侧校验后才传指针,`unsafe impl Send` 在单处理线程使用下成立),未发现明显 UB;但属
  "信任外部 .dll/.so 契约"的代码,建议在 miri 之外补 ASAN 集成 smoke(需真实 runtime)。
- **前端**:`App.tsx`(1268 行)同为偏大单文件,`innerHTML` 仅用于 anime.js scramble 文本(值来自
  受控数字/标签,非用户输入),未见 XSS 注入面;但 `App.tsx` 体量建议后续按页面/hook 拆分(非阻塞)。

---

## 10. AEC3(vendored 引擎)审计

> 范围:`vendor/aec3`,52138 行 Rust / 7 个 crate(`aec3-core` 17k、`aec3-agc2` 9k、
> `aec3` facade 12.7k、`aec3-fft` 5k、`aec3-ns` 3.1k、`aec3-simd` 2.7k、`aec3-common-audio` 2.1k)。
> 它是 `dignifiedquire/aec3`(WebRTC AudioProcessing 的纯 Rust 移植,BSD-3-Clause)的 fork,
> edition 2024 / rust 1.91,作为独立子 workspace 经 path 依赖引入。**echoless 仅经 facade
> 调用,且固定 48 kHz / 10 ms(480 样本)/ near mono / far mono|stereo。**

### 总体结论(正面)
- **无可达生产 panic**:全仓 `panic!`×6 / `unreachable!`×3 经逐一核对,在 echoless 的固定
  48k/10ms 用法下**均不可达**:
  - `pffft.rs` 三处 `panic!("unsupported radix")` 与 `aec3_fft.rs` 两处 `unreachable!` 为内部
    FFT 不变量,48k 固定尺寸不触发。
  - `high_pass_filter.rs:71` `panic!("unsupported sample rate")` 仅在启用 HPF 且非 16/32/48k
    时触发;echoless 不启用 HPF 且固定 48k。
  - `splitting_filter.rs:208` `unreachable!()` 被紧邻 `assert!(num_bands==2||3)` 守住,48k 下
    `num_bands=3`。
  - `pitch_search*.rs` 的 `File::open(...).unwrap_or_else(panic)` 位于 **`#[cfg(test)]`** 模块,
    不进生产二进制。
- **`unsafe` 用法规范**:206 处 `unsafe` 集中在 `aec3-simd` / `aec3-fft` / `matched_filter`
  的 SIMD 内核,均为 ① `#[cfg(target_arch)]` 编译期门控 + ② 运行时 `detect_backend()` 特性检测
  后再 dispatch,且每处带 `SAFETY:` 注释说明前置条件(如 "detect_backend 只在确认 avx2+fma 后返回
  Avx2")。NEON 走 "aarch64 必带 NEON" 的成立前提。FFT 内核的越界省略均有 `SAFETY:` 索引边界论证。
  未发现明显 UB 模式。
- **fork 改动面极小且有据**:唯一对外新增是 `AudioProcessingBuilder::aec3_config()`
  (`vendor/aec3/crates/aec3-apm/src/audio_processing.rs:373`)+ `set_aec3_config_override`,在 `build()` 的
  `initialize_with_config` 前注入完整 `EchoCanceller3Config`(对应 research 文档"方案 A")。注入仅在
  构造期一次性发生,不污染运行时路径。
- **热路径分配**:`audio_processing_impl::process_stream` 主体抽样 **0** 处分配(符合 WebRTC
  预分配设计)。

### SON-1 — echoless 吞掉 aec3 的处理结果 Result [Low]
- **位置**:`crates/echoless-processors/src/aec3.rs:334, 339-342, 344-347`
  (`let _ = self.inner.apm.process_render_f32(...)` / `process_capture_f32(...)`)。
- **问题**:`process_*_f32` 返回 `Result<(), Error>`(可能为 `ChannelMismatch` /
  `InvalidSampleRate` / `InvalidChannelCount` / `StreamParameterClamped`)。echoless 全部 `let _ =`
  丢弃。正常固定形参下不会触发,但一旦上游 chain 适配出 bug 导致 buffer 形状/声道不符,错误被静默,
  输出变成未定义内容而无任何诊断信号。
- **修复指令**:至少在**首次**出现错误时把它记入 `ProcessorStats::last_backend_error`(已有该字段)
  并打一条日志,而非完全忽略。

### SON-2 — 热路径 `assert!`(release 也崩)与 `process_capture` 分配未经 profile [Low]
- **位置**:`vendor/aec3/crates/aec3-core/src/*`(`assert!`/`assert_eq!` 与 `debug_assert!` 混用)、
  `echo_remover.rs` `process_capture`(`:201` 起)。
- **问题**:
  1. WebRTC 上游用 `RTC_DCHECK`(仅 debug)。移植里 `assert!`/`assert_eq!`(**debug+release 都触发**)
     与 `debug_assert!` 并存。若某个**数值不变量类** `assert!` 落在实时处理路径,遇到 NaN/denormal/
     极端输入时会在 **release 下 panic → 进程崩溃**(对实时音频是致命的)。注:本次抽样命中的多在
     `#[cfg(test)]` 内,需逐一区分生产 vs 测试,**不可假定全部安全**。
  2. `echo_remover.rs` 的 `process_capture` 窗口内抽到约 10 处 `vec!`/`with_capacity` 类 token,
     未能在本次预算内确认是否稳态每帧分配。
- **修复指令**:
  1. 审查 `aec3-core`/`aec3-ns`/`aec3-agc2` 处理路径上的 `assert!`/`assert_eq!`,把"理论不变量
     断言"(非"调用方契约校验")改为 `debug_assert!`,避免 release 崩溃;契约类保留并让上层兜住。
  2. 用 `dhat`/计数 allocator 对 `process_render/capture` 跑稳态 profile,确认零分配(与 echoless
     侧 PERF-1 一致的实时要求)。

### SON-3 — aec3 不在 echoless CI lint/audit gate 内 [Low]
- **位置**:`vendor/aec3/Cargo.toml`(独立 `[workspace]`,edition 2024)、根 `Cargo.toml`
  `exclude = ["vendor/aec3"]`、`.github/workflows/build.yml`。
- **问题**:aec3 被根 workspace 排除,echoless CI 的 `clippy -D warnings` / 测试 / (建议新增的)
  `cargo audit` 都不覆盖它。它有自带 CI,但 echoless 仓库内对它无任何回归 gate;一旦本地对 fork 再
  改动(如继续开放更多 config 字段),无 CI 拦截。
- **修复指令**:CI 增加一步对 `vendor/aec3` 单独 `cargo test -p aec3-apm -p aec3-core`(至少 facade + core 关键
  测试)与 `cargo clippy`,确保 fork 改动不回归。

### 未在本次覆盖(aec3)
- **AGC2 / RNN-VAD 路径**(`aec3-agc2`,echoless `agc=true` 时启用)与**立体声 render**
  (`reference_channels="stereo"`)路径未做深度跑测;建议各补一条端到端 smoke。
- **数值一致性**:未与上游 C++ WebRTC 做逐样本对拍(aec3 自带 cpp-validation,但 echoless 侧未跑)。

---

## 11. 两个现场问题诊断(用户报告)

### RUNTIME-1 — 调音量报 `unknown runtime control command set_output_level` [High,已定位]

**现象**(截图 1):调到 VOL 52 时弹
`null: invalid runtime control JSON: unknown runtime control command 'set_output_level'; line={"cmd":"set_output_level","level":52}`。

> **基线校准(commit 4d12c43)**:`set_output_level` 正是在**本基线提交**(标题即
> "Stabilize desktop runtime controls")才加入源码,`realtime.rs:573` 现已正确处理。因此本问题
> 在**源码层已不存在**——从 4d12c43 重新构建 GUI 所用的二进制即消除报错。下面的"根因"描述的是
> **截图当时**那个早于本提交的陈旧二进制;留下的真正待办是"根治修复"里的**版本握手**(见下),
> 它在 HEAD 仍未实现(`started` 事件只在诊断 metadata 写 `version=`,无能力协商)。

**根因(已实证)**:截图当时 GUI 实际运行的 `echoless` CLI 是**过时的 debug 二进制**,它早于
`set_output_level` 运行时控制命令被加入。证据:
- 源码 `crates/echoless-cli/src/realtime.rs:573` 明确处理 `set_output_level`;`target/release/echoless`
  里 `strings` 命中该字符串 **2 次**。
- 但 `target/debug/echoless`(构建于较早时间)里命中 **0 次** → 该二进制无此命令。
- 截图 2 的 LocalVQE 报错路径全部是 `.../target/debug/...`,**证明 GUI 启动的就是 `target/debug/echoless`**
  (即用户以 `ECHOLESS_BIN` 指向 debug,或 dev 环境跑 debug)。
- 调用链:前端 `setOutputLevel`(`app/src/api.ts:156`)→ `send_run_control` 经 stdin 下发 →
  CLI `spawn_control_reader`→`parse_runtime_control_command`。旧 CLI 的 `parse_runtime_control_command`
  没有 `set_output_level` 分支,落到 `other => bail!("unknown runtime control command")`,再被
  `spawn_control_reader` 包成 `invalid runtime control JSON: ...` 经 stdout 回给前端 → 弹该 toast。

**立即修复(用户侧,无需改码)**:重新构建 GUI 实际运行的那个二进制:
```bash
# 若 ECHOLESS_BIN 指向 debug(截图显示如此):
cargo build -p echoless-cli                 # 刷新 target/debug/echoless
# 或改用最新 release 并让 GUI 用它:
cargo build -p echoless-cli --release        # 刷新 target/release/echoless(echoless_bin 默认回退)
# 确认:strings target/debug/echoless | grep -c set_output_level  → 应 >0
```

**根治修复(代码侧,建议 sub-agent 实施)**:
1. **GUI↔CLI 版本/能力握手**:`echoless` 启动 `run --status-json` 时,`started` 事件里已带不少字段,
   建议再加 `cli_version`(`env!("CARGO_PKG_VERSION")`)与/或 `supported_controls: ["start_diagnostics",
   "stop_diagnostics","set_output_level"]`;前端据此判断能力,缺失时给"CLI 版本过旧,请重建"的**明确**
   提示,而不是把它当成一次普通的 `control_error` toast。
2. **`control_error` 的前端呈现**:对 `unknown runtime control command` 这类应识别为"后端不支持该控制"
   而非偶发错误,避免误导。
3. **dev 工作流防呆**:`app/README.md` 与 `tauri dev` 脚本明确——`ECHOLESS_BIN` 指向 debug 时需先
   `cargo build -p echoless-cli`;或在 Tauri `beforeDevCommand` 里加一步构建 CLI,避免 GUI 跑到陈旧二进制。

### RUNTIME-2 — 启用 LocalVQE 报 `localvqe library could not be loaded` [High,已定位]

**现象**(截图 2):引擎卡片显示 `LOCALVQE ● ACTIVE`、模型 v1.3/v1.2/v1.1 标 `OK`/`DEFAULT`,但启动后报
`Error: localvqe library could not be loaded; tried: .../target/debug/liblocalvqe.dylib | .../localvqe.dylib | .../app/src-tauri/...`(遍历多路径全部失败)。

**根因(已定位)**:**LocalVQE 的原生运行库 `liblocalvqe.dylib` 根本不存在**,而 echoless 没有任何途径
去获取它。具体:
- **库与模型是两个独立产物**。`download_localvqe_model`(`app/src-tauri/src/lib.rs:234`)只下载 **`.gguf`
  模型**(截图里 v1.3/v1.2/v1.1 就是这些),而真正的**原生库**(ggml 之上的 C ABI 封装
  `liblocalvqe.dylib`)需按 `docs/localvqe_inference.md` 的 `cmake … --target localvqe_shared`
  **从 LocalVQE 源码编译**。
  > **基线校准(2026-06-10,commit 4d12c43)**:此处原稿曾断言"echoless 既不下载、也不打包、也无
  > 构建步骤产出该库",**该结论不准确,已更正**。事实是:CI(`.github/workflows/build.yml`)的
  > "LocalVQE C API regression" 步骤会 `cmake --build … --target localvqe_shared` **构建**该库,
  > "Package" 步骤会把 `liblocalvqe*.dylib` + `libggml-*.{so,dylib}` **复制进 `dist/`** 一并发行。
  > 因此**打包发行版本是带原生库的**。用户截图的失败发生在 **dev 运行**(GUI 启动 `target/debug/echoless`,
  > 见 RUNTIME-1),dev 路径**不经过打包步骤**,故那里没有库——这才是现场报错的直接原因。
  - 因此真正待补的是:① **dev 工作流**无库(需手动构建 + `ECHOLESS_LOCALVQE_LIBRARY`);
    ② GUI 即便在打包态也**不会注入** `ECHOLESS_LOCALVQE_LIBRARY` 指向打包库(对比已注入的
    `ECHOLESS_PROCESS_TAP_HELPER`);③ 搜索路径窄(见下)。对比:nvafx 有 `download-install`
    在终端用户机上取 runtime;LocalVQE 无等价的"用户机自助获取"路径(只有 CI 端打包)。
- **搜索路径过窄**。`library_candidates`(`crates/echoless-processors/src/localvqe.rs:483-512`)只查
  ① 可执行文件目录及其 `localvqe/` 子目录、② cwd 及其 `localvqe/` 子目录、③ `ECHOLESS_LOCALVQE_LIBRARY`。
  **不含**模型实际所在的 `<app_local_data>/localvqe/models/`,也**不含** Tauri 打包资源目录。即便用户把
  dylib 放到模型旁边也找不到。
- **还需 ggml 后端库**:据 `docs/localvqe_inference.md`,macOS 上仅有 `*.dylib` 不够,必须把
  `libggml-cpu-*.so`/`libggml-metal.so`/`libggml-blas.so` 一并放在库旁,否则运行时
  `backend 'CPU' not registered`。
- **UI 误导**:卡片在**未验证库可加载**前就显示 `● ACTIVE` 且模型标 `OK`,让用户以为已就绪,实际启动才失败。

**立即修复(用户侧)**:按 `docs/localvqe_inference.md` 构建库,并指给 echoless:
```bash
git clone --recursive https://github.com/localai-org/LocalVQE.git
cmake -S LocalVQE/ggml -B build -DCMAKE_BUILD_TYPE=Release -DLOCALVQE_BUILD_SHARED=ON
cmake --build build --config Release --target localvqe_shared
# 把 liblocalvqe.dylib 与全部 libggml-*.{so,dylib} 放到 echoless 二进制同目录,或:
export ECHOLESS_LOCALVQE_LIBRARY=/abs/path/to/liblocalvqe.dylib
# 同目录需同时有 ggml 后端库,否则报 backend 'CPU' not registered
```
（注意:截图里 GUI 跑的是 `target/debug/echoless`,库要放到 `target/debug/` 旁,或用绝对路径 env。）

**根治修复(代码侧,建议 sub-agent 实施,按优先级)**:
1. **打包/获取原生库**(主因):
   - 方案 A(推荐):像 nvafx 一样,把 `liblocalvqe.*` + `libggml-*` 作为 Tauri **externalBin/resource
     打包**,并在 `echoless_command()` 里注入 `ECHOLESS_LOCALVQE_LIBRARY`(参照已有的
     `ECHOLESS_PROCESS_TAP_HELPER` 注入,`app/src-tauri/src/lib.rs:76-82`)。
   - 方案 B:新增 `localvqe download-install` 子命令(类比 nvafx),按平台拉取预编译库+后端,校验 SHA256。
2. **拓宽库搜索路径**:`library_candidates` 增加 `<app_local_data>/localvqe/` 与 Tauri 打包资源目录;
   让"库与模型同处一目录"能被发现。
3. **就绪态门控 UI**:卡片的 `ACTIVE`/模型 `OK` 应在**实际能加载库**后才点亮。可在前端启用前先跑一次
   轻量探针(如 `echoless` 新增 `localvqe doctor --json` 返回库/后端是否就绪),失败则给"缺少原生库,
   点此安装/查看说明"的引导,而不是直接进 run 才暴雷。
4. **错误信息可操作化**:`localvqe.rs:104-113` 的 `bail!` 文案补一行"如何获取库"(指向 docs 或安装入口),
   而非只列尝试过的路径。

---

## 12. 基线复核记录(commit 4d12c43,2026-06-10)

> 用户将工作区提交为 `4d12c43`「Stabilize desktop runtime controls and mac reference path」并以此为基线
> 要求复核。该提交相对前一提交 `9a53043` 大改了 `main.rs`(+1800)、`realtime.rs`(+2143)、
> `core/lib.rs`、`chain.rs`、Tauri `lib.rs`(+40)等——即本审计正是针对**这套已稳定化的代码**所做。
> 下表把每条可机检的 finding 重新比对 HEAD,标注其有效性。

| ID | 复核结论 vs HEAD(4d12c43) |
|----|----|
| PERF-1 | **仍成立**。`chain.rs:73` `to_vec()` + `:92` `vec![0f32;…]` + `adapt` 内多次分配照旧 |
| QUAL-1 | **仍成立**。`chain.rs:138` 注释仍标"占位;TODO rubato",`rubato` 仍未被引用 |
| QUAL-2 | **仍成立**。`core/lib.rs:141` 仍 `self.sample_rate * self.frame_ms`(u32 相乘) |
| ARCH-2 | **仍成立**。`core/lib.rs:156` `ControlApi` 与 `:288/:300` `run_realtime` bail 桩仍在 |
| SEC-1 | **仍成立**。`open_url`(lib.rs:336)仍无 scheme 校验,仍 `cmd /C start` |
| SEC-2 | **仍成立**。`main.rs:1636` 仍 `release_sha256sums.get().or_else(内置 pin)`(下载优先) |
| SEC-3 | **仍成立**。lib.rs:381/407 仍固定 `echoless-validate.toml` / `echoless-run.toml` |
| ROB-1 | **仍成立**。`nvafx_install`/`nvafx_download_install`/`validate_config` 仍同步;仅 `probe_delay` 用 `spawn_blocking` |
| SON-1 | **仍成立**。`aec3.rs:334/339/344` 仍 `let _ =` 吞 process 结果 |
| RUNTIME-2 | **部分更正**(见 §11 校准框):库**确由 CI 构建+打包进发行版**;现场失败是 dev(`target/debug`)无库。其余 gap 仍在 |
| RUNTIME-1 | **更正语境**(见 §11 校准框):`set_output_level` 已在本提交入源码,重建即修;遗留的版本握手仍未做 |
| TEST-1 | **更正且加重**:CI 不仅漏 Tauri 后端,**整个 GUI(前端+Tauri)都不构建**;Package 只打 CLI |
| TEST-2 | **仍成立**。`build.yml` 无 `cargo audit`/`cargo deny`/`fmt --check`/前端 lint |
| 其余 Low(QUAL-3/4/5、ROB-2/3/4、SEC-4/5/6、CFG-1、DOC-1、SON-2/3) | 相关文件未在本提交改动或改动不触及,结论**保持**;`localvqe.rs`(RUNTIME-2 搜索路径、SEC-4)整文件不在本次 diff 内 |

**复核结论**:除 **RUNTIME-2 的一句事实性表述**(库的打包/构建情况)需更正、**RUNTIME-1 与 TEST-1**
需补基线语境外,其余 finding 经逐条比对 HEAD **均仍有效、行号未漂移**。没有发现因本提交而被
"悄悄修掉"却仍标为未决的条目;新增的大段 `realtime.rs`/`main.rs` 代码已在初版审计中通读覆盖,未发现
本次新引入、此前漏报的高危问题(前端 `VolumeWheel`/`AdvancedPage` 等新 UI 仅做了安全面 grep,
未深审,属"未覆盖"而非"过期")。

---

## 13. 前端 / 架构耦合 / 打包 / 延迟(2026-06-10 追加,基线 4d12c43)

> 回答四个定向问题:① 前端 bug/性能;② 前后端耦合是否最优;③ 打包后 CLI 是否可用;④ 首页侦测延迟是否正确。

### PKG-1 — 打包后 GUI 找不到 CLI(及模型资源)[High]
- **位置**:`app/src-tauri/tauri.conf.json`(`bundle` 段)、`app/src-tauri/src/lib.rs:33-41`(`echoless_bin`)、
  `:43-74`(`process_tap_helper_bin`)、`:220-225`(`resolve("resources/localvqe/models", Resource)`)。
- **问题(Q3 答案:当前打包后 CLI 不可用)**:
  1. `tauri.conf.json` 的 `bundle` 只有 `active/targets/icon`,**没有 `externalBin`**(CLI 不进包)、
     **没有 `resources`**(模型/库不进包)。
  2. `echoless_bin()` 回退路径用 **编译期常量** `env!("CARGO_MANIFEST_DIR")` 拼 `../../target/release/echoless`。
     该常量是**构建机**的绝对路径,在终端用户机上**不存在** → 回退必然失败;打包态唯一可行的
     `ECHOLESS_BIN` env **打包流程并未注入**(README「已知 TODO: sidecar 打包」自承)。
     `process_tap_helper_bin()` 用 `CARGO_MANIFEST_DIR` 的 ancestors 同理失效。
  3. `localvqe_assets`(lib.rs:220)按 `BaseDirectory::Resource` 找 `resources/localvqe/models`,但
     `bundle` 无对应 `resources` 项 → 打包后该目录为空,即便有也无 LocalVQE 原生库(见 RUNTIME-2)。
  4. CI 不跑 `tauri build`(TEST-1),此路径从未被验证。
- **结论**:**未做 sidecar 打包前,打包出的 GUI 对每个命令都会 `spawn echoless failed`**;现状只有
  dev(`ECHOLESS_BIN` 或 `target/release/echoless` 存在)能跑。
- **修复指令**:
  1. `tauri.conf.json` 加 `bundle.externalBin: ["binaries/echoless"]`(配 `target-triple` 后缀),把 CLI
     作为 sidecar 打包;`echoless_command()` 改用 Tauri sidecar API 或 `app.path().resource_dir()` 解析,
     **不要**依赖 `CARGO_MANIFEST_DIR`。
  2. 同理把 LocalVQE 原生库 + ggml 后端、Process Tap helper 作为 `externalBin`/`resources` 打包,并
     在 `echoless_command()` 注入 `ECHOLESS_LOCALVQE_LIBRARY` / `ECHOLESS_PROCESS_TAP_HELPER`
     (后者已注入,前者缺,见 RUNTIME-2)。
  3. `bundle.resources` 加 `resources/localvqe/models/*.gguf`(默认模型)。
  4. CI 增加 `tauri build` 冒烟(配合 TEST-1)。
- **附注(正面)**:`app/src-tauri/Info.plist` 已含 `NSMicrophoneUsageDescription` 与
  `NSAudioCaptureUsageDescription`(打包 mac app 访问麦克风/系统音频所必需)——**已就绪**;
  仅需确认 Tauri v2 确实把 `src-tauri/Info.plist` 合并进 `.app`(v2 默认合并,建议打一次包验证)。

### LAT-1 — 首页 LATENCY 低估真实嘴到耳延迟 [Medium]
- **位置**:显示 `app/src/App.tsx:312, 873`(`live.lat = s.estimated_user_latency_ms`);
  计算 `crates/echoless-cli/src/realtime.rs:1582-1593`(`estimate_user_latency_ms`)。
- **问题(Q4 答案:数值方向对、但系统性偏低)**:后端公式 =
  `frame_ms/2 + near_delay_ms + algorithmic_latency_ms + output_queue_latency_ms(out_q)`。
  它**只覆盖**"帧累积 + near 对齐延迟 + 处理器算法延迟 + 输出 ring 排队",**漏掉**了:
  - **输入侧 ring 排队**(`mic_q`,status 里有上报 `mic_q_samples` 却未计入);
  - **cpal 设备硬件缓冲**(输入 + 输出回调缓冲,常 10–40ms);
  - **设备边界 I/O 重采样**的缓冲(设备率 ≠ 48k 时,`InterleavedLinearResampler`/`OutputLinearResampler`)。
  因此首页"LATENCY xx ms"**低于**用户实际感知的单向附加延迟。对 Discord/VRChat 这类在意端到端
  延迟的场景,标成裸"LATENCY"易被读成总延迟。
- **修复指令**:
  1. 把 `mic_q` 排队折算计入(`output_queue_latency_ms(mic_q)` 对称项);
  2. 若能从 cpal 取设备 buffer size,补一项设备缓冲估计;否则在 tooltip/文案标注"不含设备硬件缓冲";
  3. 或把标签改为"PIPELINE LATENCY"以诚实表达它是管线内估计,而非完整嘴到耳。
- **注**:AdvancedPage 的**近端延迟侦测**(probe)是另一回事——它测 mic↔ref 相对到达差并推荐
  `near_delay_ms`,逻辑与后端 `recommended_near_delay_ms` 一致,本次未发现错误;它推荐的 near_delay
  会**增大**首页 LATENCY(已在公式内,自洽)。

### ARCH-3 — 前后端耦合:shell CLI + 临时文件 + 改配置即重启(非最优)[Medium]
- **位置**:`app/src-tauri/src/lib.rs`(全部命令 shell `echoless`)、`App.tsx:593` `applyChange`
  (stop+start)、`:513` `currentToml`、`validate_config`/`start_run` 经固定临时 TOML。
- **问题(Q2 答案)**:当前耦合方式 = **Tauri 后端把 echoless CLI 当 sidecar,每个操作 spawn 一个
  子进程,配置经临时 TOML 文件传递,run 的状态/控制经 stdout/stdin JSONL**。其代价:
  1. **改配置即重启**:`applyChange` 对设备切换、NS、参考源、采样率、引擎切换……一律
     `stopRun()+validateConfig()(spawn CLI)+startRun()(spawn CLI)` → **每次调档音频断一下**
     (见 FE-3)。只有**音量**(`set_output_level`)与**录制**走 stdin 热控,不重启。
  2. **进程/IO 开销**:每个一次性查询(devices/doctor/validate)都 spawn 进程;`validate` 每次改动都跑。
  3. **共享层被旁路**:`echoless-core` 本设计为"平台无关编排 + ControlApi"共享层(CLI 与 GUI 同源),
     但实际 GUI 不链 core、只 shell CLI,core 的 `run_realtime`/`ControlApi` 是死代码(见 ARCH-2)。
  4. 伴生问题:固定临时文件名(SEC-3)、无版本握手(RUNTIME-1)。
- **是否最优**:**作为 MVP 可接受,但不是最优**。它换来了进程隔离(音频崩溃不拖垮 GUI)与 CLI 可独测,
  代价是热路径笨重 + 改配置抖动 + 版本/临时文件脆弱。
- **修复方向(二选一,按目标取舍)**:
  - **(A) 进程内直链 `echoless-core`**:Tauri 后端直接以 Rust crate 方式链入 core,音频在 Tauri 进程内
    线程跑,免子进程/JSON 往返/临时文件/版本skew;落地 ARCH-2 的方案 B(把 `process_loop`/运行时控制
    下沉为 `ControlApi`)。代价:音频崩溃会拖累 GUI(需 panic 隔离)。
  - **(B) 保留子进程但补热重配**:让 run 子进程支持经 stdin **热切换** chain/设备/参考(不重启),
    把 `applyChange` 从 stop+start 改为下发控制命令;并加版本握手(RUNTIME-1)+ 唯一临时文件/
    stdin 直传配置(SEC-3)。代价:run 循环需支持就地重建 chain(目前不支持)。
  - 推荐:近期做 (B) 的"热重配 + 握手 + 临时文件加固"消除最痛的调档抖动;(A) 列为架构演进。

### FE-1 — 4 个 rAF 循环常驻 + 每帧强制重排 [Medium]
- **位置**:`app/src/components/Scope.tsx`(`Scope` 的 `frame` rAF,3 个实例)、`FooterBars` rAF。
- **问题(Q1 性能)**:
  1. 3 个 `Scope` + 1 个 `FooterBars` 各跑一个 `requestAnimationFrame` 循环,`draw()`/bar 更新
     **每帧无条件执行**;即使 `tel.on=false`(停机/空闲)也持续 60fps 重绘(只是低幅)。桌面 webview
     最小化时不一定节流 → **空闲也持续吃 CPU/GPU/电**。
  2. 每个 `Scope.draw()` 每帧调 `canvas.getBoundingClientRect()`(3 实例 × 60fps = **180 次/秒强制
     同步布局**),属 layout thrash。
  3. 后端真实 `wave` 每 80ms 才更新一次,但 rAF 用同一数组 60fps 重绘**完全相同**的曲线(载波相位
     固定无时间项)→ 大量重复绘制。
- **修复指令**:
  1. `tel.on=false` 或 `prefers-reduced-motion` 时**暂停 rAF**(或降到极低频),并监听窗口
     `visibilitychange` 隐藏时停。
  2. 用 `ResizeObserver` 缓存尺寸,`draw()` 不再每帧 `getBoundingClientRect`。
  3. 真实波形模式下仅在 wave 更新时重绘(或对 60fps 做插值动画,避免重复绘相同帧)。

### FE-2 — 每 80ms status 触发整个 App 重渲染 [Low]
- **位置**:`App.tsx:308-329`(status 事件里 `setLive`/`setHealth`)。
- **问题**:status 每 80ms(12.5/s)更新 `live`/`health` → 触发 `App`(1268 行)整树重渲染。`Scope`
  因依赖稳定 `telRef` 不重渲(好),但 `Dropdown`/各 row/当前页面组件每 80ms 重渲一次。
- **修复指令**:把 `live`/`health` 下沉到独立子组件(或 context),只让数字表/health 行重渲;对
  `EnginePage`/`AdvancedPage` 等做 `React.memo`,避免 status tick 波及。

### FE-3 — 改配置即音频断点(随 ARCH-3)[Medium]
- **位置**:`App.tsx:593`(`applyChange` = stop+start)、被 `changeKind`/`setParam`/`changePipeline`/
  设备下拉 onChange 调用。
- **问题(Q1 bug/体验)**:除音量/录制外,任一改动都重启 run 子进程,**音频中断一下**(stop 杀进程
  → 重新枚举设备 → 重开流)。在通话中调 NS/参考/采样率都会"咔"一下。
- **修复指令**:见 ARCH-3 方案 (B)——把可热改的项(NS、output 设备、参考源、reference_channels)
  改为 stdin 热控,不重启;确需重启的(sample_rate/frame_ms/引擎)再 stop+start 并给 UI 明确"重启中"。

### 其他前端观察(Low / 非阻塞)
- `App.tsx:660` `changeOutVolume`:本地 `output_level` 立即更新、再 `setOutputLevel`(可能失败弹错)。
  这正是 **RUNTIME-1** 截图里"VOL 52 已变但后端报 unknown command"的来源——属乐观更新,
  旧 CLI 下会显示已调但实际未生效。修 RUNTIME-1(重建/握手)后即一致。
- `AdvancedPage` probe 进行中若组件卸载,`probeDelay` promise 仍会 `setProbe/setProbing`(React18
  下无害告警);可加 mounted 标志。
- `App.tsx` 全文件 1268 行,与 `realtime.rs`/`main.rs` 同属偏大单文件(ARCH-1 同类),建议按
  overview/controls/signal/hooks 拆分(非阻塞)。

---

## 14. 总体修复策略与工作流程(给持续运行的 agent)

> 本节把全文 §1–§13 的**所有** finding 编排成一条有序、可持续执行、每步可溯源的工作流,并先给出
> "重构还是重写"的战略判定。基线 commit = `4d12c43`。

### 14.0 战略判定:**不重写,做"测试护栏下的增量重构 + 两处定向结构改造"**

- **结论**:**禁止 clean-room 重写**。本仓库**架构基本健全**——crate 职责分层清晰(audio-io / processors /
  core / cli)、`EchoProcessor` trait + `ProcessorChain` 抽象合理、FFI 边界规范、vendored aec3 经核
  **无可达 panic / 无明显 UB**、集成面干净。问题是**局部债**而非结构性腐败:几处实时热路径低效、两个
  巨石文件、打包未完成、一个前后端耦合取舍。
- **理由(对照 §0 威胁模型与 §10 结论)**:重写会丢弃已**验证正确**的 DSP 集成(aec3/AEC3/LocalVQE/
  nvafx FFI)并重新引入风险,收益为负。正确路径 = ① 先建 CI 护栏;② 增量修 finding;③ 仅对
  **ARCH-1(拆巨石,纯机械)**与 **ARCH-3(耦合)** 做两处刻意的结构改造,其余皆为定点修复。
- **唯一需"决策"的项**:ARCH-3(进程内直链 core,还是保留子进程+热重配)。在 Phase 6 前由人类拍板;
  在此之前所有修复都与该决策**解耦**(不预设结论)。

### 14.1 工程纪律(每个 agent 步骤都必须遵守)

1. **分支**:从 `main`(基线 4d12c43)切阶段分支,命名 `phase-<N>/<slug>`,如 `phase-1/packaging`、
   `phase-3/realtime-quality`。高风险或大改(ARCH-1/ARCH-3)单独分支;独立无依赖的轨道(如前端 Phase 5
   与安全 Phase 4)可并行分支(必要时用 git worktree 隔离),最后各自合并。
2. **提交粒度**:**一个 finding = 一个或少数原子提交**。提交信息**必须带 finding ID**,格式:
   `<type>(<ID>): <简述>`,例:`fix(SEC-1): validate http(s) scheme in open_url`、
   `refactor(ARCH-1): extract realtime/diagnostics.rs (no behavior change)`。→ 保证**可溯源**:任一提交
   都能回指本文档某条。
3. **测试门(每次提交前必须全绿,red 不提交)**:
   ```
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace --locked
   # Phase 0 落地后追加:
   (cd app/src-tauri && cargo clippy --all-targets -- -D warnings && cargo build)
   (cd app && pnpm tsc --noEmit)
   ```
4. **行为变更先写测试(TDD)**:凡改行为的 finding(PERF-1/QUAL-1/QUAL-2/SEC-2/LAT-1/RUNTIME-1
   握手/配置校验等),**先写一个会失败的测试**复现问题 → 看它红 → 实现修复 → 看它绿。**纯机械重构**
   (ARCH-1)不写新测试,以**既有测试全绿**为安全网,且"移动代码"与"改逻辑"**绝不同提交**。
5. **合并**:阶段分支跑完全部测试门 + 自检(大改建议 `adversarial-review` / `/code-review`)后合并回 `main`;
   合并提交注明本阶段关闭了哪些 ID。
6. **回滚**:出现回归 → `git revert` 对应**原子提交**(这就是为何提交要原子 + 带 ID)。
7. **可溯源台账**:在 `docs/audit/PROGRESS.md` 维护一张"finding ID → 状态(todo/doing/done)→ 关闭提交 SHA →
   验证方式"的表;每关掉一条就更新一行。**禁止**只改代码不更新台账。

### 14.2 分阶段工作流(先做什么、再做什么)

> 顺序原则:**先建护栏 → 再让产品端到端能跑 → 再拆巨石(降低后续改动成本)→ 再修实时音质 →
> 再做安全/健壮性 → 再修前端/延迟 → 最后做架构演进 → 收尾**。前一阶段是后一阶段的前置。

| 阶段 | 目标 | 包含 finding | 分支 | 完成判据(DoD) |
|---|---|---|---|---|
| **P0 安全网** | 没有护栏不改任何代码 | TEST-1, TEST-2, CFG-1 | `phase-0/ci` | CI 新增:Tauri 后端 clippy+build、前端 `tsc --noEmit`、`tauri build` 冒烟、`cargo fmt --check`、`cargo audit`/`deny`;LocalVQE 克隆固定到具体 SHA;全绿。建 `PROGRESS.md` |
| **P1 端到端可用** | 让 GUI 真能跑通(含打包) | RUNTIME-1, RUNTIME-2, PKG-1 | `phase-1/usable` | ① `started` 事件带 `cli_version`+`supported_controls`,前端对旧 CLI 给明确提示;② `tauri.conf` 配 `externalBin`(CLI)+`resources`(模型)+ LocalVQE 库/ggml/Process Tap helper 打包并注入 env;③ `echoless_command` 不再用 `CARGO_MANIFEST_DIR`;④ **实测打 macOS+Windows 包并端到端跑通**(devices/run/音量/录制/probe) |
| **P2 拆巨石** | 降低后续改动成本(纯机械) | ARCH-1 | `phase-2/split` | `realtime.rs`→devices/resample/diagnostics/stats/control/mod;`main.rs`→cli/config_validate/nvafx_install/probe_delay;每次抽取提交保持 `cargo test` 全绿;**零行为变化** |
| **P3 实时音质/稳定** | 核心产品质量 | PERF-1, QUAL-1, QUAL-3, QUAL-4, ROB-2, SON-1 | `phase-3/realtime` | chain.rs 稳态**零分配**(加计数 allocator 测试)+ rubato **有状态**重采样 + 立体声保留(加块边界连续性测试);LocalVqe 用 VecDeque;stats 在线分桶;诊断 writer 正确 join;aec3 process 结果不再吞 |
| **P4 安全/健壮** | 加固(多为小改,可批量) | SEC-1, SEC-2, SEC-3, SEC-4, SEC-5, SEC-6, ROB-1, ROB-3, ROB-4, QUAL-2, SON-2, SON-3, DOC-1 | `phase-4/harden` | open_url 校验 scheme;nvafx 内置 pin 优先;唯一/私有临时文件(O_EXCL);localvqe 库不搜 CWD;模型下载校验 SHA;Tauri 长命令 `spawn_blocking`+超时;锁去毒化;`flatten` 不吞行;frame_size 用 u64;aec3 热路径 `assert!`→`debug_assert!` 审查 + 进 CI;pin 哈希集中 |
| **P5 前端/延迟** | 前端性能与延迟正确性 | FE-1, FE-2, LAT-1, QUAL-5(前端重复) | `phase-5/frontend` | rAF 停机/隐藏时暂停 + ResizeObserver 缓存尺寸 + 真实波形仅更新时重绘;live/health 下沉子组件 + 关键页 `React.memo`;LATENCY 计入 mic_q/设备缓冲或改标 PIPELINE LATENCY(加单测) |
| **P6 架构演进** | 解决耦合(需人类决策) | ARCH-2, ARCH-3, FE-3 | `phase-6/arch` | 先由人类在 (A) 进程内直链 core / (B) 子进程+stdin 热重配 中拍板;落地后:改配置不再无脑 stop+start(可热改项走热控),`echoless-core` 的死抽象按决策删除或填实 |
| **P7 收尾** | 去重/补测/清理 | TEST-3, 其余 QUAL-5, 文档 | `phase-7/cleanup` | chain 重采样/output_level 边界补单测;合并重复的 `copy_into`/`rms_*`/重采样器;`PROGRESS.md` 全部 done;README 打包/运行说明更新 |

**依赖要点**:P0 必须最先(否则后续无护栏);P2 在 P3/P4 之前(否则改动落在巨石文件里、且本文档行号
失配——拆分后**按函数名/符号**重新定位即可);P1 可与 P2 并行(不同文件);P6 必须在 P0–P5 稳定后,
因为它会大改 `applyChange`/耦合面,需前面阶段的测试与端到端基线兜底。

### 14.3 单个 finding 的"完成定义"(通用模板,逐条套用)

每关一条 finding,依次:① 切/在阶段分支;② (行为类)先写失败测试;③ 按本文档"修复指令"实现,
**仅按符号重新定位**(行号可能因 P2 漂移);④ 跑满 §14.1 测试门;⑤ 原子提交(信息带 ID);
⑥ 更新 `PROGRESS.md`(状态=done + 提交 SHA + 验证方式);⑦ 若引入新行为,在本文档对应条目补一行
"已修:见 <SHA>"。**任一步未过 → 不前进到下一条**。

### 14.4 全局完成判据(Definition of Done)

全部满足才算目标达成:
1. §1 汇总表全部 finding 在 `PROGRESS.md` 标 done 并附关闭 SHA;
2. CI 全绿且已包含:`fmt --check`、`clippy -D warnings`(含 `app/src-tauri`)、`cargo test --workspace`、
   `cargo audit`、前端 `tsc --noEmit`、`tauri build` 冒烟、固定版 LocalVQE 回归;
3. **实测**:macOS 与 Windows 各打一次包,GUI 端到端跑通(设备枚举 / run / 音量热调 / 录制 / probe /
   LocalVQE 启用),无 `spawn echoless failed`、无 `unknown command`、无库缺失报错;
4. 实时路径:稳态零分配测试通过、重采样块边界连续性测试通过;
5. 无 `clippy`/`fmt` 警告,无新引入的 `unwrap` 毒化点 / 静默吞错。

### 14.5 给持续运行 agent 的执行循环(伪码)

```
读 PROGRESS.md(无则按 §14.2 生成,全部 todo)
按 P0→P7 顺序、阶段内按表内 ID 顺序:
  for finding in 当前阶段未完成项:
    git checkout -b/切到 phase-<N>/...           # 若分支未建
    if 行为类: 写失败测试 → 确认红
    按 finding "修复指令" 实现(符号定位,非行号)
    跑 §14.1 测试门
    if 全绿: 原子提交(带 ID) → 更新 PROGRESS.md(done+SHA)
    else:    修到绿;反复失败则记录阻塞原因,继续下一条,勿跳过测试门
  阶段全部 done → 跑整套测试门 → (可选)adversarial-review → 合并回 main → 标注阶段关闭的 ID
直到 §14.4 全部满足 → 报告完成
```

**红线**:① 任何提交前测试门必须全绿;② 机械重构与逻辑修改不混提交;③ 每条改动可经提交 ID 回溯到本
文档;④ ARCH-3 的方向未经人类确认前不动耦合面;⑤ 打包/外部下载等不可逆或外发动作按 §(全局约定)先确认。

---

## 15. 执行前最终确认(2026-06-10,交付 agent 前必读)

> 在把本文档交给 agent 执行前做的实测确认。**有一个会立即卡住工作流的阻塞项(P0.0),必须最先修。**

### 15.1 ⛔ 阻塞:基线过不了自己的 `clippy -D warnings` 门(新增步骤 P0.0)
- **事实**:基线 `4d12c43` **能编译**(`cargo check --workspace` 1.5s 通过),但
  `cargo clippy --workspace --all-targets --locked -- -D warnings`(= CI 与 §14.1 每次提交都要过的门)
  在 rust 1.96.0 下**失败,共 13 个 root 错误 + 2 个 Tauri 错误**。
- **影响**:§14.1 测试门与 §14.2 P0 的 DoD("CI 绿")**在基线上无法满足**;agent 第一次提交前跑门就红,
  会困惑/卡死。且 **`main` 的 CI 此刻极可能正红**(CI 跑的就是这条 clippy;Tauri 那 2 个因 app 是独立
  workspace、CI 未覆盖而漏网——见 TEST-1)。
- **新增步骤 P0.0(排在 P0 最前,独立小提交)——把基线刷绿**。完整清单:

  Root workspace(`cargo clippy --workspace --all-targets`):
  | # | 位置 | lint | 修法 |
  |---|------|------|------|
  | 1 | `processors/src/aec3.rs:61` | items_after_test_module | 把 `mod tests` 移到文件末尾(其后的 `Aec3Engine` 等生产代码挪到 test 模块之前) |
  | 2 | `cli/src/realtime.rs:708` | too_many_arguments(10) | `handle_runtime_controls` 入参打包成 struct |
  | 3 | `cli/src/realtime.rs:729` | option_as_ref_deref | 改 `stats.as_deref_mut()` |
  | 4 | `cli/src/realtime.rs:750` | too_many_arguments(10) | `handle_runtime_control_command` 入参打包 |
  | 5 | `cli/src/realtime.rs:1039` | large_enum_variant | `DiagnosticCommand::Frame(Box<DiagnosticFrame>)` 装箱大变体 |
  | 6 | `cli/src/realtime.rs:1104` | too_many_arguments(8) | `DiagnosticRecorder::new` 入参打包 |
  | 7 | `cli/src/realtime.rs:1381` | nonminimal_bool | 简化 `write_frame` 末尾布尔表达式 |
  | 8 | `cli/src/realtime.rs:1492` | too_many_arguments(8) | `write_diagnostic_metadata` 入参打包 |
  | 9 | `cli/src/realtime.rs:1658` | too_many_arguments(10) | `RealtimeStats::new` 入参打包 |
  | 10 | `cli/src/realtime.rs:2047` | clone_on_copy | `SupportedStreamConfigRange` 是 `Copy`,去掉 `.clone()` |
  | 11–13 | (clippy 还报 13 个,余下在上述文件内,以 `cargo clippy` 实际输出为准) | | |

  Tauri(`cd app/src-tauri && cargo clippy --all-targets`):
  | # | 位置 | lint | 修法 |
  |---|------|------|------|
  | 14 | `app/src-tauri/src/lib.rs:435` | lines_filter_map_ok(`flatten()` on `Result`) | = **ROB-4**;改 `.map_while(Result::ok)` 或显式 match |
  | 15 | `app/src-tauri/src/lib.rs:455` | 同上 | 同上 |

- **交叉关联**:
  - #2/#4/#6/#8/#9 的 `too_many_arguments` 是 **ARCH-1**(巨石文件、分解差)的征兆——P0.0 先用 param-struct
    消门错,P2 拆分时再彻底归位,二者一致不冲突。
  - #14/#15 就是 **ROB-4** → 它不只是健壮性 nit,而是**会让 Tauri 编译失败的 clippy 错误**,优先级上升到 P0.0。
- **P0.0 DoD**:root 与 app/src-tauri **两个** workspace 的 `cargo clippy --all-targets -- -D warnings` 全绿;
  `cargo test --workspace` 全绿(agent 进 P0.0 第一件事就是先跑一遍 `cargo test --workspace` 记录基线)。
  完成后 CI(P0 落地后)才可能绿。

### 15.2 其余确认(非阻塞,已澄清/已纳入)
- ✅ **无可达生产 panic**:首方 Rust 唯一的 `panic!`(`realtime.rs:3236`)在 `#[test]` 内;与 §10 aec3 结论一致。
- ✅ **i18n 无缺键**:`en:`/`zh:` 计数 165/164 的差是 `LangProvider({ children })` 里 "childr**en:**" 子串
  误计,**非缺翻译**;逐条核对中英对齐。**不是 bug**(避免误报)。
- ⚠️ **48 处首方 `unwrap`/`expect`(非 test)**:分布 main(15)/realtime(13)/nvafx(7)/localvqe(5)/tauri(4)/
  core(2)/process_tap(2)。经通读,**音频热路径(`process_loop`/流回调/`process`)不 panic**(用 try_push/
  try_pop+fill);其余多在 setup/parse。**建议把 ROB-3 扩为**:P4 里对这 48 处做一次机械清点,确认
  长生命周期/外部输入路径无可达 `unwrap`(尤其 Tauri 3 处 `lock().unwrap()` 已在 ROB-3)。
- ⚠️ **依赖 future-incompat**:`app/src-tauri` 依赖链含 `block v0.1.6`(经 tauri-plugin-decorum→objc),
  "将被未来 Rust 版本拒绝"。纳入 **TEST-2** 的 `cargo audit`/依赖治理,P4 升级或替换。
- ✅ **执行前置就绪**:三个 workspace 各有 `Cargo.lock`(root / app/src-tauri / vendor/aec3 —— `cargo audit`
  需**分别**对三者跑);`app/node_modules` 与 `pnpm-lock.yaml` 已就位(`pnpm tsc --noEmit` 可直接跑);
  `app/src-tauri/binaries/` **不存在**(PKG-1 修复需新建并配 CLI→sidecar 的 build 编排)。

### 15.3 给 agent 的起步指令(在 P0 之前)
```
# 第 0 步:记录基线 + 刷绿(P0.0)
git checkout -b phase-0/green-baseline
cargo test --workspace --locked            # 记录基线测试结果(应绿;不绿则先查)
# 逐条修 §15.1 的 13+2 个 clippy 错误,每修一类一个原子提交,信息带 (P0.0)
cargo clippy --workspace --all-targets --locked -- -D warnings        # 必须 0 错
(cd app/src-tauri && cargo clippy --all-targets --locked -- -D warnings)  # 必须 0 错
# 全绿后合并,再进 §14.2 的 P0(CI/audit/fmt)→ P1 → …
```
**只有 §15.1 全绿后,§14.1 的"每次提交过 clippy 门"才真正可执行。** 这是整条工作流能否跑起来的前提。
