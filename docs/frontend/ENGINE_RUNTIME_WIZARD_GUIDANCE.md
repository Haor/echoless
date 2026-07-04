# Engine Runtime Wizard Guidance

本文给 Echoless GUI / Tauri 前端使用,记录 Engine 选择页、LocalVQE 模型入口、NVIDIA AFX / RTX AEC runtime 安装向导的功能边界。本文不规定具体视觉设计,只规定前端状态、参数和引导不能越过的后端事实。

相关来源:

- `docs/frontend/FRONTEND_PARAMETER_BOUNDARIES.md`
- `docs/localvqe_inference.md`
- `/Users/harukishiina/workspace/codex/AEC/NVAFX_RUNTIME_INSTALLER_HANDOFF.md`
- `echoless processors --json`
- `echoless nvafx doctor --json`

## 总体产品口径

- 默认主路径仍是 `aec3`。它应该显示为 default / ready。
- `localvqe` 是独立实验 backend,不是 AEC3 的默认级联后处理。
- `nvidia_afx_aec` 是 Windows-only RTX AEC SDK backend,不是 NVIDIA Broadcast App。
- LocalVQE 和 RTX AEC 都可以显示在 Engine 选择页,但二者的 setup 逻辑不同:
  - LocalVQE 的模型是 Echoless 可随 artifact/bundle 带上的 GGUF 权重。
  - RTX AEC 的模型是 NVIDIA AFX runtime 目录下的按 GPU 架构选择的 `.trtpkg`,不应让普通用户手动挑选。

## LocalVQE 选择规则

当前后端状态:

- `localvqe` 处理器已经真实接入上游动态 C ABI。
- 后端配置需要 `model` 非空。
- `library` 可显式填写;不填时 CLI 会优先使用 Tauri 后端注入的
  `ECHOLESS_LOCALVQE_LIBRARY`。Tauri 后端会从环境变量、打包资源
  `resources/localvqe/native/`、CLI 同目录 / `localvqe/` 子目录中寻找 native runtime。
- CI 产出的 CLI artifact 会包含:
  - `models/localvqe-v1.3-4.8M-f32.gguf`
  - Windows: `localvqe.dll`
  - macOS: `liblocalvqe*.dylib` 及 GGML backend modules

前端策略:

- 开发态或 Tauri bundle 尚未正式包含 LocalVQE assets 时,显示 `SET UP` 并提供模型下载/模型目录入口是正确的。
- 产品化后应优先使用 bundled model:
  - 只有同时检测到 bundled/downloaded `.gguf` 模型与 `native_ready=true`,LocalVQE 才显示 `READY`。
  - 前端自动写入 `model = <bundled model absolute path>`。
  - `library` 可以省略;后端会通过 env 注入 bundled native library path。
- 用户手动放入 `.gguf` 是 secondary action,通过打开模型目录完成,不是首屏必须项。
- LocalVQE 卡片应标为 experimental,避免暗示它一定优于 AEC3。
- LocalVQE 不要求把全局 `sample_rate` 改成 `16000`;GUI 应保持默认 `48000 / 10ms`。
  `ProcessorChain` 会把 48 kHz runtime mic/ref 边界适配到 LocalVQE 的 16 kHz mono 域,
  再把输出适配回 48 kHz。因此 macOS `LocalVQE + System Audio (Process Tap)` 是允许组合。
- 组合限制:macOS `reference=system` 当前走 Process Tap helper,只支持全局 pipeline
  `sample_rate = 48000`。只在用户把全局 `sample_rate` 改成非 48000 时阻止/提示
  Process Tap 不支持。

当前后端/Tauri 探针:

```ts
interface LocalVqeAssets {
  models_dir: string;
  models: Array<{ filename: string; path: string; source: "downloaded" | "bundled" | string }>;
  native_ready?: boolean;
  library_path?: string | null;
  native_dir?: string | null;
  native_files?: string[];
  cli_path?: string | null;
  process_tap_helper_path?: string | null;
}
```

