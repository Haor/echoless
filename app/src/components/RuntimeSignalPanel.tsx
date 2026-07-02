import { memo } from "react";
import { useI18n } from "../i18n";
import { useRuntimeLive } from "../runtimeTelemetry";
import { Scope, type Telemetry } from "./Scope";

const dash = (v: number | null, d = 1) => (v === null ? "—" : v.toFixed(d));

export const RuntimeSignalPanel = memo(function RuntimeSignalPanel({
  telRef,
  powerOn,
}: {
  telRef: React.MutableRefObject<Telemetry>;
  powerOn: boolean;
}) {
  const live = useRuntimeLive();
  const { t } = useI18n();
  return (
    <div className="sig">
      <div className="h">
        <span className="t slashText">{t("signal")}</span>
        <span className="v">{t("sigFlow")}</span>
      </div>
      <div className="scope">
        <div className="near">
          <div className="trace">
            <span className="lb">MIC</span>
            <Scope
              traceKey="mic"
              telRef={telRef}
              active={powerOn}
              revision={live.seq}
              phase={0}
            />
            <span className="db">
              {dash(live.mic)} <i>dBFS</i>
            </span>
          </div>
          <div className="trace">
            <span className="lb">REF</span>
            <Scope
              traceKey="ref"
              telRef={telRef}
              active={powerOn}
              revision={live.seq}
              phase={2.1}
            />
            <span className="db">
              {dash(live.ref)} <i>dBFS</i>
            </span>
          </div>
        </div>
        <div className="gap">&raquo;</div>
        <div className="far">
          <div className="trace">
            <span className="lb">OUT</span>
            <Scope
              traceKey="out"
              telRef={telRef}
              active={powerOn}
              revision={live.seq}
              phase={4.2}
            />
            <span className="db">
              {dash(live.out)} <i>dBFS</i>
            </span>
          </div>
        </div>
      </div>
    </div>
  );
});
