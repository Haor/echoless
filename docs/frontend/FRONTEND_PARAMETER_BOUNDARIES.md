# Frontend Feature and Parameter Boundaries

本文档给 Echoless GUI/Tauri 前端使用,定义当前产品功能边界、参数 contract、推荐暴露层级和不应暴露/不存在的选项。本文档不规定 UI/UX 设计,只规定前端生成配置和展示能力时不能越过的后端边界。

## Source of Truth

前端实现按以下优先级判断功能和参数是否可用:

1. `echoless processors --json`: processor kind、平台、参数类型、默认值、可选值、`advanced`、约束。
2. `echoless config validate --config <file> --json`: 配置是否被当前后端接受。
3. `echoless devices --json`: 当前机器真实可用的输入、输出、reference source。
4. `configs/example.toml`: 人类可读的推荐默认配置。
5. 本文档和 `FRONTEND_AGENT_HANDOFF.md`: 产品边界和前端接入规则。

Open Design / HTML prototype 只能作为视觉原型,不能作为配置 contract。若原型里的选项和 `processors --json` 或本文档冲突,以后端 manifest 为准。

## Product Capability Boundary

### 默认主路径

- `aec3` 是默认 backend。
- 默认管线为 `48000 Hz / 10ms frame / mono reference`。
- 默认音质策略是保真人声优先: AEC 开启,NS/AGC 默认关闭。
- 输出依赖外部虚拟音频设备,例如 Windows VB-CABLE、macOS BlackHole 2ch 或 VB-CABLE MAC。
- 前端应支持设备枚举、运行/停止、runtime status、diagnostics 录制、配置校验和虚拟音频设备安装状态提示。

### 可选实验能力

- `localvqe` 是独立可选 backend,需要 GGUF 模型和 LocalVQE 动态库。它不是默认主路径。
- `nvidia_afx_aec` 是 Windows-only RTX AEC backend,需要 `echoless nvafx doctor --json` 通过。它不是 NVIDIA Broadcast App。
- `passthrough` 可用于诊断链路,不是普通用户默认消回声方案。

### 当前不作为产品主路径

- 不默认提供 `AEC3 -> LocalVQE` 级联。此前听感反馈显示级联更容易加重电音/锯齿,当前产品保真优先。
- 不自研/内置原生虚拟麦克风驱动。首版使用外部虚拟音频设备。
- 不做原生 HAL 作为首版重点。当前 CPAL/系统音频路径没有证据表明已成为瓶颈。
- 不把 macOS `system` reference 视为天然可用 loopback。macOS 系统声参考通常需要 BlackHole/VB-CABLE MAC 等路由设备。

## Recommended Exposure Tiers

### 常规用户可见

这些能力适合放在普通设置或主流程中:

| 参数/能力 | 默认 | 边界 |
|---|---:|---|
| `mic` | `default` | 只能来自 `devices --json` 的 input 或后端支持的 selector。 |
| `reference` | `system` | Windows 可用 `system`;macOS 需按 `devices --json` 和路由设备判断。 |
| `output` | `default` | 推荐选择虚拟音频 output;只能列出实际枚举到的 output。 |
| backend select | `aec3` | 只展示 `processors --json` 返回且当前平台可用的 kind。 |
| `reference_channels` | `mono` | 可选 `mono`/`stereo`;默认 mono;RTX AEC 只能 mono。 |
| `output_level` | `50` | 全局最终输出电平,不是处理器参数。范围 `0..100`;`0` 静音,`50` 原声,`100` 约 `3x` 增益。曲线为 `gain = (output_level / 50)^log2(3)`,后端在所有处理器之后统一应用并做软限幅保护。 |
| `ns` | `false` | AEC3 内置降噪;用户需要时可开启;运行中可热控。 |
| `diagnostics.record_dir` | null | 可让用户选择保存目录。 |
| `diagnostics.max_seconds` | null | 可提供 30s/45s 等录制时长。 |

### 常规可见但需要约束

