import { useMemo, useState } from "react";
import { Button, ButtonGroup, Classes, NonIdealState, Tag } from "@blueprintjs/core";
import type {
  PortfolioTelemetrySummary,
  SolverCandidate,
  SolverCapabilities,
} from "../../api";

interface Props {
  candidates: SolverCandidate[];
  telemetry?: PortfolioTelemetrySummary;
  capabilities?: SolverCapabilities;
}

const value = (number: number | undefined, suffix = "") =>
  number === undefined ? "측정 안 됨" : `${number.toLocaleString()}${suffix}`;

export function SolverComparison({ candidates, telemetry, capabilities }: Props) {
  const runs = telemetry?.runs ?? [];
  const [page, setPage] = useState(0);
  const [feasibleOnly, setFeasibleOnly] = useState(false);
  const pageSize = 50;
  const orderedRuns = useMemo(() => [...runs]
    .filter((run) => !feasibleOnly || run.feasible)
    .sort((left, right) => Number(right.feasible) - Number(left.feasible)
      || Number(right.selected) - Number(left.selected)
      || left.wall_ms - right.wall_ms
      || left.solver_id.localeCompare(right.solver_id)
      || left.run_id - right.run_id), [runs, feasibleOnly]);
  const orderedCandidates = useMemo(() => [...candidates]
    .filter((candidate) => !feasibleOnly || candidate.feasible)
    .sort((left, right) => Number(right.feasible) - Number(left.feasible)
      || Number(right.best) - Number(left.best)
      || left.elapsed_ms - right.elapsed_ms
      || left.strategy.localeCompare(right.strategy)), [candidates, feasibleOnly]);
  const totalRows = runs.length ? orderedRuns.length : orderedCandidates.length;
  const pages = Math.max(1, Math.ceil(totalRows / pageSize));
  const safePage = Math.min(page, pages - 1);
  if (!candidates.length && !runs.length) {
    return (
      <NonIdealState
        icon="comparison"
        title="Solver telemetry 없음"
        description="이 Job은 최종 결과만 기록했거나 이전 버전에서 생성되었습니다."
      />
    );
  }
  return (
    <section className="portfolio-panel" aria-labelledby="solver-comparison-title">
      <header className="portfolio-panel-heading">
        <div>
          <h2 id="solver-comparison-title" className={Classes.HEADING}>Solver Comparison</h2>
          <p className={Classes.TEXT_MUTED}>공통 validator를 통과한 목적함수와 실제 수집된 리소스 지표입니다.</p>
        </div>
        {telemetry && <Tag minimal>{telemetry.policy} · seed {telemetry.seed}</Tag>}
      </header>
      <div className="solver-table-controls"><Button active={feasibleOnly} icon="filter" size="small" onClick={() => { setFeasibleOnly((value) => !value); setPage(0); }}>Feasible only</Button><span className={Classes.TEXT_MUTED}>{totalRows.toLocaleString()} rows</span></div>
      <div className="benchmark-table-scroll">
        <table className={`${Classes.HTML_TABLE} ${Classes.HTML_TABLE_BORDERED} ${Classes.HTML_TABLE_STRIPED} portfolio-run-table`}>
          <thead><tr><th>Solver / Run</th><th>Status</th><th>Feasible</th><th>Selected</th><th>Wall</th><th>CPU</th><th>Slots</th><th>Threads</th><th>Visited</th><th>Priority</th><th>Start</th><th>Finish</th><th>Travel</th><th>Wait</th><th>Proof</th></tr></thead>
          <tbody>
            {runs.length ? orderedRuns.slice(safePage * pageSize, (safePage + 1) * pageSize).map((run) => <tr key={`${run.solver_id}-${run.run_id}`}>
              <td><strong>{run.solver_id}</strong><small>Run {run.run_id}</small></td>
              <td>{run.status}</td><td>{run.feasible ? "Yes" : "No"}</td><td>{run.selected ? "★" : "—"}</td>
              <td>{value(run.wall_ms, " ms")}</td><td>{value(run.cpu_ms, " ms")}</td><td>{value(run.reserved_slot_ms, " slot-ms")}</td><td>{run.configured_threads}</td>
              <td>{run.objective?.visited_optional_count ?? "—"}</td><td>{run.objective?.visited_optional_priority ?? "—"}</td><td>{run.objective?.selected_start ?? "—"}</td><td>{run.objective?.finish ?? "—"}</td><td>{run.objective?.travel ?? "—"}</td><td>{run.objective?.wait ?? "—"}</td><td>{run.proof_status ?? "N/A"}</td>
            </tr>) : orderedCandidates.slice(safePage * pageSize, (safePage + 1) * pageSize).map((candidate, index) => <tr key={`${candidate.strategy}-${candidate.run_id ?? index}`}>
              <td><strong>{candidate.solver_id ?? candidate.strategy}</strong><small>{candidate.run_id ? `Run ${candidate.run_id}` : candidate.strategy}</small></td>
              <td>{candidate.metadata.outcome ?? "COMPLETED"}</td><td>{candidate.feasible ? "Yes" : "No"}</td><td>{candidate.best ? "★" : "—"}</td>
              <td>{value(candidate.elapsed_ms, " ms")}</td><td>{value(candidate.metadata.cpu_ms, " ms")}</td><td>{value(candidate.metadata.reserved_slot_ms, " slot-ms")}</td><td>{candidate.metadata.thread_count ?? "N/A"}</td>
              <td>{candidate.objective_score?.visited_optional_count ?? "—"}</td><td>{candidate.objective_score?.visited_optional_priority ?? "—"}</td><td>{candidate.objective_score?.start_time ?? candidate.objective_score?.latest_start ?? "—"}</td><td>{candidate.objective_score?.finish_time ?? "—"}</td><td>{candidate.objective_score?.travel_minutes ?? "—"}</td><td>{candidate.objective_score?.wait_minutes ?? "—"}</td><td>{candidate.metadata.proof_status ?? "N/A"}</td>
            </tr>)}
          </tbody>
        </table>
      </div>
      {pages > 1 && <ButtonGroup className="solver-table-pagination"><Button disabled={safePage === 0} onClick={() => setPage((value) => Math.max(0, value - 1))}>Previous</Button><Button disabled>Page {safePage + 1} / {pages}</Button><Button disabled={safePage + 1 >= pages} onClick={() => setPage((value) => Math.min(pages - 1, value + 1))}>Next</Button></ButtonGroup>}
      {capabilities?.solvers.some((solver) => solver.unavailable_reason) && <div className="solver-availability-list">
        {capabilities.solvers.filter((solver) => solver.unavailable_reason).map((solver) => <Tag key={solver.id} minimal intent="warning">{solver.name}: {solver.unavailable_reason}</Tag>)}
      </div>}
    </section>
  );
}
