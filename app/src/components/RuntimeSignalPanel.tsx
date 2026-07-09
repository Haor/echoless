import { memo } from "react";
import { useI18n } from "../i18n";
import { useRuntimeLive } from "../runtimeTelemetry";
import { Scope, type Telemetry } from "./Scope";
import { ScrambleText } from "./ScrambleText";
import { RAIL_TEXT, type RunStatusKind } from "./RuntimeStatusStrip";
import { dash } from "../numeric";

export const RuntimeSignalPanel = memo(function RuntimeSignalPanel({
  telRef,
  powerOn,
  statusKind,
}: {
  telRef: React.MutableRefObject<Telemetry>;
  powerOn: boolean;
  statusKind: RunStatusKind;
}) {
  const live = useRuntimeLive();
  const { t } = useI18n();
  // OFF(穿透/停机):波形改灰,信号仍在流动(mic 活着)但退出视觉主角。
  const dimmed = statusKind === "bypass" || statusKind === "stopped";
  return (
    <div className="sig">
      <div className="h">
        <span className="t slashText">{t("signal")}</span>
        <span className="v">{t("sigFlow")}</span>
      </div>
      <div className="scope">
        {/* v14:srail = 监视状态字 + 量程(v17 两列,状态字随四态 scramble) */}
        <span className="srail">
          <span>
            <ScrambleText text={RAIL_TEXT[statusKind]} />
          </span>
          <span>0 / −120 DBFS</span>
        </span>
        <div className="near">
          <div className="trace">
            <span className="lb">MIC</span>
            <Scope
              traceKey="mic"
              telRef={telRef}
              active={powerOn}
              revision={live.seq}
              phase={0}
              dimmed={dimmed}
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
              dimmed={dimmed}
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
              dimmed={dimmed}
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
