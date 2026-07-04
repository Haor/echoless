# Handoff — 采集/输出设备 I/O 重采样(支持非 16k/48k 原生率的真实设备)

**From:** 前端(GUI / 实测)
**To:** Codex(后端 / realtime 引擎)
**状态:** 后端第一版已实现(2026-06-08):固定比率线性 SRC,未做 drift 自适应
**优先级:** 高 —— 直接导致真实 USB 麦无法使用(用户外放 + USB 麦是核心场景)。

---

## 1. 现象(实机复现)

用户在 GUI 选 USB 麦 **WhiteCatBox** 作输入,开 ON 后:UI 显示「正在消除回声」,
但波形不亮、延迟「—」。排查发现 `echoless run` 子进程**一启动就报错退出**:

```
实时运行配置: mic=WhiteCatBox ref=system out=BlackHole
Error: 麦克风不支持该采样率
Caused by:
    WhiteCatBox 在 48000 Hz 无可用 input 配置
```

16k 同样失败(`WhiteCatBox 在 16000 Hz 无可用 input 配置`)。

## 2. 根因(代码核实)

- `WhiteCatBox` 原生采样率 = **24000 Hz**(`devices --json`:`default_sample_rate: 24000`,1ch)。
  既不支持 48k 也不支持 16k。
- `pick_config`(`crates/echoless-cli/src/realtime.rs:1842`)按设备**原生支持的采样率范围**
  筛选:
  ```rust
  .filter(|r| r.min_sample_rate() <= sample_rate && sample_rate <= r.max_sample_rate())
  .map(|r| r.with_sample_rate(sample_rate))
  ```
  设备范围里没有 16k/48k → `无可用 input 配置` → bail → 子进程退出。
- 旧实现即:**采集/输出设备必须原生支持管线采样率;后端不对设备 I/O 做重采样。**
  (注:`ProcessorChain` 只在节点边界适配,如 LocalVQE 16k;旧设备 cpal 流不resample。)
- 对照:**MacBook Pro麦克风(原生 48k)+ Process Tap ref(48k)实测完全跑通**
  —— 所以问题纯粹是设备原生率 ≠ 管线率时缺重采样。

## 3. 已落地方案:设备 I/O 重采样

打开 cpal 采集/输出流时优先用**管线采样率**,设备不支持时回退到默认/最近可用原生率,
在设备 ↔ 管线之间做固定比率线性重采样,使常见非 16k/48k 设备(24k / 44.1k / 32k …)
不再因为 `无可用 input/output 配置` 直接退出:

- **输入(mic / 设备型 reference)**:设备流采样率 → 重采样到 `sample_rate` → 喂 ring。
- **输出**:管线 `sample_rate` → 重采样到输出设备流采样率 → 写设备。
- `pick_config` 改为:优先精确匹配;不行则选默认 config;再不行选与管线采样率最近的支持范围边界。
- Process Tap reference 仍由 macOS helper 提供 48k;全局 pipeline 使用 Process Tap 时仍要求
  `sample_rate = 48000`。

### 当前边界

1. 第一版是固定比率线性 SRC,不做设备时钟漂移自适应。
2. 质量优先级高的后续版本可把边界 SRC 换成有状态高质量 resampler,并把 drift 估计接入同一层。
3. 多设备同时重采样已支持(mic + 设备型 ref + output),但暂未在 WhiteCatBox / 44.1k / 长时间运行场景真机压测。
4. 这解决的是设备原生采样率不匹配导致的启动失败;若后续出现长时间 buffer 漂移/断续,仍需要异步 SRC/drift 控制。

## 4. 前端现状(已配合)

- 已修「子进程崩了却显示 ON」:run 非预期退出 → 自动关 ON,并把子进程 stderr 末行
  (真正错误原因)显示在底栏。订阅 `echoless://log` 取 stderr;主动停/重启用
  `suppressExitRef` 区分,不误报。
- 现在用户选 24k 麦开 ON 会**立刻看到错误原因**,而不是静默假 ON。
- 后端已在 `devices --json` 的每个 device 上新增 `supported_sample_rates`。
  - 正常返回数组,每项为 `{ "min": 24000, "max": 24000, "channels": 1, "sample_format": "f32" }`。
  - 读取失败时返回 `{ "error": "..." }`。
- `run --status-json` 的 `started` 事件新增:
  - `mic_device_sample_rate`
  - `output_device_sample_rate`
  - `reference_device_sample_rate`
  - `io_resampling: { mic, reference, output }`
- 前端无需因为设备不是 48k/16k 而阻止运行;可把这些字段用于 diagnostics/状态说明。

## 5. 验收

- 选 24k / 44.1k 等非 16k/48k 的真实麦 + Process Tap(48k)ref → 能正常 run、出波形、
  AEC 收敛,无「无可用 input 配置」报错。
- 输出设备非 48k 时同样可用。
- 重采样不引入持续 underrun / 明显延迟回归。

## 6. 复现命令

```bash
# 旧实现中 24k 麦必失败;当前后端应能启动并在 started 事件里标记 mic io_resampling=true
echoless run --config - <<'EOF'
mic = "WhiteCatBox"
reference = "system"
output = "BlackHole"
sample_rate = 48000
frame_ms = 10
reference_channels = "mono"
[[chain]]
kind = "aec3"
EOF
# 旧错误: 麦克风不支持该采样率 · WhiteCatBox 在 48000 Hz 无可用 input 配置

# 48k 麦,正常(对照)
#   mic = "MacBook Pro麦克风"  → started ok, reference_source: macos_process_tap
```

## 7. 后端验证

已完成:

```bash
cargo test -p echoless-cli
cargo test -p echoless-cli --no-default-features
cargo run -q -p echoless-cli -- devices --json
```

当前 sandbox 下 `devices --json` 无法枚举宿主音频设备,但命令本身和 JSON 输出路径已通过。
仍需 GUI/真机验证 WhiteCatBox 24k 麦、44.1k 设备和长时间运行稳定性。
