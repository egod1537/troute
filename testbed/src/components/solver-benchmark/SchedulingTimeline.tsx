import { useState } from "react";
import { Button, ButtonGroup, Classes, NonIdealState, Tag } from "@blueprintjs/core";
import type { PortfolioTelemetrySummary, SolverTelemetryEvent } from "../../api";

function terminal(event: SolverTelemetryEvent) {
  return ["RUN_FINISHED", "RUN_TIMED_OUT", "RUN_CANCELLED", "RUN_FAILED"].includes(event.event_type);
}

export function SchedulingTimeline({ telemetry, events: liveEvents = [] }: { telemetry?: PortfolioTelemetrySummary; events?: SolverTelemetryEvent[] }) {
  const [page, setPage] = useState(0);
  const events = liveEvents.length ? liveEvents : telemetry?.events ?? [];
  if (!events.length) return <NonIdealState icon="timeline-events" title="Scheduling timeline 미수집" description="Detailed telemetry를 활성화하면 queue, run, decision, best-update 시점을 볼 수 있습니다." />;
  const max = Math.max(telemetry?.total_budget_ms ?? 1, ...events.map((event) => event.elapsed_from_job_start_ms), 1);
  const runKeys = [...new Set(events.filter((event) => event.solver_id && event.run_id).map((event) => `${event.solver_id}#${event.run_id}`))];
  const decisions = events.filter((event) => event.event_type === "SCHEDULER_DECISION");
  const pageSize = 50;
  const pages = Math.max(1, Math.ceil(runKeys.length / pageSize));
  const safePage = Math.min(page, pages - 1);
  const visibleRunKeys = runKeys.slice(safePage * pageSize, (safePage + 1) * pageSize);
  const decisionStep = Math.max(1, Math.ceil(decisions.length / 200));
  const visibleDecisions = decisions.filter((_, index) => index % decisionStep === 0 || index + 1 === decisions.length);
  return <section className="portfolio-panel" aria-labelledby="scheduling-timeline-title">
    <header className="portfolio-panel-heading"><div><h2 id="scheduling-timeline-title" className={Classes.HEADING}>Scheduling Timeline</h2><p className={Classes.TEXT_MUTED}>논리 CPU slot 예약과 wall-clock 실행 구간이며 measured CPU와는 다른 지표입니다.</p></div><Tag minimal>{telemetry?.logical_cpu_slots ?? 0} logical slots</Tag></header>
    <div className="gantt-chart" role="img" aria-label="Solver scheduling Gantt timeline">
      <div className="gantt-deadline" style={{ left: "100%" }}><span>deadline</span></div>
      {visibleDecisions.map((event) => <div key={event.event_sequence} className="gantt-decision" style={{ left: `${event.elapsed_from_job_start_ms / max * 100}%` }} title={`${event.payload.reason ?? "Scheduler decision"}${event.payload.selected_arm ? ` · ${event.payload.selected_arm}` : ""}`} />)}
      {visibleRunKeys.map((key) => {
        const [solverId, run] = key.split("#");
        const related = events.filter((event) => event.solver_id === solverId && String(event.run_id) === run);
        const started = related.find((event) => event.event_type === "RUN_STARTED");
        const ended = related.find(terminal);
        const end = ended?.elapsed_from_job_start_ms ?? max;
        const start = started?.elapsed_from_job_start_ms ?? Math.max(0, end - (ended?.payload.wall_ms ?? 0));
        const width = Math.max(0.8, (end - start) / max * 100);
        const status = ended?.payload.status ?? (started ? "RUNNING" : "QUEUED");
        return <div className="gantt-row" key={key}><span>{solverId}<small>Run {run}</small></span><div className="gantt-track"><div className={`gantt-bar status-${status.toLowerCase()}`} style={{ left: `${start / max * 100}%`, width: `${width}%` }} title={`${status} · ${end - start} ms`}><span>{ended?.payload.logical_cpu_slots ?? ended?.payload.configured_threads ?? 1} slot</span></div></div></div>;
      })}
      <div className="gantt-axis"><span>0</span><span>{Math.round(max / 2)} ms</span><span>{max} ms</span></div>
    </div>
    <div className="solver-table-controls"><span className={Classes.TEXT_MUTED}>{runKeys.length} runs · {decisions.length} decisions{decisionStep > 1 ? ` (markers sampled 1/${decisionStep})` : ""}</span>{pages > 1 && <ButtonGroup><Button disabled={safePage === 0} onClick={() => setPage((value) => Math.max(0, value - 1))}>Previous</Button><Button disabled>Page {safePage + 1} / {pages}</Button><Button disabled={safePage + 1 >= pages} onClick={() => setPage((value) => Math.min(pages - 1, value + 1))}>Next</Button></ButtonGroup>}</div>
  </section>;
}
