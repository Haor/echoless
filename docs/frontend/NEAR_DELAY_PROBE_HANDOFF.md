# Near Delay Probe Handoff

本文档说明 Echoless 主动 near-delay 诊断/校准能力的调用方式、结果含义、macOS 默认
`near_delay_ms = 25` 的来源,以及测量结果可以转成哪些推荐配置。该能力面向 macOS 和
Windows;macOS 主要用于 Process Tap 校准,Windows 主要用于 WASAPI loopback 链路诊断。

## 适用范围

`echoless probe-delay` 当前是项目原生 CLI 能力,不是独立脚本。它用于测量 reference
与 near-end mic 进入处理器前的相对到达时间。

当前边界:

- 支持 macOS `reference=system` Process Tap。
- 支持 Windows `reference=system` WASAPI loopback。
- 只分析 48 kHz diagnostics。
- 命令内部会用 `near_delay_ms = 0` 启动子进程,避免已配置的补偿影响测量。
- 完整 probe 会播放可听蜂鸣声;默认使用临时 diagnostics session,分析结束后清理。
- 需要复查 WAV/CSV 时,显式传 `--keep-session` 或 `--out-dir` 保留 session。
- 无法外放时只能用 `--analyze-only` 分析已有 session。
- 输出的推荐值主要用于 Echoless 顶层 `near_delay_ms`,不是 AEC3 动态延迟估计值。

## 为什么需要 Near Delay

AEC 的输入有两路:

```text
mic/near: 用户声音 + 房间回声
ref/far : 系统正在播放的参考声音
```

理想情况下,同一个播放事件在 `ref` 里出现后,经过扬声器、空气、麦克风链路,稍晚出现在
`mic` 里。但 macOS Process Tap 的真实链路里,reference 本身可能比 mic 进入 Echoless
更晚。这样处理器看到的是:

```text
mic 先到
ref 后到
```

如果不补偿,reference 和 mic 的时间轴会错开,AEC3 虽然有内部动态估计,但起步更难,也更容易
把搜索范围花在错误位置。`near_delay_ms` 的作用很直接:在处理器前把 mic/near 人为延后,
让两路信号更接近同一时间轴。

## 为什么 macOS 默认是 25ms

本机 Process Tap 实测样本中,蜂鸣事件的平均 lag 约为:

```text
event_lag_mean_ms = -18.5
recommended_near_delay_ms = 25
```

负数表示 mic/near 比 reference 更早到。推荐值的计算逻辑是:

```text
if event_lag_mean_ms < 0:
  recommended = round_to_5ms(abs(event_lag_mean_ms) + 8ms safety)
else:
  recommended = 0
```

`-18.5ms + 8ms safety` 约等于 `26.5ms`,按 5ms 粒度取整后得到 `25ms`。所以 macOS 默认
`near_delay_ms = 25` 不是经验拍脑袋,而是当前 Process Tap 路径上测出的保守起点。

Windows/Linux 默认仍是 `0ms`,因为这 25ms 只对应 macOS Process Tap 参考路径。不同设备、
不同 macOS 版本、不同 helper 打包方式可能改变这个值,所以 GUI 可以提供校准入口。

Windows 上不继承 macOS 的 `25ms` 默认。Windows probe 的第一目标是确认 WASAPI loopback
reference、mic echo 和链路稳定性;只有测出稳定的负 lag(reference 晚于 mic)时,才推荐写入
`near_delay_ms`。

## 完整调用

默认调用:

```bash
echoless probe-delay --json
```

指定设备:

```bash
echoless probe-delay --json \
  --mic "MacBook Pro麦克风" \
  --reference system \
  --output "BlackHole 2ch"
```

常用参数:

| 参数 | 默认 | 含义 |
|---|---:|---|
| `--mic` | `MacBook Pro麦克风` | near-end 麦克风 selector |
| `--reference` | `system` | macOS Process Tap 或 Windows WASAPI loopback reference |
| `--output` | `BlackHole 2ch` | Echoless 输出设备,建议虚拟音频设备 |
| `--out-dir` | 空 | 保留 diagnostics session 的输出根目录;不传则临时落盘并清理 |
| `--keep-session` | `false` | 未指定 `--out-dir` 时也保留本次 diagnostics session |
| `--startup-delay` | `4` | 播放蜂鸣前等待 realtime 管线稳定的秒数 |
| `--beeps` | `12` | 蜂鸣事件数量 |
| `--volume` | `0.35` | 蜂鸣音量,范围 `0.0..1.0` |
| `--keep-beep` | 空 | 保留生成的蜂鸣 WAV 到指定路径 |
| `--analyze-only` | 空 | 只分析已有 diagnostics session |
| `--json` | `false` | 输出机器可读 JSON |

分析已有 session:

```bash
echoless probe-delay --json \
  --analyze-only /tmp/echoless-near-delay-beep-probe/session-1781016666
```