前端不要硬编码 Tauri bundle 内部路径;以 `localvqe_assets()` 返回的绝对路径和
`native_ready` 为准。

## NVIDIA AFX / RTX AEC 选择规则

当前后端状态:

- backend kind 是 `nvidia_afx_aec`。
- 只支持 Windows x64。
- 运行约束固定:
  - `sample_rate = 48000`
  - `frame_ms = 10`
  - `reference_channels = "mono"`
  - mic mono / reference mono / output mono
- 前端应以 `echoless nvafx doctor --json` 作为 RTX AEC 可用性的主探针。
- `runtime_dir` 和 `model_path` 都支持 `auto`,但普通用户不应直接理解或选择 `.trtpkg` model path。
- 公共 release 下载安装已由 CLI 提供:

```powershell
echoless.exe nvafx download-install --json
```

  可选 `--runtime-dir <DIR>` 和 `--tag <RELEASE_TAG>`。stdout 是 `{ ok, report }`;
  下载、SHA256 校验、解压日志走 stderr。

前端策略:

- macOS / Linux 上显示 `Windows + RTX only`,不可生成可运行配置。
- Windows 上运行 `nvafx doctor --json`:
  - `doctor.ok = true`:显示 `READY`,允许选择 `nvidia_afx_aec`。
  - `doctor.ok = false`:显示 setup / fix required,并按 checks 给出最小行动。
- 用户可选择 runtime root,然后触发 `nvafx doctor --runtime-dir <dir> --json` 重新检测。
- 不要把 NVIDIA model 做成普通文件 picker。
- `model_path` 只应在高级/开发者 override 中出现;默认保持 `auto`。

## NVIDIA Runtime Installer Wizard

安装向导应基于 `/Users/harukishiina/workspace/codex/AEC/NVAFX_RUNTIME_INSTALLER_HANDOFF.md`。关键事实:

- Runtime 和 model 不进 git。
- 当前 release asset 已可从 GitHub public release 匿名下载;GUI 不需要 GitHub token / 登录态。
- 公开再分发前必须确认 NVIDIA AFX runtime / model 许可。
- 当前 package 是 Windows x64 / AFX SDK 2.1.0 / AEC 48 kHz only。

安装目标:

```text
%LOCALAPPDATA%\Echoless\nvafx\2.1.0
```

安装内容:

1. common runtime zip:

```text
echoless-rtx-aec-common-runtime-win64-2.1.0.zip
```

2. 按 GPU 架构选择一个 model zip:

| Compute capability | 架构 | Model asset |
|---|---|---|
| `7.5` / `75` | Turing | `echoless-rtx-aec-model-win64-2.1.0-turing-aec48.zip` |
| `8.0` / `80` | Ampere | `echoless-rtx-aec-model-win64-2.1.0-ampere-aec48.zip` |
| `8.6` / `86` | Ampere | `echoless-rtx-aec-model-win64-2.1.0-ampere-aec48.zip` |
| `8.9` / `89` | Ada | `echoless-rtx-aec-model-win64-2.1.0-ada-aec48.zip` |
| `10.0` / `100` | Blackwell | `echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip` |
| `12.0` / `120` | Blackwell | `echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip` |

安装后 model payload 应位于:

```text
features/nvafxaec/models/<arch>/aec_48k.trtpkg
```

普通用户不需要知道这个路径。

### Wizard 状态机

前端建议按状态而不是按裸 check 列表组织:

| State | 来源 | 前端行动 |
|---|---|---|
| `unsupported_platform` | 非 Windows x64 | 禁用 RTX AEC,提示 Windows + RTX only |
| `missing_driver` | 无 `nvidia-smi` 或无 `nvcuda.dll` | 引导安装 NVIDIA graphics driver |
| `unsupported_gpu` | compute capability 不在映射表 | 禁用 RTX AEC |
| `driver_too_old` | driver 低于 `572.61` | 引导更新 NVIDIA graphics driver |
| `missing_vc_redist` | 缺 VC++ runtime DLL | 引导安装 Microsoft Visual C++ 2015-2022 Redistributable x64 |
| `runtime_not_installed` | common runtime 缺失 | 进入 runtime 下载/选择向导 |
| `model_not_installed` | 匹配架构 `.trtpkg` 缺失 | 下载/安装匹配架构 model zip |
| `ready` | `doctor.ok = true` | 允许选择 RTX AEC |

### Wizard 步骤

推荐 flow:

1. Probe
   - 调用 `nvafxDoctor()`.
   - 显示 GPU 名称、driver version、selected arch、runtime dir。
2. Prerequisite fix
   - 根据 `doctor.report.checks[].status/action/detail` 显示缺失项。
   - driver / VC++ / unsupported GPU 这类问题不应进入下载 runtime 步骤。
3. Runtime source
   - 默认路径:调用 `echoless.exe nvafx download-install --json`,由后端自动选 GPU 架构并下载 common + model。
   - 备用路径:选择本地已下载 common zip + model zip,调用 `echoless.exe nvafx install ...`。
4. Model auto-selection
   - 根据 doctor 返回的 selected arch 或 compute capability 自动选择 model asset。
   - 用户不选择 Turing/Ampere/Ada/Blackwell,只展示检测结果。
5. Install
   - 调用 CLI installer,不要让前端自己解压 zip。公共下载路径:

```powershell
echoless.exe nvafx download-install `
  --runtime-dir "$env:LOCALAPPDATA\Echoless\nvafx\2.1.0" `
  --json
```

   - 本地 zip 路径:

```powershell
echoless.exe nvafx install `
  --common-zip <download-dir>\echoless-rtx-aec-common-runtime-win64-2.1.0.zip `
  --model-zip <download-dir>\echoless-rtx-aec-model-win64-2.1.0-<arch>-aec48.zip `
  --runtime-dir "$env:LOCALAPPDATA\Echoless\nvafx\2.1.0"
```

6. Verify
   - 安装后调用:

```powershell
echoless.exe nvafx doctor --runtime-dir "$env:LOCALAPPDATA\Echoless\nvafx\2.1.0" --json
```

   - 只有 `ok=true` 才把 RTX AEC 标为 ready。

### 下载与分发边界

- 当前 GitHub Release 是 public asset,前端可以调用后端 `nvafx download-install` 间接下载。
- 前端仍不要自己下载或解压 runtime;SHA256、缓存、解压和 manifest 写入由 CLI 负责。
- `runtime.echoless.ai` 在 handoff 中只是占位产品服务域名,不代表已上线。
- 在公开许可确认前,GUI 可以支持:
  - manual install from local zip。
  - GitHub public release download-install。
  - doctor-only readiness display。
- 不要在普通 push 或普通 GUI flow 中隐式下载 1GB runtime。

## Engine 页推荐行为

- AEC3:
  - Always visible.
  - Default.
  - Ready unless backend manifest is unavailable.
- LocalVQE:
  - Visible as experimental.
  - Ready only when bundled or selected model is available.
  - If not ready, selecting card leads to model setup area.
- RTX AEC:
  - Visible but disabled on unsupported OS.
  - On Windows, readiness from `nvafx doctor --json`.
  - If runtime/model missing, selecting card leads to installer wizard, not model picker.
  - If ready, generated config should keep `runtime_dir = "auto"` and `model_path = "auto"` unless user selected a custom runtime dir.

## Copy / naming constraints

- Use `NVIDIA AFX / RTX AEC SDK backend`.
- Do not call it `NVIDIA Broadcast AFX`.
- Do not imply Broadcast App is required for RTX AEC SDK.
- Do not expose `.trtpkg` as a normal user-facing model choice.
- If mentioning Broadcast, phrase it as optional downstream post-processing for residual noise, not part of RTX AEC setup.
