import { useMemo, useState, type ChangeEvent } from "react";
import { Button, Callout, Classes, Dialog, DialogBody, NonIdealState, Tag } from "@blueprintjs/core";
import type { PortfolioTelemetrySummary } from "../../api";

interface ExperimentSample {
  telemetry: PortfolioTelemetrySummary;
  experimentId?: string;
  problemHash?: string;
  matrixHash?: string;
  scenario?: string;
}

function percentile(values: number[], quantile: number) {
  if (!values.length) return undefined;
  const ordered = [...values].sort((a, b) => a - b);
  return ordered[Math.min(ordered.length - 1, Math.floor((ordered.length - 1) * quantile))];
}

function mean(values: number[]) {
  return values.length ? values.reduce((sum, value) => sum + value, 0) / values.length : undefined;
}

function percent(value?: number) {
  return value === undefined ? "N/A" : `${(value * 100).toFixed(1)}%`;
}

function milliseconds(value?: number) {
  return value === undefined ? "N/A" : `${Math.round(value)} ms`;
}

function wilson(successes: number, total: number) {
  if (!total) return undefined;
  const z = 1.96;
  const ratio = successes / total;
  const denominator = 1 + z * z / total;
  const center = (ratio + z * z / (2 * total)) / denominator;
  const margin = z * Math.sqrt((ratio * (1 - ratio) + z * z / (4 * total)) / total) / denominator;
  return [Math.max(0, center - margin), Math.min(1, center + margin)] as const;
}

function comparisonSignature(sample: ExperimentSample) {
  const summary = sample.telemetry;
  return [sample.problemHash ?? "unknown-problem", sample.matrixHash ?? "unknown-matrix", summary.total_budget_ms, summary.logical_cpu_slots].join(":");
}

