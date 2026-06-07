import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  buildConfigToml,
  doctorAudio,
  getPlatform,
  listDevices,
  listProcessors,
  onRunEvent,
  onRunExit,
  startRun,
  stopRun,
  validateConfig,
} from "./api";
import type {
  AudioDevice,
  DeviceList,
  DoctorAudio,
  Platform,
  Processor,
} from "./types";
import {
  AppIcon,
  CapClose,
  CapMax,
  CapMin,
  IcoInput,
  IcoModel,
  IcoNoise,
  IcoOutput,
} from "./components/icons";
import { FooterBars, Scope, type Telemetry } from "./components/Scope";
import { Dropdown } from "./components/Dropdown";
import { ScrambleText } from "./components/ScrambleText";

const appWindow = getCurrentWindow();

// 设备选择值统一用 stable_id(跨重启稳定;mic/output 配置直接吃它)。
// 选默认输出:优先虚拟声卡(VB-CABLE / BlackHole),否则系统默认。
function pickDefaultOutput(outs: AudioDevice[]): string {
  const virt = outs.find((d) => /cable|blackhole|vb-?audio/i.test(d.name));
  if (virt) return virt.stable_id;
  return (
    outs.find((d) => d.is_default)?.stable_id ?? outs[0]?.stable_id ?? "default"
  );
}
function pickDefaultInput(ins: AudioDevice[]): string {
  return ins.find((d) => d.is_default)?.stable_id ?? ins[0]?.stable_id ?? "default";
}

const MODELS: { kind: string; label: string }[] = [
  { kind: "sonora_aec3", label: "AEC3" },
  { kind: "localvqe", label: "LOCALVQE" },
  { kind: "nvidia_afx_aec", label: "NVAFX" },
];

function modelName(kind: string): string {
  return MODELS.find((m) => m.kind === kind)?.label ?? kind.toUpperCase();
}

interface Live {
  mic: number | null;
  ref: number | null;
  out: number | null;
  lat: number | null;
  healthy: boolean;
}

