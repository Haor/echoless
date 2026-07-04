# RTX AEC Runtime 分发方案

日期：2026-06-06

本文记录可选 NVIDIA AFX AEC backend 的 runtime 分发方案。Runtime 二进制不进入 git；确认 NVIDIA AFX SDK 和 model 再分发条款之前，不要公开上传这些二进制。

## 当前验证状态

- RTX AEC backend 已接入 `doctor` / 本地 zip `install` / 离线 WAV / 实时 `nvidia_afx_aec`。
- GitHub Actions run `27064782614` 已通过 Windows/macOS，artifact 为 `echoless-windows-X64` 与 `echoless-macos-ARM64`。
- Windows 本机 RTX 5080 Blackwell smoke 已通过：USB mic index `4`、reference `system`、output index `3`(CABLE Input)、45s diagnostics 成功、`runtime_errors=0`。
- 产品默认仍是 `aec3`；RTX AEC 是 Windows RTX 用户可选 backend，不与 AEC3 默认级联。

## 包形态

采用一个 Windows x64 通用 runtime zip，加每个 GPU 架构一个 model zip。

本地 staging 目录：

```text
C:\Users\haor2\workspace\aec\runtime-packages\dist-rtx-aec-2.1.0-aec48-split
```

通用 runtime asset：

```text
echoless-rtx-aec-common-runtime-win64-2.1.0.zip
size:   954.64 MiB
sha256: dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb
```

通用 runtime zip 包含共用 DLL、license 文件和 runtime 元数据。它不包含 `.trtpkg` 模型文件。

模型 assets：

| 架构 | Compute capability | Asset | 大小 | SHA256 |
|---|---|---|---:|---|
| Turing | 7.5 | `echoless-rtx-aec-model-win64-2.1.0-turing-aec48.zip` | 45.88 MiB | `951e03bb144156f4b27cbf2caa6930f9dabc3f1cb26a0afd9d9523f4d286dae9` |
| Ampere | 8.0 / 8.6 | `echoless-rtx-aec-model-win64-2.1.0-ampere-aec48.zip` | 45.82 MiB | `066e06ec18a7d4509675411a1e050e11b0cfc4fee30d69d783871333018c9ab9` |
| Ada | 8.9 | `echoless-rtx-aec-model-win64-2.1.0-ada-aec48.zip` | 46.13 MiB | `92170e6a259f9093397b93cf4385759c36697ecb9e308322405bce1abcb8e3df` |
| Blackwell | 10.0 / 12.0 | `echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip` | 46.66 MiB | `0e75bb7442d317990ef0d5a6477105f86b9bbae1c2c5e4a6bdfb8d4e9f42df5b` |

每个 model zip 都可以直接解压到 runtime 根目录。zip 内部包含：

```text
features/nvafxaec/models/<arch>/aec_48k.trtpkg
```

## 安装布局

推荐安装根目录：

```text
%LOCALAPPDATA%\Echoless\nvafx\2.1.0
```

安装器流程：

1. 下载 `echoless-rtx-aec-common-runtime-win64-2.1.0.zip`，解压到安装根目录。
2. 检测 NVIDIA driver 和 GPU 架构。
3. 下载与 GPU 架构匹配的 model zip。
4. 解压前校验 SHA256。
5. 把 model zip 解压到同一个安装根目录。
6. 确认 `features\nvafxaec\models\<arch>\aec_48k.trtpkg` 存在。

当前已支持本地 zip 安装：

```powershell
.\echoless.exe nvafx install `
  --common-zip .\echoless-rtx-aec-common-runtime-win64-2.1.0.zip `
  --model-zip .\echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip
```

命令会按已知 release asset 名称校验 SHA256、解压 zip、写入
`echoless-runtime-install-manifest.json`，最后自动运行 `nvafx doctor`。如果使用非官方
asset 名称，可传 `--common-sha256` / `--model-sha256` 显式校验。

未来 GitHub Release URL 模板：

```text
https://github.com/Haor/echoless/releases/download/<release-tag>/<asset-name>
```

staging 目录里的 `manifest.json` 已经按这个 asset 形态生成，包含 zip hash 和 payload model hash。

## Runtime 检测

`doctor` 命令或安装器检测失败时，不要要求普通用户安装 SDK，只提示最小缺失项：

- 缺 `nvidia-smi` 或 `nvcuda.dll`：安装 NVIDIA graphics driver。
- driver 低于 `572.61`：更新 NVIDIA graphics driver。
- 缺 `VCRUNTIME140.dll`、`VCRUNTIME140_1.dll` 或 `MSVCP140.dll`：安装 Microsoft Visual C++ 2015-2022 Redistributable x64。
- compute capability 不支持：当前 GPU 不可用 RTX AEC backend。
- 缺模型文件：下载匹配架构的 model zip，并解压到 runtime 根目录。

消费 prepared runtime 的机器不需要安装 CUDA Toolkit、TensorRT SDK、NVIDIA AFX SDK 或 NGC CLI。

跨平台边界：

- `nvafx doctor --json` 保留跨平台可运行，用作 GUI/安装器能力探针。
- macOS / Linux 上 `doctor` 应返回 `ok=false` 并报告 `platform=unsupported`；GUI 应据此隐藏或禁用 RTX AEC。
- `nvafx install`、`nvafx offline`、实时 `nvidia_afx_aec` 只支持 Windows x64。

## 集成备注

先从 Windows 侧集成，因为真实 runtime 验证需要 RTX GPU 和 AFX Windows DLL 加载行为。Mac 侧只验证跨平台构建、`doctor --json` 的不可用状态、AEC3/LocalVQE 路径；不能验证 RTX backend 音频效果。

建议实现顺序：

1. [x] 增加 `nvafx doctor` 检测和可读错误提示。
2. [x] 增加 runtime discovery：config、`ECHOLESS_NVAFX_RUNTIME_DIR`、默认 `%LOCALAPPDATA%` 安装根目录。
3. [x] 增加本地 zip installer：common runtime + per-arch model。
4. [x] 增加离线 AEC harness，用 WAV 做确定性对照。
5. [x] 增加实时 `nvidia_afx_aec` 可选 backend。
6. [ ] 确认再分发条款后，再增加远程下载 / 公开 release asset 支持。

RTX AEC v1 运行约束：

- Windows x64 only。
- 48 kHz / 10 ms / 480 samples per frame。
- mic mono、reference mono、output mono。
- `NvAFX_Run` 以两路 planar input 调用：`input[0] = near mic`，`input[1] = far reference`。
