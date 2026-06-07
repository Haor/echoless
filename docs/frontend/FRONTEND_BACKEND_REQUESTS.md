# Frontend → Backend 需求清单

GUI 已落地(`echoless/app/`,Tauri v2 + React,只消费 JSON/JSONL 契约)。本文件列出**前端希望后端补齐/稳定的能力**,按优先级排。字段名为建议,可改,但请保持 `*--json` 输出稳定可解析。

契约现状以 `FRONTEND_AGENT_HANDOFF.md` / `FRONTEND_ADAPTATION_PLAN.md` 为准;本文件只列**增量**。

> 实测基线(2026-06,本机 macOS):`devices/processors/config validate/run --status-json` 均可用且形状与文档一致(差异:`processors` 为顶层数组、JSON 键按字母序、无 reference 时 `ref_dbfs = -120.0`)。

## 后端处理记录(2026-06-07)

- P0 已实现:`run --status-json` 的 `status` event 增加 `mic_wave` / `ref_wave` / `out_wave`。每路固定 64 个 float,语义为当前 stats interval 内的 peak 包络,范围 `[0,1]`。
- P1 已实现:`echoless doctor audio --json`。返回虚拟音频候选、推荐驱动、安装状态、macOS 权限状态估计、reference source 可用性。
- P1 已实现:`devices --json` 每个设备增加 `stable_id`;`mic` / `output` / `input:<stable_id>` / `output:<stable_id>` 可被选择器匹配。当前 stable id 优先使用 CPAL `DeviceId`,其次设备地址,最后名称派生;不是最终 WASAPI endpoint id / CoreAudio UID 原生实现。
- P2 已实现:`reference_sources` 中的 `system` 增加 `available` 和 `hint`;macOS 默认标记为不可用并提示使用 BlackHole/VB-CABLE MAC 路由。
- P2 已实现:`config validate --json` 增加轻量结构校验,常见顶层字段类型和 `chain[].kind` 错误会落到具体 path;`mic` / `reference` / `output` 缺省时走默认值。
- P2 已实现:`run --status-json` 在音频流启动后先输出 `{"type":"started", ...}`。
- P2 已部分实现:`nvidia_afx_aec` 配置校验会运行 `doctor_report`;doctor 不通过时返回 `chain[i].doctor` 错误。

---

## P0 — 真实波形(最高杠杆)

**现状**:`run --status-json` 只给 dBFS 标量,三路示波只能用 dBFS 包络合成曲线(风格化,非真实波形)。

**请求**:在 status event 里**增加三路降采样波形数组**:

```jsonc
{
  "type": "status",
  // ...现有字段...
  "mic_wave": [/* N 个 float */],
  "ref_wave": [/* N 个 float */],
  "out_wave": [/* N 个 float */]
}
```

语义建议:

- **N ≈ 48~64**(每路);三路合计 ≤ 192 floats,体积可控。
- 每条 = **本次 stats 窗口内的降采样包络**:把该 `--stats-interval-ms` 间隔内的样本分成 N 桶,每桶取 **peak**(`max(abs(x))`)或 RMS;peak 更适合示波视觉。
- **归一化**:`[0,1]`(幅度)或 `[-1,1]`(带符号波形)均可,二选一并固定。前端 `Scope.tsx` 已前向兼容:**一旦检测到 `*_wave` 数组就自动改画真实波形,无需前端再改**。
- 节流后端可控:无人看时前端不订阅;给了数组也只在间隔输出一次,不需要额外高频流。

**收益**:示波器从「风格化」升级为真示波器,这是 demo 可信度的关键一环。

---

## P1 — 虚拟声卡检测 `doctor audio --json`

**现状**:前端只能靠设备名正则(`CABLE` / `BlackHole` / `VB-Audio`)猜虚拟声卡是否安装,无法做可靠的安装引导。

**请求**:新增命令(`FRONTEND_ADAPTATION_PLAN.md` 已规划,完成度 20%):

```bash
echoless doctor audio --json
```

```jsonc
{
  "ok": true,
  "virtual_output_detected": true,
  "candidate_outputs": [ { "name": "CABLE Input (VB-Audio Virtual Cable)", "selector": "1" } ],
  "candidate_inputs":  [ { "name": "CABLE Output (VB-Audio Virtual Cable)", "selector": "5" } ],
  "recommended_driver": "vb-cable",          // win: "vb-cable" / mac: "blackhole-2ch" | "vb-cable-mac"
  "install_status": "installed",             // installed | missing | unknown
  "needs_reboot": false,
  "permission_state": "granted"              // mac 麦克风权限: granted | denied | undetermined
}
```

**收益**:支撑首屏「未检测到虚拟声卡 → 引导安装 → 重新枚举验证」的 onboarding。

---

## P1 — 稳定设备标识

**现状**:`devices --json` 的 `selector` = 设备索引,**跨重启/插拔不稳**;前端只能额外存 `name` 做模糊匹配。

**请求**:每个设备多给一个**跨重启稳定的 id**(macOS CoreAudio UID / Windows WASAPI endpoint id):

```jsonc
{ "index": 1, "selector": "1", "stable_id": "AppleHDAEngineInput:1B,0,1,0:1", "name": "MacBook Pro麦克风", ... }
```

并允许 `mic` / `output` / `reference` 配置接受 `stable_id`(与现有 name/index 并存)。

**收益**:用户选的设备能可靠记住,不会因为枚举顺序变化选错设备。

---

## P2 — 锦上添花

1. **macOS system reference 可用性标记**:`reference="system"` 在 macOS 无原生 loopback,现在静默跑出 `ref_dbfs = -120`。建议在 `devices --json` 的 `reference_sources` 里给 `system` 源加 `available: bool` + `hint`,前端可在选 system 时直接提示「macOS 需装 BlackHole/VB-CABLE 路由」,而不是让用户看着静音参考困惑。
2. **config validate 语义校验**:目前缺 `mic` 等必填字段时报的是「解析 TOML 失败」(`path: "config"`)。希望必填/范围错误走**结构化 path**(如 `chain[0].model: required`),前端好定位到具体控件;并希望 validate 能预检 backend 可用性(如 `nvidia_afx_aec` 需 doctor ok)。
3. **run 启动确认**:目前前端靠「收到第一条 status」判定真正跑起来了。若能在 stdout 先发一条 `{"type":"started", backend, sample_rate, frame_ms}`(或 error 时 `{"type":"error", message}`),前端可更快、更准地切换 UI 状态与报错。

---

## 前端自行承担(无需后端)

- 配置持久化(设备/模型/NS 选择)— Tauri 侧存,不要后端配置文件概念。
- 进程生命周期 — Tauri sidecar `spawn/kill`(已实现);关窗自动 kill。
- CPU/MEM — UI 已砍,不需要字段。
- 打包 sidecar — `tauri build` 用 `externalBin` + `ECHOLESS_BIN` 注入,前端处理。
