# Echoless Windows 朋友试用指南

这份指南面向非开发者试用。目标不是跑完整工程测试,而是确认 Windows 安装包能正常安装、真实通话软件能收到 Echoless 处理后的麦克风声音,以及 AEC3 在外放场景下是否明显减少回声。

## 当前安装包

- 分支:`phase-1/usable`
- Commit:`c89862896bb04223fc8ad7ff7c36dd5e39df3442`
- GitHub Actions run:`27352043191`
- 构建页面:https://github.com/Haor/echoless/actions/runs/27352043191
- Artifact 名称:优先下载 `echoless-windows-X64`
- Artifact ID:`7567107638`
- Artifact 大小:`44,475,376` bytes
- GitHub artifact digest:`sha256:de498550311a9b130efdf3f240687cf2fc99ac4b218aa7db9749f58ff7327d7b`
- 安装器文件:下载 artifact 后,找到其中的 `.exe` 安装器运行。artifact 里也会包含 CLI zip,普通试用优先用 `.exe` 安装器。

注意:

- 这是测试包,Windows 可能提示未知发布者或 SmartScreen 警告。
- 仓库是私有的,没有仓库权限的人不能直接下载 GitHub Actions artifact。给朋友试用时,由有权限的人下载 `echoless-windows-X64` 后转发整个 artifact zip 或解压后的 `.exe` 安装器。
- Echoless 目前不自带虚拟麦克风驱动。Windows 上需要先安装 VB-CABLE 或等价虚拟音频设备。
- 默认测试路径是 `AEC3`,这是当前主线。`LocalVQE` 和 `NVIDIA RTX AEC` 是可选后端,不要混在同一次主观测试里判断。

## 需要先准备

1. Windows 10/11 电脑。
2. 一个真实麦克风,例如 USB 麦克风、声卡麦克风或笔记本内置麦克风。
3. 一个外放播放设备,例如音箱、显示器扬声器或耳机外放测试。
4. 一个虚拟音频设备:
   - 推荐 VB-CABLE。
   - 安装后系统里通常会出现 `CABLE Input` 和 `CABLE Output`。
   - Echoless 输出选择 `CABLE Input`。
   - Discord / VRChat / 语音软件的麦克风选择 `CABLE Output`。

## 安装步骤

1. 有仓库权限的人打开 GitHub Actions run 页面。
2. 下载 `echoless-windows-X64` artifact。
3. 如果要发给朋友,可以直接转发整个 artifact zip;也可以自己解压后只转发里面的 Echoless `.exe` 安装器。
4. 在 Windows 电脑上运行 Echoless `.exe` 安装器。
5. 如果 Windows 提示未知发布者,选择继续运行。
6. 启动 Echoless。

如果还没安装 VB-CABLE,先安装 VB-CABLE,安装后重新打开 Echoless 或刷新设备列表。

## 推荐首次配置

在 Echoless 里:

1. `Input` 选择真实麦克风。
2. `Reference` 选择系统播放声音或当前扬声器对应的 reference。
3. `Output` 选择 `CABLE Input`。
4. `Model` 选择 `AEC3`。
5. `Noise` 先关闭或设为 low。
6. `Reference Channels` 先用 mono。
7. `Sample Rate` 用 48000。
8. `Frame` 用 10 ms。
9. `Output Level` 保持 50。

在 Discord / VRChat / 语音软件里:

1. 麦克风选择 `CABLE Output`。
2. 播放设备继续选择正常音箱/耳机,不要选择 `CABLE Input`。
3. 关闭软件自带的强降噪、自动增益、回声消除后先测一次;之后可以再打开软件自带处理做对比。

重要:不要把 Echoless 的 `Output` 选成物理音箱。那会把处理后的人声播放到音箱里,可能重新进入麦克风,造成回授或啸叫。

## 主测试:AEC3 外放回声消除

