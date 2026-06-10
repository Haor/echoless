# RTX Download-Install Handoff (frontend → Codex)

日期: 2026-06-08
作者侧: 前端 (Tauri GUI)
读者侧: 后端 / Codex

本文同步前端当前进度,并请后端实现 RTX runtime 的**公共 release 直接下载安装**。
资产命名 / SHA256 / 目录结构 / 状态机不在此重复,见:

- `NVAFX_RUNTIME_INSTALLER_HANDOFF.md`(资产、SHA256、archive 结构、安装根目录、CLI install 用法)
- `docs/frontend/ENGINE_RUNTIME_WIZARD_GUIDANCE.md`(向导状态机、模型按架构自动选、文案边界)

## 1. 仓库已公开 → 下载不再需要鉴权

用户已把 `Haor/echoless` 转为 public repo。Release asset 现在可**匿名直连下载**,例如:

```text
https://github.com/Haor/echoless/releases/download/rtx-aec-runtime-win64-2.1.0-aec48-preview.1/echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip
```

因此向导里 DOWNLOAD 源的文案已从 "private preview · requires sign-in" 改为
"from GitHub public release · auto-matches your GPU model"。不再需要 token / 登录态。

Release base(其余 asset 同前缀):

```text
https://github.com/Haor/echoless/releases/download/rtx-aec-runtime-win64-2.1.0-aec48-preview.1/
  echoless-rtx-aec-common-runtime-win64-2.1.0.zip          (955 MiB)
  echoless-rtx-aec-model-win64-2.1.0-<arch>-aec48.zip      (~46 MiB, <arch> ∈ turing|ampere|ada|blackwell)
  manifest.json
  SHA256SUMS.txt
```

`<arch>` 由 `nvafx doctor --json` 的 `report.selected_arch`(或 compute capability 映射)决定;
普通用户不选架构。映射表见 installer handoff。

## 2. 前端当前状态(commit 478cf7d,仅 `app/`)

GUI 已落地 Engine 选型页 + RTX 安装向导 + 开发态模拟。相关文件:

- `app/src/nvafx.ts` — `deriveRtxState()`(doctor checks → 8 态)、`ladderStatus()`、
  `nvafxModelAsset(arch)`、`simNvafxDoctor()`(dev 模拟)。
- `app/src/pages/RtxSetupPage.tsx` — 接管式向导:SYSTEM 读出 → READINESS 阶梯 →
  自适应 ACTION(硬阻断 / 外部修复 / 本地 zip 安装 / 公共下载 / ready)。
- `app/src/pages/EnginePage.tsx` — 三引擎规格牌 + 就绪门槛 + `SET UP RTX »` 入口。
- `app/src-tauri/src/lib.rs` — Tauri 命令(app 层,shell CLI)。

向导用 doctor 的真实 check 名派生状态(`platform` / `nvidia-smi` / `nvcuda.dll` /
`vc-runtime:*` / `gpu:N:driver` / `gpu:N:arch` / `runtime-dir` / `runtime:*` /
`runtime:model`),无需后端改 doctor 输出。

## 3. 前端调用的后端 seam(当前状态)

都是 `app/src-tauri/src/lib.rs` 的 Tauri command,内部 shell `echoless` CLI。
返回值前端按 `NvafxDoctor = { ok: bool, report: DoctorReport }` 解析。

| Tauri command | 参数 | 现状 | 实现 |
|---|---|---|---|
| `nvafx_doctor` | `runtime_dir?` | ✅ 已接 | `echoless nvafx doctor [--runtime-dir D] --json` |
| `nvafx_install` | `common_zip, model_zip, runtime_dir?` | ✅ 已接 | `echoless nvafx install ...` → 再 `nvafx doctor --json` 回传 |
| `open_url` | `url` | ✅ 已接 | 系统默认浏览器(驱动 / VC++ 下载页) |
| **`nvafx_download_install`** | `runtime_dir?` | ✅ 前端已接 | shell `nvafx download-install [--runtime-dir D] --json`(**后端已实现**) |

`nvafx_download_install` 已改为 shell `echoless nvafx download-install --json`,
该子命令打印 `{ok, report}` doctor JSON 到 stdout。下载/校验/解压日志走 stderr,
错误经 stderr 透传给前端。

## 4. 公共 release 下载安装

### CLI 子命令(与现有 `nvafx install` / `doctor` 一致)

```text
echoless nvafx download-install [--runtime-dir <DIR>] [--tag <RELEASE_TAG>] --json
```

已实现行为:

1. 跑 `nvafx doctor` 取 `selected_arch`。
   - 拿不到架构 → 报错(前端在此前已用阶梯拦住,不会走到这步)。
2. 从公共 release 下载:
   - `echoless-rtx-aec-common-runtime-win64-2.1.0.zip`
   - `echoless-rtx-aec-model-win64-2.1.0-<arch>-aec48.zip`
   - 下载源默认上面的 release base;`--tag` 可覆盖。
3. 默认 preview release 使用 CLI 内置 SHA256 pin 作为信任锚;release 的 `SHA256SUMS.txt`
   只做交叉校验,不一致则失败。非默认 `--tag` 才使用 release sums。
4. 复用现有 install 逻辑解压到 `runtime_dir`(缺省 `%LOCALAPPDATA%\Echoless\nvafx\2.1.0`),
   写 `echoless-runtime-install-manifest.json`,其中包含 `install_source.kind = "github-release"`。
5. 跑 `nvafx doctor --json` 并打印到 stdout(JSON)。

前端 `nvafx_download_install` 已 shell 这个子命令(`--json`),直接回传其 stdout。
**返回契约:stdout 是 `{ "ok": bool, "report": {...} }`(同 `nvafx doctor --json`)。**

### 进度反馈(可选但建议)

1GB 下载较慢。若 CLI 能向 stderr 或 stdout 流式输出进度(下载 % / 解压阶段),
前端可以从忙态升级成进度条。当前前端只显示 "downloading… ~1 GB" 忙态。
若不做流式,保持现状即可。

## 5. 其它后端待办(非阻塞)

- **model-only 安装**:当前 `nvafx install` 要求 `--common-zip` 和 `--model-zip` 都给。
  当 doctor 是 `model_not_installed`(common 已装,只缺架构模型)时,前端只能让用户再给
  common zip(本地 zip 模式)或走完整下载。建议加 `nvafx install --model-zip <Z>`
  (允许省略 common)以支持"只补模型"。
- 当前 `nvafx_install` / download 都是 shell CLI 后取末尾 doctor JSON;若 install 失败,
  CLI 应以非 0 退出 + stderr 给清晰原因(前端把 stderr 当错误展示)。

## 6. 不要回退的前端约束(供后端联调时注意)

- 不暴露 `.trtpkg`,不让普通用户选 GPU 架构(自动)。
- 文案:`NVIDIA AFX / RTX AEC SDK`,不叫 Broadcast AFX;Broadcast 仅作可选下游降噪。
- RTX v1 约束固定:Windows x64 / 48k / 10ms / mono mic+ref+output。
- 就绪判定唯一真源是 `nvafx doctor --json` 的 `ok` 与 `checks`。

## 下一会话建议

- 联调: Windows RTX 机器上真机走 doctor → download-install → doctor=ok → run。
- 后端后续:如果需要更细进度条,再让 CLI 对 stderr 输出结构化 phase/progress。