`--analyze-only` 不会播放声音,只读取该目录下的 `mic.wav` 和 `ref.wav`。

## JSON 结果

示例:

```json
{
  "session_dir": "/tmp/echoless-near-delay-probe-123/session-1781016666",
  "session_retained": false,
  "ref_dbfs": -25.24,
  "mic_dbfs": -25.69,
  "global_lag_ms": -18.0,
  "global_corr": 0.8174,
  "event_count": 12,
  "event_detected": 12,
  "event_lag_mean_ms": -18.5,
  "event_lag_stddev_ms": 0.65,
  "event_lag_drift_ms": -1.5,
  "recommended_near_delay_ms": 25,
  "per_beep_lags": [
    { "index": 1, "time_s": 0.5, "lag_ms": -18.0, "corr": 0.9 }
  ],
  "warnings": []
}
```

字段含义:

| 字段 | 含义 | 判读 |
|---|---|---|
| `session_dir` | 本次 diagnostics session 目录 | `session_retained=false` 时进程退出后已清理 |
| `session_retained` | 本次 session 是否保留在磁盘 | 为 `true` 时才显示“打开目录” |
| `ref_dbfs` | reference RMS 电平 | 太低说明系统蜂鸣没有被 reference 捕获 |
| `mic_dbfs` | mic RMS 电平 | 太低说明麦克风没有收到蜂鸣 |
| `global_lag_ms` | 全局包络相关估计 lag | 辅助参考 |
| `global_corr` | 全局相关强度 | 太低说明信号不够可靠 |
| `event_count` | 有效参与统计的蜂鸣数量 | 越接近 `event_detected` 越可靠 |
| `event_detected` | 从 ref 里找到的蜂鸣数量 | 少于预期时不要自动应用 |
| `event_lag_mean_ms` | 单个蜂鸣 lag 的平均值 | 推荐计算的主输入 |
| `event_lag_stddev_ms` | 单个蜂鸣 lag 的标准差 | 越小越稳定 |
| `event_lag_drift_ms` | 最后一个有效蜂鸣 lag 减第一个 | 绝对值大说明链路可能漂移 |
| `recommended_near_delay_ms` | 建议写入的顶层 near delay | GUI 可作为推荐项 |
| `per_beep_lags` | 每个蜂鸣的 lag/corr 明细 | 调试和趋势展示用 |
| `warnings` | 后端判定的可靠性警告 | 非空时不要自动应用 |

lag 符号约定:

- `event_lag_mean_ms < 0`: mic/near 早于 reference,可以用 `near_delay_ms` 延后 near。
- `event_lag_mean_ms >= 0`: reference 没有晚到,推荐 `near_delay_ms = 0`。

这里的“早/晚”指 Echoless 录到的 `mic.wav` 相对 `ref.wav` 的时间,不是物理世界里扬声器
和麦克风的绝对先后。物理上当然是扬声器先发声、麦克风后收到;但 Process Tap / loopback
reference 流可能因为系统音频链路更晚进入 Echoless,从而出现 `mic.wav` 里的蜂鸣早于
`ref.wav` 的情况。

前端可把它翻译成更直白的产品规则:

- reference 晚于 mic,也就是 `event_lag_mean_ms < 0`: 自动填入 `recommended_near_delay_ms`。
- reference 早于或等于 mic,也就是 `event_lag_mean_ms >= 0`: `near_delay_ms` 保持 `0`,
  只显示诊断信息。

## 正负 Lag 的配置策略

统一定义:

```text
event_lag_mean_ms = mic 中蜂鸣出现的时间 - ref 中蜂鸣出现的时间
```

### mic 早于 reference

当 `event_lag_mean_ms < 0` 时,AEC 看到的是:

```text
mic echo 先到
reference 后到
```

这是最不理想的方向,因为因果实时 AEC 不能用“未来才到的 reference”消掉已经进入 mic 的
回声。此时应把 near/mic 人为延后:

```toml
near_delay_ms = <recommended_near_delay_ms>
```

例子:

```text
event_lag_mean_ms = -18.5
recommended_near_delay_ms = 25
post_alignment_lag = -18.5 + 25 = +6.5ms
```

应用后,处理器看到的是 reference 先到、mic echo 稍后到,方向变成 AEC 更容易处理的状态。

### mic 晚于 reference

当 `event_lag_mean_ms > 0` 时,AEC 看到的是:

```text
reference 先到
mic echo 后到
```

这是正常方向。此时不要再主动延后 near/mic:

```toml
near_delay_ms = 0
```

如果继续加 `near_delay_ms`,只会让 mic 更晚,增加用户感知延迟,通常不会改善 AEC。
这个正 lag 可以作为 diagnostics 信息展示,但不要默认写入 AEC3 `initial_delay_ms`。

