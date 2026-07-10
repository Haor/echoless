# 对 `AUDIT_REPORT_2026-07-10.md` 的复核与处置

> 复核日期：2026-07-10
> 复核基线：`dev@1aa747708d4d8eac8d732c7ed8c827e22d744508`(与被审报告同一 HEAD,复核期间产品代码零修改)
> 复核对象:Codex 出具的 `audit/AUDIT_REPORT_2026-07-10.md`(32 条:P1×7、P2×14、P3×11)与 `audit/DECISIONS.md`(6 项待决)
> 复核方式:5 路并行 agent 对 22 条重点条目逐条回源码验证(不信任审计给定行号,自行定位),外加 T-11 的 CI 成本实测

## 一、结论:审计事实准确,但严重度校准偏严、且未纳入用户的风险取舍

- **事实层面可信**:22 条重点核查中 **19 条 CONFIRMED、3 条 PARTIAL、0 条 REFUTED**;抽检的"安全正向证据"(下载 SHA 校验 + 原子 rename、Actions 全钉 commit SHA)全部属实;与上一轮 `docs/internal/audit/AUDIT_REPORT_2026-07-05.md` 的去重声明经抽查成立(T-11/D-09 等是"上轮修一半后的新状态",非换皮刷条目)。
- **严重度层面偏严**:两个 P1(D-09 许可证、S-12 签名)经**用户明确裁决关闭**(单开发者项目,证书与合规工具链成本不成比例,接受风险),它们压低了 S/D 两个维度评分,使综合分 6.1 偏低——剔除已裁决关闭的合规项后实际约 **6.8–7**。核心问题从来不是发布合规,而是 4 个运行态 P1(B-25/26/27/28)。
- **上下文更正(2026-07-10 用户确认)**:本仓库为**公开 GitHub Release 仓库**,面向外部用户分发,并非自用;D-09/S-12 的关闭是用户裁决而非"自用推论"。据此 D-12(公开 example.toml 误导)升级为必修并已当日修复;D-11 由用户裁决跳过(直接发正式版,RC 区分问题失效)。
- **归因层面有小漏**:P-05 措辞夸大;A-09 未先检查前端兜底是否生效就定 P2;B-30 可达性未折入定级;A-09 漏 `stream_error`、serde fallback `error`、status 的 `clock_skew_ref_correlated` 这 3 项漂移;B-25 漏 `skip_stale` 奇偶问题;B-29 把不喂 detector 的 `ref_underruns` 布尔化误归入 detector 缺陷。

## 二、三梯队处置

### 第一梯队 —— 真的值得修(运行态实质缺陷 + 白拿的回归门)

| 条目 | 问题 | 核实 | 为什么值得 |
|---|---|---|---|
| **B-26**【P1】 | macOS Process Tap 失联/拒权后以零参考"假运行" | CONFIRMED,无 watchdog,唯一清 `running` 的路径是 cpal `DeviceNotAvailable` | 静默降级成 bypass、UI 仍显示运行,现场通话回声全漏;macOS 主力平台 |
| **B-28**【P1】 | 旧 sidecar reader 过期 exit 污染新 run | CONFIRMED,竞态可达(reader 不 join,EOF 晚于新 run `started`),且无错误横幅 | Settings-Save 自动重启是高频路径;与黑屏事故同一"死页面"问题域 |
| **B-27**【P1】 | LocalVQE 连续错误时 near/far 缓冲无界增长(~128KB/s)+ 恢复后回放旧音频 | CONFIRMED:push 在 `?` 之前、错误被 processor 内部吞掉、上层无 reset | 真机 GPU 上下文丢失等场景直接命中 |
| **B-25**【P1】 | stereo reference ring 半帧提交 → L/R 永久错位 | CONFIRMED,复核**另发现**旧路径 `skip_stale` 丢奇数样本同样破坏声道奇偶 | VM 22% skew 下 ring 反复溢出高概率触发;错位后 AEC 参考实质失效 |
| **B-29**【P2】 | clock-skew 单向失明 + stereo 单位差 2× | CONFIRMED(布尔化归因有误,主体成立) | stereo 下 `ref_correlated` 恒 false → **告警彻底聋**;这套告警是定位 VM 22% skew 的核心诊断器,三处小改可修 |
| **T-11**【P1】 | 21 个 Tauri 后端测试 CI 从未执行 | CONFIRMED | 见下方成本实测:净增 ~5s、零冗余,守的全是安全/子进程/配置原子性边界 |

