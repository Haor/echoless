# Product Update Strategy

本文记录 Echoless 产品化阶段的更新策略。这里的“热更新”分两类:

- **运行时参数热更新**:设备、backend、AEC 参数在 AEC runtime 运行中变更。
- **产品自更新**:GUI app、`echoless` sidecar、模型/runtime assets、配置 schema 的版本升级。

当前决策:

- 首版不做运行时参数热更新。设备、采样率、backend、模型、RTX runtime 参数变化时重启 AEC runtime。
- 产品自更新需要提前预留接口,但不在当前 CLI 后端里实现。
- 不在普通 push 上跑 release/update pipeline。更新产物只应由 tag 或手动 release workflow 生成。

## 调研来源

- Velopack: <https://github.com/velopack/velopack>
- Velopack docs: <https://docs.velopack.io/>
- Electron + Velopack workflow gist: <https://gist.github.com/EYHN/aba3d65fa945a79161b51e75dc323eb8>
- Tauri updater plugin: <https://v2.tauri.app/plugin/updater/>

## Velopack 摘要

Velopack 是跨平台 installer 和 automatic update framework。它的关键点:

- `vpk pack` 从已构建的 app 目录生成 installer、portable 包、full/delta update 包和 `releases.{channel}.json` feed。
- runtime 侧用 `UpdateManager` 检查、下载、应用更新。
- update feed 可以放在普通 HTTPS 静态文件服务、S3/R2/Azure、GitHub/GitLab release 或 Velopack Flow。
- channel 是核心概念。跨平台/跨架构必须避免 channel 混用,例如 `win-x64-stable`、`osx-arm64-beta`。
- Windows 更新会替换安装目录里的 `current`;运行中的 sidecar/exe 会阻止替换,所以应用更新前必须停止 AEC runtime。
- `vpk` CLI 依赖 .NET SDK;CI/release workflow 要安装并 pin 版本。

Velopack 适合:

- 想要 installer + delta update + release channel 统一管理。
- Electron 或非 Tauri GUI。
- 希望 app bundle 里同时带 GUI、sidecar、LocalVQE runtime/model 等文件。

主要成本:

- 需要额外维护 `vpk` release workflow。
- macOS 仍要处理 deep codesign、notarization、DMG/pkg 体验。
- 私有 GitHub release 或私有对象存储需要 token/鉴权设计。

## Gist 里的可借鉴点

该 gist 是 Electron + Velopack 的完整 release workflow 草案,不是 Echoless 可直接照抄的 Tauri 方案。可借鉴:

- main process 早期调用 `VelopackApp.build().run()`。
- 用 `UpdateManager` 包一层 app-level `UpdateStatus`。
- 暴露 `update:check`、`update:apply` IPC。
- 后台定时检查,发现更新后再进入前端状态流。
- 下载更新时提供 progress。
- apply update 前调用 `waitExitThenApplyUpdate`,然后退出 app。
- release workflow 先下载旧 release,再 pack 新 release,从而生成 delta。
- macOS release 需要 deep codesign、notarization、staple 和 gatekeeper 验证。
- release asset 可上传到 R2/S3 这类对象存储。

不建议照抄:

- Electron-specific IPC 和 electron-builder 路径。
- 示例里的 app id、bucket、secret、notary keychain 形态。
- 把 release workflow 绑定到普通 push。Echoless 应使用 tag/manual release。

## Tauri Updater 摘要

如果最终 GUI 用 Tauri,Tauri 自带 updater plugin 是更简单的首选预留方向:

- `tauri.conf.json` 里开启 `bundle.createUpdaterArtifacts = true`。
- 配置 updater `pubkey` 与 `endpoints`。
- update artifact 需要签名;客户端用内置公钥验证。
- 前端或 Rust 后端可调用 `check`、`downloadAndInstall`,安装后 relaunch/restart。
- production 默认要求 HTTPS endpoint。

Tauri updater 适合:

- 最终 app 是标准 Tauri bundle。
- 更新内容主要是 GUI app + bundled sidecar。
- 希望少维护一套外部 installer/update framework。

主要限制:

- installer/channel/delta/rollout 能力不如 Velopack 完整。
- 需要确认 sidecar、模型、dylib/DLL 和 platform runtime 是否全部被 Tauri bundle/updater artifact 正确覆盖。
- 大型可选资产,例如 RTX AEC runtime/model,可能仍要保留独立 `doctor/install` 下载流程,不能简单塞进每次 app update。

## Echoless 推荐

当前建议是 **预留 Updater API,暂不选死实现**:

```text
Frontend UI
  -> UpdateService abstraction
     -> Tauri updater implementation
     -> Velopack implementation
     -> no-op implementation for dev/unpackaged builds
```

首选判断:

- 如果 GUI 确定走 Tauri 且发行规模不大,先用 Tauri updater。
- 如果后续需要更成熟的 installer、delta 包、channel 切换、对象存储部署、跨框架迁移,再评估 Velopack。

必须预留的状态模型:

```ts
type AppUpdateStatus = {
  supported: boolean;
  channel: "stable" | "beta" | "dev";
  currentVersion: string;
  checking: boolean;
  availableVersion: string | null;
  releaseNotes: string | null;
  downloading: boolean;
  progress: number | null;
  downloaded: boolean;
  applying: boolean;
  error: string | null;
};
```

必须预留的命令:

- `updates.getStatus()`
- `updates.check({ manual: boolean })`
- `updates.download()`
- `updates.applyAndRestart()`
- `updates.setChannel(channel)`

应用更新前的安全流程:

1. 如果 AEC runtime 正在运行,先提示用户并停止 sidecar。
2. 保存当前配置和 diagnostics session metadata。
3. 确认没有 `echoless` sidecar、LocalVQE dylib/DLL、RTX backend handle 正在使用。
4. 下载并校验签名/manifest。
5. 应用更新并重启 GUI。

## 资产边界

应随 app update 走的内容:

- Tauri/Electron GUI。
- `echoless` sidecar executable。
- 默认配置 schema 与 UI manifest。
- 可合法再分发的 LocalVQE runtime/model。

不应默认随 app update 走的内容:

- NVIDIA AFX runtime/model,直到再分发许可确认。
- 用户 diagnostics 录音。
- 用户配置、设备选择、日志、crash reports。

## Release Channel 建议

最小可行:

- `win-x64-stable`
- `win-x64-beta`
- `osx-arm64-stable`
- `osx-arm64-beta`

后续如果支持 Intel Mac 或 Windows ARM64,再加:

- `osx-x64-stable`
- `win-arm64-stable`

不要让 Windows app 读取 macOS channel,也不要让 ARM64 app 读取 x64 channel。

## 前端可用更新能力

本节只说明后端/更新层能提供什么能力,不规定前端如何设计 UI。

- 能查询当前版本、channel、是否支持更新。
- 能手动检查更新。
- 能下载更新并报告进度。
- 能在下载完成后应用更新并重启 app。
- 能切换 release channel,前提是发行侧提供对应 feed。
- dev/unpackaged build 应能返回 `supported=false`。
- 如果 AEC runtime 正在运行,更新层应要求先停止 sidecar,再应用更新。

## 当前不做

- 不做静默强制更新。
- 不做运行时 patch 单个 DLL/model。
- 不允许用户输入任意 update URL。
- 不把 update pipeline 绑定到普通 CI push。
- 不把 RTX runtime/model 纳入 app update,直到许可确认。
