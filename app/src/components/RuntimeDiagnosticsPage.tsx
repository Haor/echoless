import { memo } from "react";
import { useRuntimeHealth } from "../runtimeTelemetry";
import type { DoctorAudio } from "../types";
import { DiagnosticsPage } from "../pages/DiagnosticsPage";

export const RuntimeDiagnosticsPage = memo(function RuntimeDiagnosticsPage({
  rec,
  seconds,
  diagDir,
  running,
  doctor,
  onMicSetup,
  onRec,
  onSeconds,
  onDir,
}: {
  rec: boolean;
  seconds: number | null;
  diagDir: string;
  running: boolean;
  doctor: DoctorAudio | null;
  onMicSetup: () => void;
  onRec: (v: boolean) => void;
  onSeconds: (v: number | null) => void;
  onDir: (v: string) => void;
}) {
  const health = useRuntimeHealth();
  return (
    <DiagnosticsPage
      rec={rec}
      seconds={seconds}
      diagDir={diagDir}
      running={running}
      health={health}
      doctor={doctor}
      onMicSetup={onMicSetup}
      onRec={onRec}
      onSeconds={onSeconds}
      onDir={onDir}
    />
  );
});