Windows 上大多数正常结果应该落在这一类:WASAPI loopback reference 先到,mic echo 后到。
此时 probe 的作用是健康检查,不是调参。

### 接近 0 或不稳定

当 `event_lag_mean_ms` 接近 0 时,两路已经基本对齐,默认保持:

```toml
near_delay_ms = 0
```

如果 `event_lag_stddev_ms` 或 `event_lag_drift_ms` 明显偏大,说明链路不稳定。此时不要自动
应用推荐值,应检查 reference 捕获、设备采样率、系统权限、输出路由和 diagnostics 文件。

## AEC 理想目标

对 AEC 来说,最理想的不是固定“mic 比扬声器延迟多少毫秒”,而是:

```text
reference 先进入处理器
mic 中对应 echo 稳定地稍后进入处理器
```

换成 probe 结果,修正后的目标是:

```text
post_alignment_lag = event_lag_mean_ms + near_delay_ms
```

理想目标:

- `post_alignment_lag >= 0`
- 数值稳定,不要随时间明显漂移
- 通常落在 `0..20ms` 的小正数区间比较舒服

`post_alignment_lag` 不是越大越好。更大的正数会让 AEC3 仍然能搜索,但不代表音质更好;
如果是由 `near_delay_ms` 主动造成,还会直接增加用户说话到虚拟输出的延迟。macOS 默认
`25ms` 的目的不是追求 25ms echo delay,而是把实测 `-18.5ms` 修正成约 `+6.5ms`。

## 可写入的推荐项

### 直接推荐

只建议直接写入一个配置:

```toml
near_delay_ms = <recommended_near_delay_ms>
```

应用后需要重启 runtime。这个值会进入用户感知延迟估算:

```text
estimated_user_latency_ms =
  frame_ms / 2
  + near_delay_ms
  + algorithmic_latency_ms
  + output_queue_latency_ms
```

### 可显示但不默认写入

AEC3 `initial_delay_ms` 可以把后对齐残余作为高级 hint:

```text
post_alignment_residual_ms = max(0, event_lag_mean_ms + recommended_near_delay_ms)
```

但默认不要自动写入,因为 AEC3 运行时会动态估计回声路径延迟。这个值更适合调试页面显示,
或给高级用户手动参考。

AEC3 `delay_num_filters` 不建议从单次 probe 自动推导。只有当多次测量都稳定,并且后续
diagnostics 证明需要缩小搜索窗或降低 CPU 时,才考虑作为高级实验项。

### 不能由本次 probe 推导

以下参数不能由 near-delay probe 决定:

- AEC3 `tail_ms`: 取决于房间反射、扬声器和麦克风摆位。
- `sample_rate`: 由处理器约束和设备能力决定,默认推荐仍是 `48000`。
- `frame_ms`: 由实时延迟和处理器约束决定,默认推荐仍是 `10`。
- `reference_channels`: 由 reference 声道策略决定,默认推荐仍是 `mono`。
- LocalVQE 内部参数:当前只使用顶层 `near_delay_ms`。
- RTX AEC 内部参数:当前只使用顶层 `near_delay_ms`。

## 前端接入规则

前端可以把 `probe-delay --json` 作为“参考/麦克风延迟诊断”的后端能力。macOS 可展示为
near-delay 校准;Windows 建议展示为 AEC 链路延迟诊断,只有负 lag 时才自动填
`near_delay_ms`。建议契约如下:

- 调用完整 probe 前,确认用户允许播放短蜂鸣声。
- 如果 `warnings` 非空,不要自动应用推荐值。
- 如果 `event_detected` 明显少于 `--beeps`,不要自动应用推荐值。
- 如果 `global_corr` 很低,不要自动应用推荐值。
- 如果 `event_lag_stddev_ms` 或 `event_lag_drift_ms` 明显偏大,提示结果不稳定。
- 当 `recommended_near_delay_ms > 0` 且结果稳定时,可自动填入顶层 `near_delay_ms`,然后重启 runtime。
- 当 `recommended_near_delay_ms = 0` 时,不要改配置,只展示诊断信息。
- 运行中展示时区分 `near_delay_ms`、`estimated_user_latency_ms`、`aec_estimated_delay_ms`。

推荐的保存口径:

```toml
# macOS Process Tap recommended value from probe-delay.
near_delay_ms = 25
```

## 验收标准

一次可信 probe 至少应满足:

- `warnings` 为空。
- `event_detected` 接近蜂鸣数量。
- `event_count` 大于 0,且接近 `event_detected`。
- `ref_dbfs` 和 `mic_dbfs` 不过低。
- `event_lag_stddev_ms` 处于低水平。
- 多次运行得到的 `recommended_near_delay_ms` 不大幅跳变。

如果 probe 结果可信但 AEC 仍差,下一步看 diagnostics 的 `ref.wav/mic.wav/out.wav/stats.csv`,
而不是继续调 `near_delay_ms`。