| 参数 | 默认 | 暴露规则 |
|---|---:|---|
| `ns_level` | `low` | 仅在 `ns=true` 时有效;可选值只能是 `low`、`moderate`、`high`、`veryhigh`。UI 可显示 "very high",但提交值必须是 `veryhigh`;运行中可热控。 |
| `sample_rate` | `48000` | 这是管线采样率,首版建议锁定或放高级设置。RTX AEC 必须是 `48000`;AEC3 推荐 `48000`;不要把 `44.1k`/`96k` 作为普通推荐项。真实设备不是 48k 时由后端 I/O 重采样处理。 |
| `frame_ms` | `10` | 首版建议锁定或放高级设置。RTX AEC 必须是 `10`;AEC3 推荐 `10`;不要把 `20ms` 作为普通推荐项。 |
| `near_delay_ms` | macOS `25`,其他平台 `0` | 高级/校准项。范围 `0..500`;运行中可热控。主动延迟侦测只有在推荐值大于 0 时才写入。 |

### 高级设置

这些参数真实存在,但普通用户误调收益低、风险高:

| 参数 | 默认 | 边界 |
|---|---:|---|
| `agc` | `false` | 保真优先默认关闭;可能造成音量泵动或双讲忽大忽小;运行中可热控。 |
| `initial_delay_ms` | null | AEC3 初始延迟 hint;范围 `0..500`;运行中可热控。运行时仍会动态估计回声对齐。 |
| `tail_ms` | null | AEC3 echo tail 长度;最小值 4;运行中不可热控,修改需要重启 runtime。 |
| `delay_num_filters` | null | AEC3 延迟搜索窗;最小值 1;运行中不可热控,修改需要重启 runtime。 |
| `linear_stable_echo_path` | `false` | AEC3 高级调参项;运行中不可热控,修改需要重启 runtime。 |

### 主动延迟侦测可写入的参数

`echoless probe-delay --json` 会测量 macOS Process Tap 或 Windows WASAPI loopback
reference 与 mic 的相对到达时间。完整调用、
字段解释和判读规则见 `docs/frontend/NEAR_DELAY_PROBE_HANDOFF.md`。

它的结果只应直接写入:

- 顶层 `near_delay_ms`: 当测量稳定时使用 `recommended_near_delay_ms`。该值已包含平台默认
  负向搜索偏置:macOS 默认 `25ms`;Windows/Linux 默认 `20ms`。运行中的手动修改可热控;probe
  本身仍需要暂停主 runtime,因为它要独占设备播放蜂鸣和录制。

可以显示但不要默认自动改:

- AEC3 `initial_delay_ms`: 可把 `max(0, event_lag_mean_ms + recommended_near_delay_ms)`
  作为高级 hint,但 AEC3 本身会动态估计,默认不需要写。若用户手动调整,运行中可热控。
- AEC3 `delay_num_filters`: 只有多次 probe 都显示延迟非常稳定、且后续实测需要降低 CPU/收敛范围时才考虑。

不能由这次侦测推导:

- `tail_ms`: 取决于房间反射和扬声器/麦克风环境。
- `sample_rate` / `frame_ms` / `reference_channels`: 由处理器约束和设备能力决定。
- LocalVQE / RTX AEC 的内部参数:当前没有可暴露的 delay hint;只使用顶层 `near_delay_ms`。

### LocalVQE 仅在选择该 backend 后暴露

| 参数 | 默认 | 边界 |
|---|---:|---|
| `model` | required | GGUF 模型路径,必须非空。 |
| `library` | auto | 动态库路径,可为空让后端自动查找。 |
| `threads` | auto | 数字,最小值 1;不填表示上游 auto。 |
| `backend` | auto | 字符串 hint,例如上游 runtime 支持的 backend 名。 |
| `device` | auto | 数字 device index;不要把 `auto/cpu/gpu` 写入 `device`。 |
| `noise_gate` | `false` | 默认关闭;开启可能吃掉轻声和尾音;运行中可热控。 |
| `noise_gate_threshold_dbfs` | `-45.0` | 数字阈值;建议作为高级项;运行中可热控。 |

LocalVQE 的 native 处理边界是 16 kHz mono,但 GUI 不应因此把全局 `sample_rate` 改成 16 kHz。当前链路会在 processor 边界做适配。

### RTX AEC 仅 Windows 可用