export default function App() {
  const [platform, setPlatform] = useState<Platform>("macos");
  const [devices, setDevices] = useState<DeviceList | null>(null);
  const [processors, setProcessors] = useState<Processor[]>([]);
  const [selInput, setSelInput] = useState("default");
  const [selOutput, setSelOutput] = useState("default");
  const [kind, setKind] = useState("sonora_aec3");
  const [ns, setNs] = useState(false);
  const [running, setRunning] = useState(false); // 进程是否存活(含 restart 抖动)
  const [powerOn, setPowerOn] = useState(false); // 用户开关意图(UI 显示/动画只看这个)
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [doctor, setDoctor] = useState<DoctorAudio | null>(null);
  // reference:可用源由 devices.reference_sources 提供;mac system 无 loopback → 默认退 none。
  const [reference, setReference] = useState("system");
  const [live, setLive] = useState<Live>({
    mic: null,
    ref: null,
    out: null,
    lat: null,
    healthy: true,
  });

  const telRef = useRef<Telemetry>({ mic: -120, ref: -120, out: -120, on: false });
  const runningRef = useRef(running);
  runningRef.current = running;
  const powerOnRef = useRef(powerOn);
  powerOnRef.current = powerOn;

  // 平台 + 设备/处理器枚举 + 事件订阅
  useEffect(() => {
    // 清理可能残留的 sidecar(前端 reload 后 Rust 子进程可能还活着 → 状态脱同步)。
    stopRun().catch(() => {});
    getPlatform().then(setPlatform).catch(() => {});
    refreshDevices();
    listProcessors()
      .then((m) => setProcessors(m.processors))
      .catch((e) => setErr(String(e)));
    doctorAudio().then(setDoctor).catch(() => {});

    const uns: UnlistenFn[] = [];
    (async () => {
      uns.push(
        await onRunEvent((ev) => {
          if (ev.type === "started") {
            telRef.current.on = true;
            setRunning(true);
            return;
          }
          // status
          const s = ev;
          const tel = telRef.current;
          tel.mic = s.mic_dbfs;
          tel.ref = s.ref_dbfs;
          tel.out = s.out_dbfs;
          tel.on = true;
          tel.micWave = s.mic_wave;
          tel.refWave = s.ref_wave;
          tel.outWave = s.out_wave;
          setLive({
            mic: s.mic_dbfs,
            ref: s.ref_dbfs,
            out: s.out_dbfs,
            lat: s.estimated_user_latency_ms,
            healthy:
              !s.diverged && s.runtime_errors === 0 && !s.last_backend_error,
          });
        }),
      );
      uns.push(
        await onRunExit(() => {
          telRef.current.on = false;
          setRunning(false);
        }),
      );
    })();
    return () => uns.forEach((u) => u());
  }, []);

  function refreshDevices() {
    listDevices()
      .then((d) => {
        setDevices(d);
        setSelInput((cur) => (cur === "default" ? pickDefaultInput(d.inputs) : cur));
        setSelOutput((cur) =>
          cur === "default" ? pickDefaultOutput(d.outputs) : cur,
        );
        // 默认 reference:system 可用就用 system,否则退到 none;用户改过则保留。
        const sys = d.reference_sources.find((r) => r.id === "system");
        setReference((cur) =>
          cur !== "system" ? cur : sys && !sys.available ? "none" : "system",
        );
      })
      .catch((e) => setErr(String(e)));
  }

  type Override = Partial<{
    mic: string;
    output: string;
    reference: string;
    kind: string;
    ns: boolean;
  }>;

  function currentToml(over?: Override) {
    return buildConfigToml({
      mic: over?.mic ?? selInput,
      output: over?.output ?? selOutput,
      reference: over?.reference ?? reference,
      kind: over?.kind ?? kind,
      ns: over?.ns ?? ns,
    });
  }

  async function start() {
    setBusy(true);
    setErr(null);
    try {
      const toml = currentToml();
      const v = await validateConfig(toml);
      if (!v.ok) {
        setErr(v.errors.map((e) => `${e.path}: ${e.message}`).join("; "));
        setBusy(false);
        return;
      }
      telRef.current.on = true;
      await startRun(toml, 80);
      setRunning(true);
      setPowerOn(true);
    } catch (e) {
      setErr(String(e));
      telRef.current.on = false;
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    setBusy(true);
    try {
      await stopRun();
    } catch (e) {
      setErr(String(e));
    } finally {
      telRef.current.on = false;
      setRunning(false);
      setPowerOn(false);
      setBusy(false);
    }
  }

  async function togglePower() {
    if (busy) return;
    if (powerOn) await stop();
    else await start();
  }

  // 运行中改配置 → 重启 runtime 应用新值(后端契约要求)。
  // 成功路径不动 powerOn,避免状态框/开关跟着 scramble(各管各的)。
  async function applyChange(next: Override) {
    if (!powerOnRef.current) return;
    setBusy(true);
    try {
      await stopRun();
      const toml = currentToml(next);
      const v = await validateConfig(toml);
      if (!v.ok) {
        setErr(v.errors.map((e) => `${e.path}: ${e.message}`).join("; "));
        telRef.current.on = false;
        setRunning(false);
        setPowerOn(false);
        return;
      }
      telRef.current.on = true;
      await startRun(toml, 80);
      setRunning(true);
      setErr(null);
    } catch (e) {
      setErr(String(e));
      telRef.current.on = false;
      setRunning(false);
      setPowerOn(false);
    } finally {
      setBusy(false);
    }
  }

  const refOptions = (devices?.reference_sources ?? [])
    .filter((r) => r.available)
    .map((r) => ({
      value: r.selector ?? r.id,
      // input/output 同名设备(如 BlackHole 2ch)加方向标注以区分
      label:
        r.kind === "input"
          ? `${r.label} · in`
          : r.kind === "output"
            ? `${r.label} · out`
            : r.label,
    }));

  const isMac = platform === "macos";
  const off = !powerOn;
  const stopped = off;
  const unstable = powerOn && !live.healthy;

  const statusText = stopped
    ? "ECHO STOPPED"
    : unstable
      ? "UNSTABLE"
      : "REMOVING ECHO";
  const boxClass = stopped ? "box stopped" : unstable ? "box warn" : "box";

  const dash = (v: number | null, d = 1) =>
    v === null ? "—" : v.toFixed(d);

  return (
    <div className={`window ${isMac ? "mac" : "win"}`}>
      {/* ---- titlebar ---- */}
      <header className="tbar" data-tauri-drag-region>
        <AppIcon />
        <span className="screen">Overview</span>
        <span className="hatch" />
        <span className="uid">{modelName(kind)}</span>
        {!isMac && (
          <span className="caption">
            <button className="cbtn" onClick={() => appWindow.minimize()}>
              <CapMin />
            </button>
            <button className="cbtn" onClick={() => appWindow.toggleMaximize()}>
              <CapMax />
            </button>
            <button className="cbtn close" onClick={() => appWindow.close()}>
              <CapClose />
            </button>
          </span>
        )}
      </header>

      {/* ---- content ---- */}
      <main className="content">
        <div className="kick">
          <span className="d">
            <i />
            <i />
            <i />
          </span>{" "}
          Acoustic Echo Cancellation · Local
        </div>
        <div className="hero">
          <div className="word">ECHOLESS</div>
          {/* 物理滑动开关:主体方块在条纹轨道里左右滑动 + 标签 scramble */}
          <button
            className={`power ${off ? "off" : "on"}`}
            disabled={busy}
            onClick={togglePower}
          >
            <span className="slider">
              <ScrambleText text={off ? "OFF" : "ON"} trigger={powerOn} />
            </span>
          </button>
        </div>
        <div className="status">
          <span className={boxClass}>
            {/* 运行=圆点 ●,停止=方块 ■ */}
            <span className={`sq ${powerOn ? "dot" : ""}`} />{" "}
            <ScrambleText text={statusText} />
          </span>
          <span className="m">
            LATENCY <b>{dash(live.lat, 0)}</b> MS
          </span>
          <span className="m">{unstable ? "CHECK SETUP" : "STABLE"}</span>
        </div>

        <hr className="hair" />

        {/* ---- controls ---- */}
        <div className="rows">
          <div className="row">
            <span className="bul">•</span>
            <span className="k">Input</span>
            <span className="co">:</span>
            <span className="v">
              <Dropdown
                value={selInput}
                options={(devices?.inputs ?? []).map((d) => ({
                  value: d.stable_id,
                  label: d.name,
                }))}
                onChange={(v) => {
                  setSelInput(v);
                  applyChange({ mic: v });
                }}
              />
            </span>
            <span className="sp" />
            <span className="meta">Microphone · Near-end</span>
            <span className="ico">
              <IcoInput />
            </span>
          </div>

          <div className="row">
            <span className="bul">•</span>
            <span className="k">Model</span>
            <span className="co">:</span>
            <div className="segg" id="models">
              {MODELS.map((m) => {
                const proc = processors.find((p) => p.kind === m.kind);
                const supported =
                  !proc || proc.platforms.includes(platform);
                const exp = proc?.experimental;
                return (
                  <button
                    key={m.kind}
                    className={`b ${kind === m.kind ? "active" : ""} ${
                      exp ? "exp" : ""
                    }`}
                    disabled={!supported}
                    onClick={() => {
                      setKind(m.kind);
                      applyChange({ kind: m.kind });
                    }}
                  >
                    {m.label}
                  </button>
                );
              })}
            </div>
            <span className="sp" />
            <span className="meta">
              Reference{" "}
              <Dropdown
                compact
                align="right"
                warn={reference === "none"}
                value={reference}
                options={refOptions}
                onChange={(v) => {
                  setReference(v);
                  applyChange({ reference: v });
                }}
              />
            </span>
            <span className="ico">
              <IcoModel />
            </span>
          </div>

          <div className="row">
            <span className="bul">•</span>
            <span className="k">Output</span>
            <span className="co">:</span>
            <span className="v">
              <Dropdown
                value={selOutput}
                options={(devices?.outputs ?? []).map((d) => ({
                  value: d.stable_id,
                  label: d.name,
                }))}
                onChange={(v) => {
                  setSelOutput(v);
                  applyChange({ output: v });
                }}
              />
            </span>
            <span className="sp" />
            {doctor && !doctor.virtual_output_detected ? (
              <span className="meta" style={{ color: "var(--warn)" }}>
                <span className="mk">!!!</span> install{" "}
                <b>{doctor.recommended_driver}</b> virtual cable
              </span>
            ) : (
              <span className="meta">
                <span className="mk">&gt;&gt;&gt;</span> in app pick{" "}
                <b>CABLE Output</b> as mic
              </span>
            )}
            <span className="ico">
              <IcoOutput />
            </span>
          </div>

          <div className="row">
            <span className="bul">•</span>
            <span className="k">Noise</span>
            <span className="co">:</span>
            <div className="segg" id="ns">
              <button
                className={`b ${ns ? "active" : ""}`}
                onClick={() => {
                  setNs(true);
                  applyChange({ ns: true });
                }}
              >
                ON
              </button>
              <button
                className={`b ${!ns ? "active" : ""}`}
                onClick={() => {
                  setNs(false);
                  applyChange({ ns: false });
                }}
              >
                OFF
              </button>
            </div>
            <span className="sp" />
            <span className="meta">Reduce background noise</span>
            <span className="ico">
              <IcoNoise />
            </span>
          </div>
        </div>

        <hr className="hair" />

        {/* ---- signal:三路示波 ---- */}
        <div className="sig">
          <div className="h">
            <span className="t">// Signal</span>
            <span className="v">
              Near-end <b>Mic + Ref</b> &raquo; Clean <b>Output</b>
            </span>
          </div>
          <div className="scope">
            <div className="near">
              <div className="trace">
                <span className="lb">MIC</span>
                <Scope traceKey="mic" telRef={telRef} phase={0} />
                <span className="db">
                  {dash(live.mic)} <i>dBFS</i>
                </span>
              </div>
              <div className="trace">
                <span className="lb">REF</span>
                <Scope traceKey="ref" telRef={telRef} phase={2.1} />
                <span className="db">
                  {dash(live.ref)} <i>dBFS</i>
                </span>
              </div>
            </div>
            <div className="gap">&raquo;</div>
            <div className="far">
              <div className="trace">
                <span className="lb">OUT</span>
                <Scope traceKey="out" telRef={telRef} phase={4.2} />
                <span className="db">
                  {dash(live.out)} <i>dBFS</i>
                </span>
              </div>
            </div>
          </div>
        </div>
      </main>

      {/* ---- footer ---- */}
      <footer className="fbar">
        <button className="link" style={linkStyle}>
          Advanced <span className="mk">&gt;&gt;&gt;</span>
        </button>
        <button className="link" style={linkStyle}>
          Diagnostics <span className="mk">&gt;&gt;&gt;</span>
        </button>
        <span className="sp" />
        {err ? (
          <span className="stamp" style={{ color: "var(--warn)" }} title={err}>
            {err.length > 48 ? err.slice(0, 48) + "…" : err}
          </span>
        ) : (
          <span className="stamp">MONO · 48K · 10MS</span>
        )}
        <FooterBars telRef={telRef} />
      </footer>
    </div>
  );
}

const linkStyle: React.CSSProperties = {
  color: "var(--t-soft)",
  textDecoration: "none",
  display: "flex",
  alignItems: "center",
  gap: 7,
  background: "transparent",
  border: "none",
  font: "inherit",
  letterSpacing: "inherit",
  textTransform: "inherit",
};
