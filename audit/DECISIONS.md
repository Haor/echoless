# Echoless `dev@1aa7477` 审计待决项

> 来源：`audit/AUDIT_REPORT_2026-07-10.md`
> 复核：`audit/AUDIT_REVIEW.md`(2026-07-10 逐条回源码验证 + 三梯队分级)
> 状态：**已按“面向外部公开发布,接受 unsigned 与手工许可证清单,不保证 Linux”填入裁决**
> 使用方式：每项"裁决"处为复核给出的建议;未列入本文件的条目按 `AUDIT_REVIEW.md` 三梯队执行。

> **裁决前提(2026-07-10 用户确认)**:本项目为单开发者维护的**公开 GitHub Release 仓库**,面向外部用户分发安装包。
> 涉及**签名证书/许可证工具链**的条目(DEC-01/DEC-02)由用户**明确裁决跳过**——理由是成本对当前项目规模不成比例、接受相应风险,
> 而非"自用无需合规"。面向公开用户的文档/示例中的误导性内容(如 D-12)则按公开产品标准修正。
> **平台边界**:公开发布范围不等于全平台同等保证;用户明确决定当前**不维护、不保证 Linux 产物质量**,Linux 相关发布门保持现状;但公开 Linux 产物的事实性文档错误仍应纠正。

## DEC-01 — S-12：签名/公证完成前的发布策略

- **A（推荐）**：暂停 stable release；RC 可保留但明确标记 unsigned/testing-only，移除绕过 Gatekeeper 指令。macOS Developer ID + notarization、Windows Authenticode、provenance 全部到位后再发 stable。
- **B**：macOS 优先签名/公证，Windows 暂时 unsigned 并显著警告。
- **C**：继续 unsigned release，只改文案说明风险。
- **约束**：A/B 需要证书、CI secrets 与发布权限；审计 agent 不创建或导入凭据。
- **裁决**：**C(用户裁决:接受 unsigned)** —— 继续 unsigned release。Developer ID($99/年)+ Windows Authenticode 对单开发者项目成本不成比例,用户接受该风险。**不动现在的文案。**

## DEC-02 — D-09：第三方许可证与 SBOM 工具链

- **A（推荐）**：Rust 使用 target-aware `cargo-about`（或等价工具），JS 使用 production-lockfile collector，CI 同时生成 notices + SPDX/CycloneDX SBOM，unknown/missing/denied license fail closed。
- **B**：使用 `cargo-deny` 做 policy，另写小脚本生成 notices；JS 独立收集。
- **C**：继续手工维护 `THIRD-PARTY-LICENSES.md`，只增加一次性对账。
- **约束**：A/B 会新增构建工具和 CI 维护面；需 pin 版本并缓存，不应把 dev-only 包混入分发清单。
- **裁决**：**C(用户裁决关闭)** —— 不引入 cargo-about/SBOM 工具链,`THIRD-PARTY-LICENSES.md` 维持手工清单。用户明确决定不为此建 CI 维护面。若介意其"Rust deps 仅 MIT/Apache/BSD"的措辞不准确,可顺手把该句改为"见各 crate 各自 license,未逐项枚举",避免留下错误断言。整条不进第一/第二梯队。

## DEC-03 — A-09 / B-28：跨进程运行事件契约

- **A（推荐）**：Rust 侧定义共享 event DTO/enum + `run_id`，生成/提交 JSON schema 或 TypeScript 类型；CLI fixtures 与前端 runtime parser 双保险。
- **B**：不引 codegen；CLI 生成 versioned golden JSONL fixtures，TypeScript 手写 union + runtime parser，CI 做双端契约测试。
- **C**：仅补当前缺失字段和 `run_id`，不建立 schema/fixture 门。
- **约束**：无论选项，旧代 status/exit 必须可识别并被后端/tray/前端忽略。
- **裁决**：**C + B-28 的 `run_id` 归第一梯队** —— 不引 schema codegen。复核确认前端 `App.tsx:948` 的 `if (ev.type !== "status") return;` 硬白名单 + 顶层 ErrorBoundary **已完全兜住未知事件**(当年黑屏事故的五层修复仍在),所以 A-09 的"契约漂移"实际是 P3 卫生项,不值 codegen 投资。**但 B-28 必修**(第一梯队):为每个 run 分配单调 `run_id`,status/exit 携带 ID,只有 active ID 可更新 RunState/tray、前端忽略旧代事件。顺手在补 `run_id` 时把 CLI 已发但 TS union 缺的 `stream_error`/`clock_skew_warning`/`clock_skew_resolved`/serde fallback `error`、`RuntimeStatus.clock_skew_ref_correlated` 及 `control_error.cmd: string \| null` 补进类型(第二梯队),其中 `stream_error` 能给"USB 麦被拔"精准提示。不建 golden fixtures/schema 门。