测试时让电脑音箱播放音乐、视频对白或游戏声音,同时对着麦克风说话。让对方在 Discord / VRChat 里听,或用语音软件的测试录音功能听回放。

需要观察:

1. Echoless 是否能启动,没有报错。
2. 说话时 mic 波形/电平有变化。
3. 播放系统声音时 ref 波形/电平有变化。
4. 说话时 out 波形/电平有变化。
5. 对方能听到你的声音。
6. 对方听到的系统播放声是否明显变小。
7. 你说话和外放同时存在时,你的声音是否被吞、忽大忽小、变电音或有锯齿感。
8. 是否有短暂断音、卡顿、爆音、啸叫。
9. 语音延迟是否可接受。

建议测试三段:

1. 只播放外放,不说话:对方应尽量听不到或只听到很小残留。
2. 只说话,不播放外放:对方应听到自然人声。
3. 外放和说话同时存在:对方应听到人声,外放回声应被压低,人声不能明显忽大忽小。

## 可选测试:Output Level

`Output Level` 是最终输出音量:

- 0 = 静音。
- 50 = 原声电平。
- 100 = 约 3 倍增益,后端有软限幅保护。

测试方法:

1. 运行中把 `Output Level` 从 50 调到 40、60、75。
2. 对方确认音量是否实时变化。
3. 调高后确认没有明显破音或刺耳限幅。
4. 调到 0 确认语音软件收到静音。

## 可选测试:LocalVQE

LocalVQE 是独立可选后端,不是默认主线。它使用打包内置的 v1.3 GGUF 模型和 native runtime。

测试方法:

1. 先完成 AEC3 测试。
2. 停止当前处理。
3. 把 `Model` 切到 `LocalVQE`。
4. 如果 UI 提示模型缺失或需要下载,记录截图;正常安装包应已经带默认模型。
5. 用同样的三段测试试听。

需要重点反馈:

- 是否能启动。
- 是否明显降回声。
- 人声是否更自然或更失真。
- 是否比 AEC3 更容易卡顿、断音或延迟变大。

## 可选测试:NVIDIA RTX AEC

RTX AEC 只适合 Windows + NVIDIA RTX 显卡用户。它依赖 NVIDIA AFX runtime/model,不应作为普通用户首次测试的必选项。

推荐引导:

1. 没有 NVIDIA RTX 显卡时,忽略这个选项。
2. 有 RTX 显卡时,先跑 AEC3 主测试。
3. 进入 RTX/NVAFX 设置页或诊断页查看 runtime 是否 ready。
4. 如果提示需要 runtime/model,按 UI 引导安装;不要手动复制未知 DLL。
5. RTX AEC 单独测试,不要和 AEC3/LocalVQE 混合评价。

需要反馈:

- GPU 型号。
- RTX runtime 是否 ready。
- 启动是否成功。
- 人声保真度、回声残留、延迟、卡顿情况。

## 出问题时收集什么

请尽量回传这些信息:

1. Windows 版本。
2. 麦克风型号。
3. 播放设备型号。
4. 虚拟音频设备名称,例如 VB-CABLE。
5. Echoless 里选择的 Input / Reference / Output。
6. 语音软件里选择的麦克风。
7. 当前 Model、Noise、Reference Channels、Output Level。
8. 是否 ref 电平会随系统播放声音变化。
9. 是否 out 电平会随说话变化。
10. 主观反馈:回声消除程度、人声自然度、断音、延迟、啸叫。
11. 如果诊断页能录制,录 10-20 秒并把生成的 diagnostics 文件夹发回。

## 已知限制

- 当前仍依赖外部虚拟音频设备,安装器不会自动安装 VB-CABLE。
- 选择错误的 output 设备可能造成回授或啸叫。
- AEC3 默认是保真人声优先,不是最强降噪模式。
- LocalVQE 是实验性可选后端,音质不一定优于 AEC3。
- RTX AEC 是 Windows RTX-only,还受 NVIDIA runtime/model 安装状态影响。