> B-25 与 B-29 同批联调(都动 reference 计数/帧语义);B-28 的 `run_id` 设计顺手覆盖 A-09 缺的字段。

#### T-11 成本实测(回应"是否拖慢 CI / 测试是否冗余")

| 项目 | 实测 | 说明 |
|---|---|---|
| 21 个测试运行耗时 | **1.13s** | 单元测试,毫秒级 |
| 测试二进制增量编译(deps 缓存命中) | **3.9s** | CI 用 `Swatinem/rust-cache`,依赖树早已缓存 |
| **加 `cargo test` 净增量** | **~5s** | 相对已有多分钟 Tauri build 可忽略 |

关键:CI 现在的 `clippy --all-targets` 只做类型检查、**不 codegen/链接可执行二进制**,`build --locked` 已编译好整棵依赖树和主 crate。`cargo test` 唯一新增工作是把 crate 以 test cfg 再 codegen+链接一个测试二进制(实测 3.9s)。编译成本 99% 已付,只差最后 5 秒执行断言。**不存在拖慢 CI。**

**冗余检查:0 条冗余、0 条 trivial。** 被测函数(`validate_browser_url`/`validate_open_path`/`write_toml_create_new`/`localvqe_model_pin` 等)只存在于 `app/src-tauri`,root workspace 与前端都测不到,无跨层重复。这 21 个测试恰好覆盖审计在别处担心"会悄悄变绿的回归类别":URL 白名单(含 `vb-audio.com.evil.example` 仿冒、换行注入)、路径穿越/open_path brand-root 限定、模型名 `../` 拒绝、下载 SHA 完整性、命令注入(不走 `cmd /C`)、配置原子写、子进程超时回收与生命周期(与 B-28 同域)。等于已养好一支消防队却从不出勤。

### 第二梯队 —— 顺手修,改动小且不引入新问题

| 条目 | 问题 | 修法(改动量) |
|---|---|---|
| **S-13**【P2】 | 设备名经 animejs `innerHTML` 注入(`<style>` 已动态复现生效) | 动画每帧写 `textContent` 替代 `innerHTML`(几行,纯替换) |
| **B-30**【P2→实际P3】 | CoreAudio remove listener 失败仍释放 context(理论 UAF) | 加 `if status == 0` 再 `Box::from_raw`(1–2 行) |
| **B-31**【P3】 | `tomlString` 漏转义控制字符 | 补一行 `\x00-\x1F` replace |
| **B-32**【P3】 | 日志文件名秒级碰撞绕过 8MiB cap | 文件名加 PID 后缀(1 行) |
| **S-14**【P3】 | 手写 URL parser 反斜线绕过(当前调用方全硬编码,不可达) | 换 `tauri::Url`/`url::Url` 解析(几行) |
| **D-11**【P2】 | RC tag 产物内部版本仍 `1.1.0` | **用户裁决跳过**:直接发正式版 `v1.1.0`,RC 区分问题自然失效 |
| **D-12**【P2】 | 随 CLI 发布的 `example.toml` 两处陈旧(声称 artifact 含 DLL+模型;称边界 SRC 是占位线性) | **已修(2026-07-10)**:example.toml 两处注释 + root Cargo.toml 一处注释,对齐 rubato FFT 现状与 GUI/CLI 资产分发方式 |
| **T-12**【P2】 | workflow 无 `pull_request` 触发 | `on:` 加一行,只影响 CI 触发面,零产品风险 |
| **D-13**【P3】 | 文档写 `~/.local/share/echoless`,实现是 `Echoless` | 改两处文档 |
| **D-14**【P3】 | app README sidecar 解析顺序漏 4 步 | 照 `bin_resolve.rs` 实际顺序重写一段 |
| **A-09 缩水版**【P2→P3】 | TS union 补 `stream_error`/`clock_skew_*`/serde fallback `error`,status 补 `clock_skew_ref_correlated`,`control_error.cmd` 接受 null | 兜底已生效,纯类型补齐;`stream_error` 能给"USB 麦被拔"精准提示 |

### 第三梯队 —— 没必要 / 过于严苛(用户裁决关闭或长期投资类)