## DEC-04 — A-08：自定义 LocalVQE `.gguf` 的产品承诺

- **A（推荐）**：正式支持目录发现；后端提供统一 catalog，未知 `.gguf` 以 generic model 行显示并可选，同时明确兼容性/错误提示。
- **B**：只支持官方 pin 模型；忽略/隐藏未知文件，并修改生成 README 和用户文档，不再承诺任意 `.gguf`。
- **C**：官方模型 UI 可选，自定义模型仅允许高级手工路径，不保证兼容。
- **约束**：模型能力（AEC-only、NS、采样率、backend）不能再只靠文件名推断。
- **裁决**：**挂起** —— 复核定级此条介于二三梯队。你自己就是"用户",若确实想在 GUI 里切后端已 pin 的 `localvqe-v1-1.3M-f32.gguf` 或自炼 `.gguf`,**最小动作**是把 v1 加进前端 `EnginePage.tsx` 的 `LVQE_MODELS` 硬编码列表(几行);完整 A(后端返回统一 catalog + 未知模型 generic 行)是产品化投资,非当前必需。当前若不开放模型切换,可维持现状。**决定点留给你**:要在 GUI 切非默认模型 → 做最小 A;否则挂起。

## DEC-05 — D-10：Linux 缺 LocalVQE runtime 时的发布行为

- **A（推荐）**：tag/release fail closed；native runtime 构建或 FFI load smoke 失败就不发布 Linux installer。
- **B**：继续发布，但构建时从 manifest/UI 移除 LocalVQE，并在 asset/release notes 明确该产物不含此引擎。
- **C**：维持 warning-only fallback。
- **约束**：不能出现“UI 宣称可用、运行时必失败”的绿色 release。
- **裁决**：**维持 C(Linux 不纳入当前保证范围)** —— 项目整体面向外部公开发布,但用户明确决定当前不维护、不保证 Linux 产物质量。因此保留 `continue-on-error` 与 warning-only fallback,不为 Linux 增加 fail-closed 发布门。若未来单独宣布 Linux 为受支持平台,再改为 A(native runtime 或 FFI load smoke 失败即不发布 Linux installer)。本条当前挂起。

## DEC-06 — C-08：零消费者公开 API

- **A（推荐）**：删除 `NullSource/NullSink/MonotonicClock/StdClock/DeviceKind/DeviceInfo` 及失真文档，未来有真实消费者时再按需求引入。
- **B**：保留，但指定 owner、启用里程碑和兼容承诺，并加入真实行为测试。
- **约束**：不接受“保留占位但继续零测试/零消费者”的状态。
- **裁决**：**A(删除)** —— 删掉 `NullSource/NullSink/MonotonicClock/StdClock/DeviceKind/DeviceInfo` 及 `docs/internal/architecture/audio_io_scope.md` 中让读者误以为已有 fallback/test 路径的表述。零消费者的公开占位 API 只增加维护面和误导;将来有真实消费者时按需重新引入。属第二/三梯队卫生项,删除后跑 root 全门验证。

## 裁决完成检查

> 下列为复核(`AUDIT_REVIEW.md`)给出的建议裁决,已填入各 DEC;打勾表示"已给出建议",**最终采纳仍待用户确认**。

- DEC-01 → C
- DEC-02 → C(关闭 SBOM 工具链;可选修正措辞)
- DEC-03 → C + B-28 `run_id` 归第一梯队(不建 codegen)
- DEC-04 → 挂起(取决于你是否要在 GUI 切非默认模型)
- DEC-05 → 维持 C,挂起(Linux 不纳入当前维护与发布质量保证;未来宣布支持时再 fail-closed)
- DEC-06 → A(删除零消费者占位 API)

建议裁决日期:`2026-07-10`
建议来源:`AUDIT_REVIEW.md`(assistant 复核)
最终裁决人:`待用户确认`
