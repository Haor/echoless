import { memo } from "react";
import { useRuntimeLive } from "../runtimeTelemetry";
import { FooterBars, type Telemetry } from "./Scope";

export const RuntimeFooterBars = memo(function RuntimeFooterBars({
  telRef,
  powerOn,
}: {
  telRef: React.MutableRefObject<Telemetry>;
  powerOn: boolean;
}) {
  const live = useRuntimeLive();
  return <FooterBars telRef={telRef} active={powerOn} revision={live.seq} />;
});
