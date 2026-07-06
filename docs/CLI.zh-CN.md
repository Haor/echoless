# `echoless` CLI reference

[English](CLI.md) | 简体中文

CLI 完全独立运行——桌面应用只是它的前端。用 `cargo build --release`
构建(二进制位于 `target/release/echoless`),或直接使用随应用可执行文件
一同分发的副本。

每个检查类命令都接受 `--json` 以输出机器可读的结果。

```
echoless <COMMAND>

  offline      process mic.wav + ref.wav through the chain → out.wav
  processors   list engines and their parameter manifests
  devices      list audio devices and reference sources
  doctor       environment diagnostics
  config       config file tools (validate)
  run          realtime pipeline
  probe-delay  measure mic↔reference alignment delay
  nvafx        NVIDIA RTX AEC runtime tools (doctor / install / offline)
```

## devices

```bash
echoless devices --json          # inputs, outputs, reference_sources
echoless devices --json --fast   # skip slow queries (GUI refresh path)
```

其他命令所用的设备选择符接受 `default`、列表索引、名称片段,或本命令
输出中的 `stable_id`。参考源选择符:`system`(操作系统 loopback/tap)、
`none`、`output:<name>`、`input:<name>`。

## run

```bash
echoless run --mic default --reference system --output "CABLE Input"
echoless run --config my.toml --status-json
```

主要 flag(均覆盖配置文件):`--mic`、`--reference`、
`--output`、`--sample-rate`、`--frame-ms`、`--reference-channels mono|stereo`、
`--near-delay-ms`、`--output-level 0..100`(50 = 原始音量)、
`--processor aec3|localvqe|nvidia_afx_aec|…`、`--ns/--no-ns`、`--ns-level`、
`--tail-ms`、`--verbose`、`--status-json`、
`--diagnostic-dir <DIR> [--diagnostic-seconds N]`。

加上 `--status-json` 后,stdout 输出 JSONL:首先是一个 `started` 事件
(协商后的设备、`supported_controls`、重采样信息),随后是周期性的状态帧
(dBFS 电平、延迟、引擎指标),以及针对 runtime 控制指令的确认事件。
人类可读的日志走 stderr。

### Runtime controls

`run` 运行期间,向 stdin 每行写入一个 JSON 对象:

| Command | Payload | Effect |
|---|---|---|
| `set_output_level` | `{"cmd":"set_output_level","level":50}` | 实时输出增益(0 静音 · 50 原始音量 · 100 ≈ 3×) |
| `set_near_delay_ms` | `{"cmd":"set_near_delay_ms","ms":25}` | 实时 near/far 对齐 |
| `set_bypass` | `{"cmd":"set_bypass","bypass":true}` | 跳过引擎但保持其热态(15 ms 交叉淡化) |
| `set_initial_delay_ms` | `{"cmd":"set_initial_delay_ms","ms":8}` | AEC3 初始延迟提示 |
| `set_aec3_ns` | `{"cmd":"set_aec3_ns","ns":true,"ns_level":"high"}` | AEC3 噪声抑制 |
| `set_aec3_agc` | `{"cmd":"set_aec3_agc","agc":false}` | AEC3 AGC |
| `set_localvqe_noise_gate` | `{"cmd":"set_localvqe_noise_gate","enabled":true,"threshold_dbfs":-45}` | LocalVQE 输出门限 |
| `start_diagnostics` | `{"cmd":"start_diagnostics","dir":"...","max_seconds":30}` | 录制 mic/ref/out WAV |
| `stop_diagnostics` | `{"cmd":"stop_diagnostics"}` | 结束本次录制会话 |

`started` 事件中的 `supported_controls` 数组是判断某个二进制究竟接受
哪些控制指令的权威依据。

## probe-delay

通过扬声器播放一列蜂鸣声、同时录制两条通路,再逐个蜂鸣做互相关
(0.5 ms 包络分辨率),测量真实的 near↔far 延迟:

```bash
echoless probe-delay --json --mic default --reference system --output "CABLE Input"
```

该命令接受 `default` 选择符,但其 clap 默认值是刻意面向 macOS 的
(`MacBook Pro...` / `BlackHole 2ch`),用于维护者本地的校准装置。
可移植脚本应显式传入 `--mic`、`--reference` 和 `--output`,而不要依赖
这些默认值。

它本身不会停止任何东西——不要在另一个 `run` 正占用设备时运行它。
Flag:`--beeps N`(12)、`--startup-delay S`(4)、`--volume 0..1`(0.35)、
`--out-dir/--keep-session/--keep-beep`、`--analyze-only <session>`。

JSON 结果包含 `recommended_near_delay_ms`(实测延迟 + 8 ms 安全余量)、
逐蜂鸣延迟、标准差/漂移和告警。在 `--json` 模式下,进度标记以 JSONL
形式发到 stderr(`beep_train_start` 附带精确的蜂鸣节奏)。支持 macOS、
Windows 和 Linux(Linux 会把 monitor 参考映射回其 sink 以完成播放)。

## offline

用任意引擎做 WAV 进 / WAV 出的处理——适合 A/B 测试和 CI:

```bash
echoless offline --mic mic.wav --reference ref.wav --out clean.wav --chain aec3
echoless offline --mic mic.wav --reference ref.wav --out clean.wav --config my.toml
```

`offline` 会校验处理器拓扑和 WAV 进 / WAV 出的行为。它不模拟实时设备
边界、`near_delay_ms`、bypass 交叉淡化、队列背压,或设备采样率转换通路,
因此不应把它当作精确的实时延迟或实时路由基准。

## doctor / processors / config

```bash
echoless doctor audio --json    # virtual device present? reference OK? permissions?
echoless processors --json      # engine manifest (params, platforms, defaults)
echoless config validate my.toml
```

## nvafx (Windows + RTX)

```bash
echoless nvafx doctor --json               # GPU / driver / VC++ / runtime checks
echoless nvafx download-install --json     # fetch runtime + model for this GPU (~1 GB)
echoless nvafx install --common-zip <common.zip> --model-zip <model.zip>  # install from local zips
echoless nvafx offline --mic ... --reference ... --out ...
```

## Configuration file

`run`/`offline` 接受一份 TOML 流水线配置——参见
[`configs/example.toml`](../configs/example.toml),那是一份带注释的示例,
涵盖设备、pipeline(`sample_rate`、`frame_ms`、`near_delay_ms`、
`reference_channels`),以及带各引擎参数的 `[[chain]]` 引擎块。

## Environment variables

| Variable | Purpose |
|---|---|
| `ECHOLESS_PROCESS_TAP_HELPER` | macOS 系统音频辅助程序的路径(否则:先找二进制旁边,再从 CWD 向上找 `tools/macos-process-tap-poc/.build/…`) |
| `ECHOLESS_LOCALVQE_LIBRARY` | `liblocalvqe` 的路径(否则:先找应用 bundle 资源,再找品牌数据目录) |

模型/数据目录:`~/Library/Application Support/Echoless`(macOS)、
`%LOCALAPPDATA%\Echoless`(Windows)、`~/.local/share/echoless`(Linux)。
