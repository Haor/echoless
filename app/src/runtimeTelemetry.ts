import { useSyncExternalStore } from "react";
import type { RuntimeStatus } from "./types";

export interface Live {
  mic: number | null;
  ref: number | null;
  out: number | null;
  lat: number | null;
  healthy: boolean;
  seq: number;
}

export interface Health {
  input_drops: number;
  ref_underruns: number;
  output_underruns: number;
  mic_stale_drops: number;
  ref_stale_drops: number;
  stale_drops: number;
  runtime_errors: number;
  diverged: boolean;
  session_dir: string | null;
  backend_error: string | null;
  recording: boolean;
  rec_elapsed_s: number;
  rec_drops: number;
}

const ZERO_LIVE: Live = {
  mic: null,
  ref: null,
  out: null,
  lat: null,
  healthy: true,
  seq: 0,
};

const ZERO_HEALTH: Health = {
  input_drops: 0,
  ref_underruns: 0,
  output_underruns: 0,
  mic_stale_drops: 0,
  ref_stale_drops: 0,
  stale_drops: 0,
  runtime_errors: 0,
  diverged: false,
  session_dir: null,
  backend_error: null,
  recording: false,
  rec_elapsed_s: 0,
  rec_drops: 0,
};

let liveSnapshot = ZERO_LIVE;
let healthSnapshot = ZERO_HEALTH;
const listeners = new Set<() => void>();

function emit() {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function resetRuntimeHealth() {
  healthSnapshot = ZERO_HEALTH;
  emit();
}

// 停机/重启时清空实时读数(dBFS / 延迟),否则最后一帧数值残留在界面上。
export function resetRuntimeLive() {
  liveSnapshot = ZERO_LIVE;
  emit();
}

export function setDiagnosticsSessionDir(sessionDir: string | null) {
  healthSnapshot = { ...healthSnapshot, session_dir: sessionDir };
  emit();
}

export function publishRuntimeStatus(s: RuntimeStatus) {
  liveSnapshot = {
    mic: s.mic_dbfs,
    ref: s.ref_dbfs,
    out: s.out_dbfs,
    lat: s.estimated_user_latency_ms,
    healthy: !s.diverged && s.runtime_errors === 0 && !s.last_backend_error,
    seq: s.frames,
  };
  healthSnapshot = {
    input_drops: s.input_drops ?? 0,
    ref_underruns: s.ref_underruns ?? 0,
    output_underruns: s.output_underruns ?? 0,
    mic_stale_drops: s.mic_stale_drops ?? 0,
    ref_stale_drops: s.ref_stale_drops ?? 0,
    stale_drops: s.stale_drops ?? 0,
    runtime_errors: s.runtime_errors ?? 0,
    diverged: Boolean(s.diverged),
    session_dir: s.diagnostics_session_dir ?? healthSnapshot.session_dir,
    backend_error: s.last_backend_error ?? null,
    recording: Boolean(s.recording),
    rec_elapsed_s: s.diagnostics_elapsed_s ?? 0,
    rec_drops: s.diagnostics_drops ?? 0,
  };
  emit();
}

export function useRuntimeLive(): Live {
  return useSyncExternalStore(subscribe, () => liveSnapshot, () => ZERO_LIVE);
}

export function useRuntimeHealth(): Health {
  return useSyncExternalStore(subscribe, () => healthSnapshot, () => ZERO_HEALTH);
}