export function ExperimentDashboard({ current, dark, isOpen, onClose }: { current?: PortfolioTelemetrySummary; dark: boolean; isOpen: boolean; onClose: () => void }) {
  const [imported, setImported] = useState<ExperimentSample[]>([]);
  const [selectedPolicy, setSelectedPolicy] = useState<string>();
  const samples = useMemo<ExperimentSample[]>(() => [...(current ? [{ telemetry: current }] : []), ...imported], [current, imported]);
  const signatures = useMemo(() => new Set(samples.map(comparisonSignature)), [samples]);
  const groups = useMemo(() => {
    const grouped = samples.reduce((result, sample) => {
      const records = result.get(sample.telemetry.policy) ?? [];
      records.push(sample);
      result.set(sample.telemetry.policy, records);
      return result;
    }, new Map<string, ExperimentSample[]>());
    return [...grouped.entries()].map(([policy, records]) => {
      const summaries = records.map((record) => record.telemetry);
      const runs = summaries.flatMap((summary) => summary.runs);
      const selected = runs.filter((run) => run.selected);
      const feasible = summaries.filter((summary) => summary.runs.some((run) => run.feasible)).length;
      const wall = runs.map((run) => run.wall_ms);
      const cpu = runs.flatMap((run) => run.cpu_ms === undefined ? [] : [run.cpu_ms]);
      const firstFeasible = summaries.flatMap((summary) => {
        const event = summary.events.find((item) => item.event_type === "FIRST_FEASIBLE");
        return event ? [event.elapsed_from_job_start_ms] : [];
      });
      const objectives = selected.flatMap((run) => run.objective ? [run.objective] : []);
      return {
        policy,
        records,
        count: records.length,
        feasible,
        feasibleInterval: wilson(feasible, records.length),
        wallP50: percentile(wall, 0.5),
        wallP95: percentile(wall, 0.95),
        wallWorst: wall.length ? Math.max(...wall) : undefined,
        cpuP50: percentile(cpu, 0.5),
        cpuP95: percentile(cpu, 0.95),
        firstP50: percentile(firstFeasible, 0.5),
        coverage: mean(objectives.map((objective) => objective.visited_optional_count)),
        priority: mean(objectives.map((objective) => objective.visited_optional_priority)),
        travel: mean(objectives.map((objective) => objective.travel)),
        reservedSlotMs: runs.reduce((sum, run) => sum + run.reserved_slot_ms, 0),
        wins: selected.length,
        eliminationCount: summaries.flatMap((summary) => summary.events).filter((event) => event.event_type === "ARM_ELIMINATED").length,
      };
    });
  }, [samples]);
  const selected = groups.find((group) => group.policy === selectedPolicy);

  async function importFile(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    if (!file) return;
    const raw = await file.text();
    const values: unknown[] = (() => {
      try {
        const parsed = JSON.parse(raw) as unknown;
        return Array.isArray(parsed) ? parsed : [parsed];
      } catch {
        return raw.split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line) as unknown);
      }
    })();
    const telemetry = values.flatMap((value): ExperimentSample[] => {
      if (typeof value !== "object" || value === null) return [];
      const record = value as { telemetry?: PortfolioTelemetrySummary; policy?: string; runs?: unknown[]; experiment_id?: string; problem_hash?: string; matrix_hash?: string; scenario?: string };
      const summary = record.telemetry ?? (record.policy && Array.isArray(record.runs) ? value as PortfolioTelemetrySummary : undefined);
      return summary ? [{ telemetry: summary, experimentId: record.experiment_id, problemHash: record.problem_hash, matrixHash: record.matrix_hash, scenario: record.scenario }] : [];
    });
    setImported((currentValues) => [...currentValues, ...telemetry]);
    event.target.value = "";
  }

  return <Dialog className="experiment-dialog" isOpen={isOpen} onClose={onClose} portalClassName={dark ? Classes.DARK : undefined} title="Portfolio Experiment Dashboard" icon="heat-grid"><DialogBody>
    <div className="portfolio-panel-heading"><div><h2 className={Classes.HEADING}>Policy experiments</h2><p className={Classes.TEXT_MUTED}>동일 problem/matrix hash, budget, slots 조건만 직접 비교합니다. Seed는 반복 표본으로 집계합니다.</p></div><Button icon="import"><label className="file-button">JSON 가져오기<input type="file" accept="application/json,.json,.jsonl" onChange={(event) => void importFile(event)}/></label></Button></div>
    {signatures.size > 1 && <Callout intent="warning" title="비교 조건 불일치">서로 다른 problem/matrix hash 또는 budget/slot 결과가 섞였습니다. 아래 수치는 표시하되 정책 우열 근거로 사용하지 마세요.</Callout>}
    {!groups.length ? <NonIdealState icon="database" title="실험 데이터 없음" description="portfolio_benchmark의 runs.jsonl을 가져오거나 telemetry가 있는 Job을 선택하세요."/> : <div className="benchmark-table-scroll"><table className={`${Classes.HTML_TABLE} ${Classes.HTML_TABLE_BORDERED} ${Classes.HTML_TABLE_STRIPED}`}><thead><tr><th>Policy</th><th>N</th><th>Feasible (95% CI)</th><th>Coverage / Priority</th><th>Travel mean</th><th>First feasible p50</th><th>Wall p50 / p95 / worst</th><th>CPU p50 / p95</th><th>Reserved slot-ms</th><th>Selected</th><th>Eliminated</th><th>Gap / regret</th></tr></thead><tbody>{groups.map((group) => <tr key={group.policy} onClick={() => setSelectedPolicy(group.policy)} className={selectedPolicy === group.policy ? "is-selected" : undefined}><td><Tag minimal>{group.policy}</Tag></td><td>{group.count}</td><td>{percent(group.count ? group.feasible/group.count : undefined)} {group.feasibleInterval ? <small>({percent(group.feasibleInterval[0])}–{percent(group.feasibleInterval[1])})</small> : null}</td><td>{group.coverage?.toFixed(2) ?? "N/A"} / {group.priority?.toFixed(2) ?? "N/A"}</td><td>{group.travel?.toFixed(1) ?? "N/A"}</td><td>{milliseconds(group.firstP50)}</td><td>{milliseconds(group.wallP50)} / {milliseconds(group.wallP95)} / {milliseconds(group.wallWorst)}</td><td>{milliseconds(group.cpuP50)} / {milliseconds(group.cpuP95)}</td><td>{group.reservedSlotMs.toLocaleString()}</td><td>{group.wins}</td><td>{group.eliminationCount}</td><td>N/A</td></tr>)}</tbody></table></div>}
    {selected && <section className="experiment-drilldown"><h3>{selected.policy} drill-down</h3><p className={Classes.TEXT_MUTED}>SCIP phase bound는 공통 optimality gap으로 환산하지 않습니다. CPU/RSS 미지원 값은 N/A입니다.</p><ul>{selected.records.map((record, index) => <li key={`${record.experimentId ?? "job"}-${index}`}><code>{record.experimentId ?? `sample-${index + 1}`}</code> · seed {record.telemetry.seed} · {record.scenario ?? "current job"} · {record.telemetry.runs.length} runs</li>)}</ul></section>}
  </DialogBody></Dialog>;
}