| 参数 | 默认 | 边界 |
|---|---:|---|
| backend kind | `nvidia_afx_aec` | `rtx_aec` 不是有效 kind。 |
| `runtime_dir` | auto | NVIDIA AFX runtime 目录。 |
| `model_path` | auto | RTX AEC model 路径。 |
| `intensity_ratio` | `1.0` | 数字,最小值 0。 |
| `use_default_gpu` | `true` | 高级项。 |
| `disable_cuda_graph` | `false` | 高级项。 |
| `on_runtime_error` | `silence` | 只能是 `silence` 或 `bypass`。 |

RTX AEC v1 硬约束:

- `sample_rate = 48000`
- `frame_ms = 10`
- `reference_channels = "mono"`
- Windows + RTX GPU + `nvafx doctor --json` 通过

macOS/Linux 上前端可以展示为 unsupported,但不应生成可运行的 `nvidia_afx_aec` 配置。

## Do Not Expose as User Parameters

以下内容可以作为 diagnostics/status 展示,但不要作为稳定用户配置项:

- ring buffer 大小和内部队列阈值。
- stale drop 阈值。
- CPAL stream 内部 buffer 选择。
- AEC3 suppressor、nearend detector、matched filter 等内部结构。
- diagnostics 文件轮转细节。
- 原始设备 index 作为唯一持久配置。可以保存 selector,但应同时保存设备名称用于下次辅助匹配。

## Known Invalid or Misleading Options

如果前端从旧原型继承了这些值,需要修正:

| 原型/错误值 | 正确值或处理 |
|---|---|
| `rtx_aec` backend kind | 使用 `nvidia_afx_aec`。 |
| "NVIDIA Broadcast AFX" | 写作 NVIDIA AFX / RTX AEC SDK backend;不要等同于 Broadcast App。 |
| `ns_level = "very"` | 使用 `veryhigh`。 |
| LocalVQE `device = "auto" \| "cpu" \| "gpu"` | `device` 是数字;CPU/GPU 类 hint 应走 `backend` 字符串或隐藏。 |
| macOS `系统输出 (loopback)` 永远可用 | 必须以 `devices --json` 和实际路由为准;通常需要 BlackHole/VB-CABLE MAC。 |
| 硬编码 `Virtual Desktop Mic` 为 output | 只有枚举到可写 output endpoint 才能选择。 |
| 普通设置中推荐 `44.1k`、`96k`、`20ms` | 首版默认/推荐应是 `48k / 10ms`;其他值放高级并用 validate 校验。 |

## Runtime Rules for Frontend

- 每次保存或应用配置前,运行 `echoless config validate --config <file> --json`。
- 切换 backend、设备、采样率、frame、模型或 RTX runtime 后,重启 runtime。
- 运行中可热控参数以 `started.supported_controls` 为准。当前后端支持
  `output_level`、`near_delay_ms`、AEC3 `initial_delay_ms`、AEC3 `ns/ns_level`、AEC3 `agc`、
  LocalVQE `noise_gate/noise_gate_threshold_dbfs` 的 stdin runtime control;如果
  `supported_controls` 缺少对应命令,前端应提示 CLI 过旧,不要静默降级为重启。
- 运行时展示 `estimated_user_latency_ms` 和 `aec_estimated_delay_ms` 时要区分语义:
  - `estimated_user_latency_ms`: Echoless 软件管线内的用户说话到虚拟输出前估算延迟;不含设备硬件缓冲、通话软件缓冲或网络延迟。首页建议标为 `Pipeline` / `管线延迟`。
  - `aec_estimated_delay_ms`: AEC3 估计的回声路径对齐延迟。
  - `aec3_delay_blocks`: AEC3 内部延迟估计块数,类型为 `Option<u32>`;长期贴 0 下限表示真实 lag 可能比当前 `near_delay_ms` 偏置更负。
  - `near_delay_ms`: 我们主动加入的 near/mic 对齐延迟,会计入 `estimated_user_latency_ms`。
  - `input_queue_latency_ms` / `output_queue_latency_ms`: 输入/输出队列积压贡献的软件管线延迟。
- diagnostics 录制应保存 `mic.wav`、`ref.wav`、`out.wav`、`stats.csv` 和 metadata,用于用户反馈和回归分析。
