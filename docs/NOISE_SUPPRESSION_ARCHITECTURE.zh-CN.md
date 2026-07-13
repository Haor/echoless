# 通用降噪节点架构设计

状态:架构与 AEC3 迁移策略已拍板,待实现  
适用版本:1.1.0 之后  
实验产物:`audit/ns-node-simulation/`(不进入 Git)

## 1. 目标

Echoless 将回声消除模型与二级降噪拆成两个独立选择层:

```text
MODEL                         NS
AEC3 / LVQE / NVAFX    ->     WEBRTC / RNNOISE / OFF    ->    output
```

MODEL 负责回声消除。NS 是互斥的可选后处理节点:

- `WEBRTC`:WebRTC 经典背景噪声抑制器。
- `RNNOISE`:RNNoise 神经网络背景噪声抑制器。
- `OFF`:不加载外接降噪节点。

这项改造的目标是让 AEC3、NVAFX 和纯 AEC 版 LocalVQE 共用同一套降噪选择,
同时从状态机层面禁止 LocalVQE 内置降噪与外接降噪叠加。

## 2. 非目标

- WebRTC NS 和 RNNoise 不负责替代 reference-aware AEC,也不被定义为残余回声消除器。
- 本轮不改变采集、参考源、输出设备或虚拟麦克风方案。
- 本轮不允许同时串联 WebRTC NS 与 RNNoise。
- 本轮不为 LocalVQE 1.2/1.3 提供关闭模型内置 NS 的能力。
- 本轮不恢复运行目录、模型目录或诊断目录的自定义入口。

## 3. 产品交互

### 3.1 主页面 NS 控件

原有 NOISE `ON/OFF` 改成与 MODEL 同构的三段式控件,不使用下拉列表:

```text
04 NS    [ WEBRTC ] [ RNNOISE ] [ OFF ]
```

交互要求:

- 当前项高亮,再次点击当前项不得重载处理链。
- 可用项可以点击;不可用项置灰且不响应点击、键盘确认或触控。
- 三个选项互斥,任何时刻只能有一个 `NoiseMode`。
- `OFF` 的准确含义是“不加载外接 NS 节点”。

建议使用独立的前端领域类型,不要继续复用 AEC3 的 `ns: bool`:

```ts
type NoiseMode = "webrtc" | "rnnoise" | "off";
```

### 3.2 可用性矩阵

| MODEL | 版本 | WEBRTC | RNNOISE | OFF | 约束来源 |
|---|---|---:|---:|---:|---|
| AEC3 | - | 可用 | 可用 | 可用 | AEC 与外接 NS 分离 |
| NVAFX | - | 可用 | 可用 | 可用 | NVAFX 输出可进入通用 NS |
| LVQE | 1.2 | 禁用 | 禁用 | 强制选中 | 模型包含内置 NS |
| LVQE | 1.3 | 禁用 | 禁用 | 强制选中 | 模型包含内置 NS |
| LVQE | 1.4 | 可用 | 可用 | 可用 | 纯 AEC 模型 |

选择 LVQE 1.2 或 1.3 时必须原子完成以下状态转换:

1. 将 `NoiseMode` 设为 `off`。
2. 从目标配置中移除已经存在的通用 NS 节点。
3. 重启处理链时只加载 LocalVQE 节点。
4. 将 `WEBRTC` 和 `RNNOISE` 置灰。
5. 在说明区域显示 `NOISE SUPPRESSION BUILT INTO LVQE`。

从 LVQE 1.2/1.3 切回 1.4 或其他引擎时保持 `OFF`,不自动恢复之前的 NS。
显式选择比隐式恢复更容易理解,也可避免一次模型切换悄悄增加处理和延迟。

### 3.3 LocalVQE 模型标签与悬停说明

引擎页面的版本标签改为能力标签:

```text
v1.2  [NS]
v1.3  [NS]
v1.4  [AEC]
```

- 删除 v1.3 当前的 `STD` 标签。
- v1.2 与 v1.3 标记 `NS`,表示 AEC + 内置噪声抑制。
- v1.4 标记 `AEC`,表示纯 AEC,可以连接通用 NS。
- 模型路径不再作为悬停主文案。路径属于诊断信息,保留在 Diagnostics 或模型目录入口。

