# Handoff — LocalVQE 模型选择 / 下载(前端已做,剩打包 + 原生库)

**From:** 前端(GUI)
**To:** Codex(后端 / 打包)
**状态:** 前端模型列表 + 下载已实现;Tauri sidecar/resource 接入已补。
默认模型与 native runtime 会由 `pnpm prepare:tauri-assets` 在构建环境提供产物时复制进 bundle。

---

## 1. 前端已实现(2026-06-08)

引擎页选中 LocalVQE 时,内嵌一个模型列表(替代原来的单一 .gguf 选择器):

- 列出官方 repo `LocalAI-io/LocalVQE` 的 4 个 `.gguf` 模型:
  `v1.3-4.8M`(**默认**)/ `v1.2-1.3M` / `v1.1-1.3M` / `v1-1.3M`,带参数量 + 大小。
- 列表内置在 **LocalVQE 卡片内**(NVAFX checklist 盒子风格):每行一个状态盒子 ——
  绿 `OK`=已下载可用、黄 `下载/GET`=未下载(点整行即下载)、`✓`=当前选中(整行高亮)。
- 已存在的(下载目录 / 打包资源)点一下即设 `params.model`;未下载点一下即从 HF 拉取后自动选中。
- 卡片底部:「打开模型目录 ↗」(`open_path(models_dir)`)+「官方 repo ↗」。
  ~~原「选本地 .gguf…」文件选择器已移除~~,改为打开目录 —— 用户把 `.gguf` 丢进该目录即被检测。
- `localvqe_models_dir` 首次创建目录时会写一个 `README.txt`,说明「把 LocalVQE .gguf 放这里;
  应用内下载也落这里;任何 .gguf 会被自动检测并可在引擎页选用」。
- 无选中模型时不再显示突兀的红/黄「需要模型」文案;由卡片右上角状态(`待配置`/SET UP,amber)体现。

### Tauri 命令(`app/src-tauri/src/lib.rs`)
- `localvqe_assets() -> { models_dir, models, native_ready, library_path, native_dir, native_files, cli_path, process_tap_helper_path }`
  - `models_dir` = `<app_local_data>/localvqe/models`(自动创建)。
  - 扫描下载目录 + **打包资源 `resources/localvqe/models`** 里的 `.gguf`。
  - 扫描 Tauri sidecar / resource 路径,返回 LocalVQE native runtime 是否可用。
- `download_localvqe_model(filename) -> path`
  - 从 `https://huggingface.co/LocalAI-io/LocalVQE/resolve/main/<filename>` 用 `curl -fL` 下载到
    `models_dir`(先写 `.part` 再 rename);限定 `.gguf` 文件名、防路径穿越。
- 前端 `api.ts`:`localvqeAssets()` / `downloadLocalvqeModel()`;`EnginePage` 内消费。

## 2. 后端 / 打包现状

### 2.1 默认模型(用户确认:**v1.3-4.8M**)
- `tauri.conf.json` 已包含 `bundle.resources = ["resources/"]`。
- `pnpm prepare:tauri-assets` 会在发现 `ECHOLESS_LOCALVQE_MODEL` / `LOCALVQE_MODEL` /
  CI `RUNNER_TEMP` 中的 `localvqe-v1.3-4.8M-f32.gguf` 时,复制到
  **`resources/localvqe/models/`**。
- 普通 dev 环境没有模型产物时不会阻塞,`localvqe_assets` 仍会扫描用户下载目录。

### 2.2 原生库打包 + env 注入(LocalVQE 能跑的**前提**)
后端 `crates/echoless-processors/src/localvqe.rs` 启动需要原生库
(`liblocalvqe.dylib` / `.so` / `localvqe.dll`),查找顺序:`library` 参数 →
`ECHOLESS_LOCALVQE_LIBRARY` env → CLI 自身的默认搜索。
- Tauri 后端已在 `echoless_command()` 注入 `ECHOLESS_LOCALVQE_LIBRARY`,并把 native 目录 prepend 到
  `PATH` / `LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH` / `DYLD_FALLBACK_LIBRARY_PATH`。
- `pnpm prepare:tauri-assets` 会在发现 `ECHOLESS_LOCALVQE_LIBRARY` 或 CI LocalVQE build 产物时,
  把 `liblocalvqe*` / `localvqe.dll` 与 GGML companion libraries 复制到
  **`resources/localvqe/native/`**。
- 前端以 `native_ready` 判定是否允许 LocalVQE READY;只有模型存在但 native runtime 缺失时显示
  `SET UP` / `缺少原生运行库`。

### 2.3 CLI sidecar / helper
- `tauri.conf.json` 已配置 `bundle.externalBin = ["binaries/echoless"]`。
- `pnpm prepare:tauri-assets` 会构建当前平台 CLI,复制为
  `src-tauri/binaries/echoless-<target-triple>{.exe}`。
- macOS 会构建/复制 `tools/macos-process-tap-poc` 到 `resources/helpers/echoless-process-tap-poc`;
  Tauri 后端会注入 `ECHOLESS_PROCESS_TAP_HELPER`。

## 3. 验收
- 打包后:LocalVQE 默认就有 v1.3 模型可直接「使用」+ 开 ON 能跑(库被找到)。
- 下载其它版本(v1/v1.1/v1.2)→ 落到 models 目录 → 选中 → 开 ON 也能跑(库经 env 找到)。
- 无库时报错清晰(已有 bail 文案;前端崩溃退出会在底栏显示该 stderr)。

## 4. 关联
- 模型加载 / 库查找:`crates/echoless-processors/src/localvqe.rs`(line ~85-110, 196-198, 500-509)。
- LocalVQE 16k 原生 / ProcessorChain 适配:见 `DEVICE_SAMPLE_RATE_RESAMPLING_HANDOFF.md`。