| 条目 | 审计定级 | 为什么不用理 |
|---|---|---|
| **D-09** 许可证清单 + DEC-02 | P1 | **用户裁决关闭**:不建 cargo-about/SBOM 工具链;可选一行措辞修正避免留错误断言 |
| **S-12** 无签名/公证 + xattr 指令 + DEC-01 | P1 | **用户裁决接受 unsigned,并明确不改现有文案**($99/年 Developer ID + Authenticode 成本不成比例);风险已知且接受,本条关闭 |
| **P-05** 10ms 线程"持续分配" | P2 | 夸大:waveform/JSON 只在 status interval 构造、emitter spawn 一次性;常态热路径仅 1 Vec + 1 String,无实际爆音风险 |
| **T-15** release 无安装 smoke | P2 | 只在真要发 stable 时才需要的门 |
| **D-10** Linux 缺 LocalVQE 仍发布 + DEC-05 | P2 | 项目整体面向外部公开发布,但 **Linux 不纳入当前维护与发布质量保证范围**;维持 warning-only,本条挂起 |
| **D-15** `block 0.1.6` future-incompat | P2 | Rust 真正 hard error 前有很长窗口,届时升级 decorum 即可 |
| **C-07** ProcessorKind 退化 string | P3 | 触发需手改自己的 localStorage,且 CLI 会兜住报错 |
| **C-08** 零消费者占位 API + DEC-06 | P3 | 删不删都行,纯洁癖 |
| **S-15** 日志含设备名/路径无脱敏 | P3 | 无 token/密钥;公开 issue 附日志的泄露面限于用户名/设备名,"导出脱敏诊断"属产品体验项,不紧迫 |
| **T-13/T-14** 组件测试/路径 crate 测试 | P2/P3 | 有价值但属长期工程投资,非缺陷 |
| **A-10** manifest min/max 未透传 UI | P2 | 后端会兜住越界,属 UX 打磨项,低优先级 |
| **A-08** v1/自定义模型 GUI 选不到 + DEC-04 | P2 | 介于二三梯队:若想让用户(或自己)在 GUI 切 v1/自定义模型就修(最小改动=把 v1 加进前端硬编码列表),否则放着 |

> D-12 在明确“面向外部公开 release”后升级为必修,**已于 2026-07-10 修复**(见第二梯队表)。D-11 由用户裁决跳过。Linux 则由用户明确排除在当前维护与发布质量保证范围之外。

## 三、对 Codex 审计能力的评价

**强项**
- **事实核查纪律极好**:行号基本精确、无一条捏造、正向证据段经得起抽检;S-13 甚至用仓库实际 animejs + Chrome 动态复现,而非纸面推理。
- **增量意识真实**:与上轮报告去重是真去重,发现了"上轮修复的半途状态"(T-11、D-09),并对上轮 S-04 的对称性前提给出反证(D-10)。
- **交付形态好**:验收基线可执行、修复顺序含依赖分析(B-25/B-29 联调、B-28/A-09 共用 run_id)、DECISIONS 把裁决权留给用户、审计限制一节诚实(明说无真机长跑)。主动降温处(不把 quick-xml 说成可利用漏洞、S-14 标注属纵深防御)显示克制。

**弱项**
- **风险偏好校准不足**:未预先知道用户对公开发布仍明确接受 unsigned 与手工许可证清单,因此把 D-09/S-12 顶成 P1 并拉低维度评分。
- **严重度校准弱于事实校准**:擅长"这段代码有没有问题",不擅长"这个问题在这个产品里值多少"(P-05 夸大、A-09 未验兜底、B-30 未折可达性)。
- **覆盖有小漏**:A-09 漏 `stream_error`/serde fallback `error`/`clock_skew_ref_correlated` 3 项漂移、B-25 漏 `skip_stale` 奇偶、B-29 归因错位。

## 四、裁决建议

6 项待决的建议已填入 `audit/DECISIONS.md`,核心:DEC-01/DEC-02 经用户裁决关闭(接受 unsigned / 不建许可证工具链),DEC-03 选 C 但 B-28 的 `run_id` 属第一梯队要做,DEC-06 选 A(删除),DEC-04/DEC-05 挂起。落地顺序:**B-26 → B-28 → B-27 → B-25+B-29 联调 → T-11 接 CI** 为第一批,第二梯队约半天清完(D-12 已修、D-11 用户跳过),第三梯队直接关闭。