悬停说明建议:

| 版本 | 英文说明 | 中文说明 |
|---|---|---|
| v1.2 | `AEC with built-in noise suppression · legacy model` | `AEC + 内置降噪 · 旧版模型` |
| v1.3 | `AEC with built-in noise suppression` | `AEC + 内置降噪` |
| v1.4 | `Pure AEC · supports external noise suppression` | `纯 AEC · 支持外接降噪` |

## 4. 配置与状态模型

### 4.1 规范状态

前端只持有两个产品级选择:

```text
EngineSelection = { kind, params }
NoiseMode       = webrtc | rnnoise | off
```

生成 CLI 配置之前执行一次规范化:

```text
if engine == localvqe && model in [v1.2, v1.3]:
    noise_mode = off
```

该约束必须同时存在于:

- 前端状态转换,提供即时且可理解的 UI。
- 配置生成或后端验证,阻止旧配置、手工配置或竞态绕过 UI 形成双重 NS。

### 4.2 处理链映射

```text
NoiseMode::off
    [engine]

NoiseMode::webrtc
    [engine, webrtc_ns]

NoiseMode::rnnoise
    [engine, rnnoise]
```

LocalVQE 1.2/1.3 的合法链始终是 `[localvqe]`。LocalVQE 1.4、AEC3 与 NVAFX
可以使用三种映射。

配置中应将通用 NS 表达为真正的 processor node,而不是继续把产品状态编码成
AEC3 私有参数。旧的 `aec3.ns` 与 `aec3.ns_level` 在迁移完成后不再作为 GUI 的
真理来源。

## 5. 后端节点设计

### 5.1 统一节点契约

两个 NS 均实现现有 `EchoProcessor` 接口,但忽略 `far`:

```text
near = 上一级 AEC 输出
far  = 由 ProcessorChain 继续传入,NS 节点不读取
out  = 降噪后的 mono 音频
```

节点必须:

- 支持 48 kHz、mono、10 ms(480 samples)的产品主域。
- 不在实时 `process` 路径分配内存。
- 正确实现 `reset`、预热和算法延迟上报。
- 在 stats 中提供节点名、处理耗时和运行错误,不伪造 ERLE。
- 对非有限采样做与现有处理器一致的防御,不得把 NaN/Inf 传给输出设备。

### 5.2 WebRTC NS

推荐用 `aec3-apm` 构造 NS-only APM 节点:

```text
echo_canceller    = None
noise_suppression = Some(level)
gain_controller2  = None
capture format    = 48 kHz mono
```

不直接裸用 `aec3-ns::NoiseSuppressor`,因为裸用需要在 Echoless 重复实现 48 kHz
三频带拆分/合并、上频带增益、延迟补偿、clamp 与帧状态管理。NS-only APM 已经包含
这些边界行为,更适合作为通用节点。

第一版保持现有 WebRTC NS 档位的产品默认值。档位是否继续暴露为高级参数,
由实现阶段另行决定;它不改变主页面的三选一结构。

### 5.3 RNNoise

RNNoise 作为独立节点接收 AEC 输出,保持原生 48 kHz/10 ms 处理域,不增加节点边界
重采样。实现固定到 Xiph 官方 RNNoise 提交 `70f1d256`,并内置该提交对应的官方完整模型:

- 许可证为 BSD-3-Clause,与项目 MIT 许可证兼容;许可全文随发行包分发。
- 固定快照晚于 `v0.2`,使用 32 频带新网络,并包含 `bb18d2f` 的瞬态噪声增益衰减修复。
- 官方 C 源码在构建时静态编译;用户侧不需要 C 工具链、libclang、额外 DLL 或运行时模型下载。
- 完整模型二进制约 14.1 MiB,直接编入产物;源码提交、模型版本和 SHA-256 记录在 vendor 目录。
- `f32` API 使用 16-bit PCM 幅度域,节点负责与 Echoless 的归一化 `[-1, 1]` 域双向转换。
- 算法使用 960-sample 分析窗和 480-sample 步长,节点按 10 ms 上报额定延迟。
- 初始化和模型解析只发生在处理器创建或重置阶段;逐帧处理不得分配堆内存。

