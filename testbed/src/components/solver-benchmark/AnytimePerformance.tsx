import { useMemo, useState } from "react";
import { Button, ButtonGroup, Classes, NonIdealState } from "@blueprintjs/core";
import type { ObjectiveVector, PortfolioTelemetrySummary, SolverTelemetryEvent } from "../../api";

type Metric = keyof Pick<ObjectiveVector, "visited_optional_count" | "visited_optional_priority" | "selected_start" | "finish" | "travel" | "wait">;
const METRICS: { id: Metric; label: string }[] = [
  { id: "visited_optional_count", label: "Visited" }, { id: "visited_optional_priority", label: "Priority" },
  { id: "selected_start", label: "Start" }, { id: "finish", label: "Finish" }, { id: "travel", label: "Travel" }, { id: "wait", label: "Wait" },
];

export function AnytimePerformance({ telemetry, events: liveEvents = [] }: { telemetry?: PortfolioTelemetrySummary; events?: SolverTelemetryEvent[] }) {
  const [metric, setMetric] = useState<Metric>("visited_optional_count");
  const events = liveEvents.length ? liveEvents : telemetry?.events ?? [];
  const points = useMemo(() => events.filter((event) => event.event_type === "GLOBAL_BEST_UPDATED" && event.payload.objective).map((event) => ({ x: event.elapsed_from_job_start_ms, y: event.payload.objective![metric], solver: event.solver_id ?? "unknown", run: event.run_id, objective: event.payload.objective! })), [events, metric]);
  if (!points.length) return <NonIdealState icon="series-search" title={events.length ? "실행 내내 feasible 개선 미발견" : "Anytime telemetry 미수집"} description={events.length ? "최종 결과만 있거나 global best 갱신이 없었습니다." : "이전 Job은 최종 결과만 기록됐을 수 있습니다."} />;
  const width = 760, height = 230, pad = 34;
  const totalBudgetMs = telemetry?.total_budget_ms ?? Math.max(...events.map((event) => event.elapsed_from_job_start_ms), 1);
  const maxX = Math.max(totalBudgetMs, ...points.map((point) => point.x), 1);
  const values = points.map((point) => point.y), minY = Math.min(...values), maxY = Math.max(...values, minY + 1);
  const coordinates = points.map((point) => ({ ...point, sx: pad + point.x / maxX * (width - pad * 2), sy: height - pad - (point.y - minY) / (maxY - minY) * (height - pad * 2) }));
  const path = coordinates.map((point, index) => index ? `H${point.sx} V${point.sy}` : `M${point.sx} ${point.sy}`).join(" ");
  return <section className="portfolio-panel" aria-labelledby="anytime-title"><header className="portfolio-panel-heading"><div><h2 id="anytime-title" className={Classes.HEADING}>Anytime Performance</h2><p className={Classes.TEXT_MUTED}>사전식 목적함수를 임의 가중합하지 않고 구성 요소별 step trace로 표시합니다.</p></div></header>
    <ButtonGroup className="anytime-tabs">{METRICS.map((item) => <Button key={item.id} active={metric === item.id} onClick={() => setMetric(item.id)}>{item.label}</Button>)}</ButtonGroup>
    <div className="anytime-chart-scroll"><svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={`${metric} best-so-far step chart`}><line x1={pad} y1={height-pad} x2={width-pad} y2={height-pad} className="chart-axis"/><line x1={pad} y1={pad} x2={pad} y2={height-pad} className="chart-axis"/><path d={path} className="anytime-line" fill="none"/>{coordinates.map((point, index) => <circle key={`${point.x}-${index}`} cx={point.sx} cy={point.sy} r="4" className="anytime-point"><title>{`${point.x} ms · ${point.solver} run ${point.run ?? "?"} · ${metric}=${point.y}`}</title></circle>)}<text x={pad} y={height-8}>0 ms</text><text x={width-pad-60} y={height-8}>{maxX} ms</text><text x={4} y={pad+4}>{maxY}</text><text x={4} y={height-pad}>{minY}</text></svg></div>
    <div className="anytime-markers">{[1_000, 5_000, 10_000, 30_000].filter((time) => time <= totalBudgetMs).map((time) => { const prior = [...points].reverse().find((point) => point.x <= time); return <span key={time}><strong>{time/1000}s</strong>{prior ? prior.y : "feasible 전"}</span>; })}</div>
  </section>;
}