没有采用 `nnnoiseless`,因为它仍基于旧版 22 频带网络,没有跟进官方 `v0.2` 的网络
代际和后续瞬态噪声修复。没有采用已停止维护的 `rnnoise-c`/`rnnoise-sys`,以免引入
bindgen、libclang 或额外动态库;Echoless 只维护官方运行时的最小静态构建边界。

## 6. AEC3 行为变化

当前 AEC3 把 WebRTC NS 放在同一个 APM 内:

```text
raw mic -> NS Analyze -> AEC3 -> NS Process -> output
```

其中 `Analyze` 只更新噪声统计;真正修改信号的 `Process` 位于 AEC3 后。拆成通用节点后:

```text
raw mic -> AEC3 -> NS Analyze + NS Process -> output
```

因此两条管线使用相同的 AEC3 与 WebRTC NS 算法,但 NS 的噪声估计输入不同。
这可能影响:

- AEC3 非线性抑制或舒适噪声改变后的噪声估计。
- 双讲期间的人声保护和噪声底稳定性。
- 启动/收敛阶段的瞬态。
- 输出波形、频谱与 AECMOS degradation 分数。

这项变化不能仅凭源码推断为等价,必须通过第 7 节实验确认。实验结论出来之前,
本文不宣称拆分后的 AEC3 与 1.1.0 听感等价。

## 7. 拆分模拟实验

### 7.1 目的

实验只回答以下问题:

1. 集成式 WebRTC NS 与通用后置 WebRTC NS 的输出差异有多大?
2. 差异主要出现在远端单讲、近端单讲还是双讲?
3. 后置分析是否降低回声分数、其他失真分数或人声稳定性?
4. 通用节点是否引入额外帧、启动异常、NaN、clipping 或未上报延迟?

该实验不评价 RNNoise,也不决定 RNNoise 绑定方案。

### 7.2 数据

优先使用 Microsoft AEC Challenge 的真实测试样本,覆盖:

- far-end single talk;
- near-end single talk;
- double talk;
- 条件允许时增加 double talk with echo-path movement。

完整数据集不必进入仓库。若本地 `reference_repos/AEC-Challenge` 没有 LFS 音频,
只取得完成实验所需的最小子集,并在实验报告中记录来源 URL、文件名、SHA-256、
采样率和 talk type。已有的 AEC Challenge 派生示例可以用于预检,但正式报告必须
明确标出来源。

### 7.3 对照管线

对每组 `mic` + `lpb/far` 运行完全相同的 48 kHz、10 ms 帧序列:

| ID | 管线 | 用途 |
|---|---|---|
| A | AEC3(AEC + 内置 NS) | 1.1.0 行为基线 |
| B | AEC3(AEC only) -> WebRTC NS-only APM | 目标架构模拟 |
| C | AEC3(AEC only) | 判断差异来自 NS 位置还是 AEC 本身 |

三条管线使用相同的 AEC3 配置、NS level、初始延迟、参考声道策略和输入帧。
每个音频先完整跑过处理链以保留收敛过程,评分时遵循 AECMOS 对各 talk type 的
有效片段截取规则。

### 7.4 产物与指标

每个样本保存 A/B/C 三份 WAV 和一行结构化结果,至少包含:

- 输出帧数、采样率、峰值、clipped sample count、non-finite sample count;
- AECMOS echo score 与 other-degradation score;
- A/B 的 waveform RMS delta、相关系数和 log-spectral distance;
- 额定与实测延迟差异;
- 平均和 P95 每帧处理耗时;
- talk type、评分片段范围和运行配置。

结果文件:

```text
audit/ns-node-simulation/
  README.md
  experiment.json
  results.csv
  audio/<sample>/{integrated,split,aec_only}.wav
  report.md
```

### 7.5 判读规则

实验报告不使用单一波形差异决定优劣:

- A/B 不 bit-exact 是预期结果,重点看场景化的 AECMOS 与可听差异。
- far-end single talk 重点看 echo score,near-end single talk 重点看 degradation。
- double talk 同时观察 echo 与 degradation,避免通过过度压制近端人声换取更少回声。
- 任何 NaN/Inf、长度变化、持续 clipping、未解释的整帧静音或状态崩溃均为硬失败。
- 若客观指标方向冲突或差异接近模型噪声,保留盲听样本,不强行给出等价结论。

实验结束后再确定以下实现门槛:

- AEC3 是否也迁移到通用 WebRTC NS 节点;
- 是否暂时保留 AEC3 内部 NS,只向 NVAFX 和 LVQE 1.4 提供通用 WebRTC NS;
- 通用 WebRTC NS 的算法延迟应上报多少;
- 是否需要针对 AEC3 保留 pre-AEC analysis tap。

### 7.6 2026-07-13 模拟结果

本地实验使用三个可追溯到 AEC Challenge 条件的便利样本,覆盖 far-end single talk、
near-end single talk 与 double talk。结果只用于识别架构风险,不能替代完整官方测试集。

- A/B/C 输出长度一致,未出现 NaN/Inf、clipping 或拆分新增的整帧静音。
- 两个独立 AEC-only 实例在 f32 输出上 bit-exact,排除了随机舒适噪声混杂。
- 集成 NS 相对 AEC-only 固定增加 6.0 ms;拆分管线相对集成管线又增加约
  0.65–0.92 ms。
- 时移对齐后,integrated 与 split 的相关系数为 `0.893 / 0.574 / 0.547`,证明拆分
  不是波形等价改造,近端单讲和双讲的行为变化更明显。
- AECMOS 没有显示一致性质量回退:far-end echo `-0.0334`,near-end degradation
  `+0.0439`,double-talk degradation `+0.0952`;样本量不足以证明发布级等价。

2026-07-13 完成用户盲听。三个场景均几乎听不出 integrated 与 split 的区别;
double-talk 中用户略偏好 B,盲测答案显示 B 为拆分后的通用节点。结合 AECMOS 没有
一致回退、健壮性检查通过和额外延迟低于1 ms,产品决策如下:

1. WebRTC NS 直接拆成通用 processor node,供 AEC3、NVAFX 与 LocalVQE 1.4 共用。
2. AEC3 的 `WEBRTC` 选项也映射为 `[aec3, webrtc_ns]`,不保留集成 NS 产品路径。
3. AEC3 节点自身以 NS 关闭状态运行,避免重复降噪。
4. 不增加兼容开关、隐藏回退选项或两套并行实现。当前用户规模不需要为旧声音行为
   维持额外复杂度。
5. 通用 WebRTC NS 节点按 `6.5 ms` 上报 nominal latency,并用离线延迟测试锁定。

完整本地报告、可复跑 harness、WAV 与结构化指标位于
`audit/ns-node-simulation/`;该目录按项目约定不进入 Git。

## 8. 实现顺序

1. 引入 `NoiseMode` 与兼容矩阵测试,不接 DSP。
2. 实现 WebRTC NS 节点、配置验证与 processor manifest。
3. 将 AEC3 内置 NS 迁移到通用节点,删除产品层旧映射。
4. 实现三段式 UI、LVQE 标签/悬停说明和原子状态转换。
5. 单独调研并实现 RNNoise 节点。
6. 完成离线质量、实时无分配、延迟、切换和打包验证。

## 9. 验收边界

功能实现完成时必须证明:

- AEC3、NVAFX、LVQE 1.4 均可选择 `WEBRTC / RNNOISE / OFF`。
- LVQE 1.2/1.3 会强制 `OFF`,且配置层无法构造双重 NS。
- 当前选项不可重复点击重载。
- 模型切换与 NS 切换不会遗留旧节点或恢复未显式选择的 NS。
- LVQE 版本标签与悬停说明符合第 3.3 节。
- WebRTC/RNNoise 节点的延迟、耗时和错误进入现有 stats/diagnostics。
- AEC3 迁移后的行为满足实验确定的质量门槛。
