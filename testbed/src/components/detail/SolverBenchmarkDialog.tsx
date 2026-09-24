import { useEffect, useMemo, useState } from "react";
import {
  Button,
  Callout,
  Classes,
  Collapse,
  Dialog,
  DialogBody,
  Intent,
  NonIdealState,
  Tag,
} from "@blueprintjs/core";
import type { SolverCandidate, SolverDiagnostics } from "../../api";

interface SolverBenchmarkDialogProps {
  candidates: SolverCandidate[];
  diagnostics?: SolverDiagnostics;
  locationCount: number;
  dark: boolean;
  isOpen: boolean;
  onClose: () => void;
}

type BenchmarkView =
  | "overview"
  | "exact"
  | "clustered"
  | "greedy"
  | "mst"
  | "christofides"
  | "sa-greedy"
  | "sa-mst"
  | "sa-christofides"
  | "sa-clustered"
  | "sa-warm-start";

interface StrategyNavigationItem {
  id: BenchmarkView;
  label: string;
  matches: (strategy: string) => boolean;
}

const STRATEGY_NAVIGATION: StrategyNavigationItem[] = [
  { id: "exact", label: "Exact Bit DP", matches: (value) => value === "exact_bit_dp" },
  { id: "clustered", label: "Clustered", matches: (value) => value === "clustered" },
  { id: "greedy", label: "Greedy", matches: (value) => value === "greedy" },
  { id: "mst", label: "MST Double-Tree", matches: (value) => value === "mst_double_tree" },
  { id: "christofides", label: "Christofides", matches: (value) => value === "christofides" },
  { id: "sa-greedy", label: "SA (Greedy)", matches: (value) => value.startsWith("sa_greedy_") },
  { id: "sa-mst", label: "SA (MST)", matches: (value) => value.startsWith("sa_mst_") },
  { id: "sa-christofides", label: "SA (Christofides)", matches: (value) => value.startsWith("sa_christofides_") },
  { id: "sa-clustered", label: "SA (Clustered)", matches: (value) => value.startsWith("sa_clustered_") },
  { id: "sa-warm-start", label: "SA (Warm Start)", matches: (value) => value.startsWith("sa_warm_start_") },
];

function scoreValue(candidate: SolverCandidate, field: "latest_start" | "finish_time") {
  return candidate.objective_score?.[field] ?? "—";
}

function minuteValue(candidate: SolverCandidate, field: "travel_minutes" | "wait_minutes") {
  const value = candidate.objective_score?.[field];
  return value === undefined ? "—" : `${value}분`;
}

function saRunNumber(strategy: string) {
  const match = strategy.match(/_run_(\d+)_/);
  return match ? Number(match[1]) : Number.MAX_SAFE_INTEGER;
}

function CandidateFacts({ candidate }: { candidate: SolverCandidate }) {
  const facts = [
    ["Strategy", candidate.strategy],
    ["Feasible", candidate.feasible ? "Yes" : "No"],
    ["Selected", candidate.best ? "Yes · ★ Best" : "No"],
    ["Runtime", `${candidate.elapsed_ms} ms`],
    ["Latest Start", scoreValue(candidate, "latest_start")],
    ["Finish", scoreValue(candidate, "finish_time")],
    ["Travel", minuteValue(candidate, "travel_minutes")],
    ["Wait", minuteValue(candidate, "wait_minutes")],
  ];
  return (
    <dl className="benchmark-facts">
      {facts.map(([label, value]) => (
        <div key={label}>
          <dt>{label}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function ExactBitDpDetail({ candidates, locationCount }: { candidates: SolverCandidate[]; locationCount: number }) {
  const candidate = candidates.find((value) => value.strategy === "exact_bit_dp");
  const [rawOpen, setRawOpen] = useState(false);
  useEffect(() => setRawOpen(false), [candidate?.strategy]);

  if (!candidate) {
    return <div className="benchmark-strategy-detail"><h2 className={Classes.HEADING}>Exact Bit DP</h2><NonIdealState icon="info-sign" title="실행 결과 없음" description="Location 수가 exact limit를 초과했거나 Exact Bit DP가 실행되지 않았습니다." /></div>;
  }

  const generatedStates = candidate.metadata.state_count;
  const frontierStates = candidate.metadata.frontier_state_count;
  const pruningRatio = generatedStates && frontierStates !== undefined
    ? `${Math.max(0, (1 - frontierStates / generatedStates) * 100).toFixed(1)}%`
    : "—";
  const summary = [
    ["Feasible", candidate.feasible ? "Yes" : "No"],
    ["Selected", candidate.best ? "Yes · ★ Best" : "No"],
    ["Runtime", `${candidate.elapsed_ms} ms`],
    ["Latest Start", scoreValue(candidate, "latest_start")],
    ["Finish", scoreValue(candidate, "finish_time")],
    ["Travel", minuteValue(candidate, "travel_minutes")],
    ["Wait", minuteValue(candidate, "wait_minutes")],
  ];
  const statistics = [
    ["Generated States", generatedStates?.toLocaleString() ?? "—"],
    ["Frontier States", frontierStates?.toLocaleString() ?? "—"],
    ["Frontier Cells", candidate.metadata.frontier_cell_count?.toLocaleString() ?? "—"],
    ["Pruning Ratio", pruningRatio],
    ["Location Count", locationCount.toLocaleString()],
    ["Time Slot Size", "10 min"],
  ];
  const objective = [
    ["Latest Start", scoreValue(candidate, "latest_start")],
    ["Finish", scoreValue(candidate, "finish_time")],
    ["Travel", minuteValue(candidate, "travel_minutes")],
    ["Wait", minuteValue(candidate, "wait_minutes")],
  ];
  const metricGrid = (values: string[][]) => <dl className="exact-metrics-grid">{values.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>;

  return (
    <div className="benchmark-strategy-detail exact-bit-dp-detail">
      <div className="benchmark-detail-heading"><div><h2 className={Classes.HEADING}>Exact Bit DP</h2><code>{candidate.strategy}</code></div>{candidate.best && <Tag intent={Intent.PRIMARY} minimal>★ Best</Tag>}</div>
      <section><h3>Summary</h3>{metricGrid(summary)}</section>
      <section><h3>DP Statistics</h3>{metricGrid(statistics)}<p className={`${Classes.TEXT_MUTED} pruning-help`}>Pruning Ratio = 1 − Frontier States / Generated States</p></section>
      <section><h3>Route</h3><div className="exact-route" aria-label="Exact Bit DP Route">{candidate.route.length ? candidate.route.join(" → ") : "—"}</div></section>
      <section><h3>Objective</h3>{metricGrid(objective)}</section>
      <section className="raw-diagnostics">
        <Button alignText="left" fill icon="code" endIcon={rawOpen ? "chevron-up" : "chevron-down"} variant="minimal" onClick={() => setRawOpen((open) => !open)}>Raw Diagnostics</Button>
        <Collapse isOpen={rawOpen}><pre className={`${Classes.CODE_BLOCK} raw-diagnostics-output`} aria-label="Exact Bit DP Raw Diagnostics">{JSON.stringify(candidate.metadata, null, 2)}</pre></Collapse>
      </section>
    </div>
  );
}

function ClusteredDetail({ candidates }: { candidates: SolverCandidate[] }) {
  const candidate = candidates.find((value) => value.strategy === "clustered");
  const [rawOpen, setRawOpen] = useState(false);
  useEffect(() => setRawOpen(false), [candidate?.strategy]);

  if (!candidate) {
    return <div className="benchmark-strategy-detail"><h2 className={Classes.HEADING}>Clustered</h2><NonIdealState icon="info-sign" title="실행 결과 없음" description="이 Job에서는 Clustered solver가 실행되지 않았거나 결과를 생성하지 못했습니다." /></div>;
  }

  const metadata = candidate.metadata;
  const details = metadata.cluster_details ?? [];
  const summary = [
    ["Feasible", candidate.feasible ? "Yes" : "No"],
    ["Selected", candidate.best ? "Yes · ★ Best" : "No"],
    ["Runtime", `${candidate.elapsed_ms} ms`],
    ["Latest Start", scoreValue(candidate, "latest_start")],
    ["Finish", scoreValue(candidate, "finish_time")],
    ["Travel", minuteValue(candidate, "travel_minutes")],
    ["Wait", minuteValue(candidate, "wait_minutes")],
  ];
  const overview = [
    ["Cluster Count", metadata.cluster_count?.toLocaleString() ?? "—"],
    ["Cluster Sizes", metadata.cluster_sizes?.join(" / ") ?? "—"],
    ["Cluster Strategy", metadata.cluster_strategy ?? "—"],
    ["Order Strategy", metadata.cluster_order_strategy ?? "—"],
  ];
  const improvement = [
    ["Before Score", metadata.score_before_improvement?.toLocaleString() ?? "—"],
    ["After Score", metadata.score_after_improvement?.toLocaleString() ?? "—"],
    ["Local Improvement Count", metadata.improved_moves?.toLocaleString() ?? "—"],
    ["Improvement Strategy", metadata.improvement_strategy ?? "—"],
    ["Swap", metadata.swap_enabled === undefined ? "—" : metadata.swap_enabled ? "Yes" : "No"],
    ["Relocate", metadata.relocate_enabled === undefined ? "—" : metadata.relocate_enabled ? "Yes" : "No"],
    ["2-opt", metadata.two_opt_enabled === undefined ? "—" : metadata.two_opt_enabled ? "Yes" : "No"],
  ];
  const metricGrid = (values: string[][]) => <dl className="exact-metrics-grid">{values.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>;

  return (
    <div className="benchmark-strategy-detail clustered-detail">
      <div className="benchmark-detail-heading"><div><h2 className={Classes.HEADING}>Clustered</h2><code>{candidate.strategy}</code></div>{candidate.best && <Tag intent={Intent.PRIMARY} minimal>★ Best</Tag>}</div>
      <section><h3>Summary</h3>{metricGrid(summary)}</section>
      <section>
        <h3>Cluster Overview</h3>
        {metricGrid(overview)}
        <div className="cluster-members" aria-label="Cluster별 장소 목록">
          {details.length ? [...details].sort((left, right) => left.cluster - right.cluster).map((cluster) => <div key={cluster.cluster}><strong>Cluster {cluster.cluster}</strong><span>{cluster.members.join(", ") || "—"}</span></div>) : <p className={Classes.TEXT_MUTED}>Cluster별 장소 정보가 없습니다.</p>}
        </div>
      </section>
      <section><h3>Cluster Order</h3><div className="exact-route" aria-label="Cluster Order">{metadata.cluster_order?.length ? metadata.cluster_order.map((cluster) => `Cluster ${cluster}`).join(" → ") : "—"}</div></section>
      <section>
        <h3>Cluster Detail</h3>
        <div className="cluster-detail-list">
          {details.length ? details.map((cluster) => <article className="cluster-detail-card" key={cluster.cluster}>
            <div className="cluster-card-heading"><strong>Cluster {cluster.cluster}</strong><span>{cluster.members.length} locations</span></div>
            <dl>
              <div><dt>Entry</dt><dd>{cluster.entry ?? "—"}</dd></div>
              <div><dt>Exit</dt><dd>{cluster.exit ?? "—"}</dd></div>
              <div className="cluster-route-row"><dt>Internal Route</dt><dd>{cluster.route.length ? cluster.route.join(" → ") : "—"}</dd></div>
              <div><dt>Exact Generated</dt><dd>{cluster.state_count.toLocaleString()}</dd></div>
              <div><dt>Exact Frontier</dt><dd>{cluster.frontier_state_count.toLocaleString()}</dd></div>
            </dl>
          </article>) : <NonIdealState icon="info-sign" title="Cluster 상세 정보 없음" description="이 결과는 상세 cluster diagnostics를 포함하지 않습니다." />}
        </div>
      </section>
      <section><h3>Merge / Improvement</h3>{metricGrid(improvement)}</section>
      <section><h3>Final Route</h3><div className="exact-route" aria-label="Clustered Final Route">{candidate.route.length ? candidate.route.join(" → ") : "—"}</div></section>
      <section className="raw-diagnostics">
        <Button alignText="left" fill icon="code" endIcon={rawOpen ? "chevron-up" : "chevron-down"} variant="minimal" onClick={() => setRawOpen((open) => !open)}>Raw Diagnostics</Button>
        <Collapse isOpen={rawOpen}><pre className={`${Classes.CODE_BLOCK} raw-diagnostics-output`} aria-label="Clustered Raw Diagnostics">{JSON.stringify(metadata, null, 2)}</pre></Collapse>
      </section>
    </div>
  );
}

function MstDoubleTreeDetail({ candidates }: { candidates: SolverCandidate[] }) {
  const candidate = candidates.find((value) => value.strategy === "mst_double_tree");
  const [rawOpen, setRawOpen] = useState(false);
  useEffect(() => setRawOpen(false), [candidate?.strategy]);

  if (!candidate) {
    return <div className="benchmark-strategy-detail"><h2 className={Classes.HEADING}>MST Double-Tree</h2><NonIdealState icon="info-sign" title="실행 결과 없음" description="이 Job에서는 MST Double-Tree가 실행되지 않았거나 결과를 생성하지 못했습니다." /></div>;
  }

  const metadata = candidate.metadata;
  const summary = [
    ["Feasible", candidate.feasible ? "Yes" : "No"],
    ["Selected", candidate.best ? "Yes · ★ Best" : "No"],
    ["Runtime", `${candidate.elapsed_ms} ms`],
    ["Latest Start", scoreValue(candidate, "latest_start")],
    ["Finish", scoreValue(candidate, "finish_time")],
    ["Travel", minuteValue(candidate, "travel_minutes")],
    ["Wait", minuteValue(candidate, "wait_minutes")],
  ];
  const mst = [
    ["Symmetric Distance Strategy", metadata.symmetric_distance_strategy ?? "—"],
    ["MST Cost", metadata.mst_cost?.toLocaleString() ?? "—"],
    ["MST Edge Count", metadata.mst_edge_count?.toLocaleString() ?? "—"],
  ];
  const evaluation = [
    ["Travel", minuteValue(candidate, "travel_minutes")],
    ["Wait", minuteValue(candidate, "wait_minutes")],
    ["Latest Start", scoreValue(candidate, "latest_start")],
    ["Finish", scoreValue(candidate, "finish_time")],
    ["Feasible", candidate.feasible ? "Yes" : "No"],
  ];
  const metricGrid = (values: string[][]) => <dl className="exact-metrics-grid">{values.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>;

  return (
    <div className="benchmark-strategy-detail mst-double-tree-detail">
      <div className="benchmark-detail-heading"><div><h2 className={Classes.HEADING}>MST Double-Tree</h2><code>{candidate.strategy}</code></div>{candidate.best && <Tag intent={Intent.PRIMARY} minimal>★ Best</Tag>}</div>
      <section><h3>Summary</h3>{metricGrid(summary)}</section>
      <section>
        <h3>MST</h3>
        {metricGrid(mst)}
        <div className="benchmark-table-scroll">
          <table className={`${Classes.HTML_TABLE} ${Classes.HTML_TABLE_BORDERED} ${Classes.HTML_TABLE_STRIPED} mst-edge-table`} aria-label="MST Edges">
            <thead><tr><th>From</th><th>To</th><th>Symmetric Distance</th></tr></thead>
            <tbody>{metadata.mst_edges?.length ? metadata.mst_edges.map((edge, index) => <tr key={`${edge.from}-${edge.to}-${index}`}><td>{edge.from}</td><td>{edge.to}</td><td>{edge.distance}</td></tr>) : <tr><td colSpan={3} className={Classes.TEXT_MUTED}>MST edge 정보가 없습니다.</td></tr>}</tbody>
          </table>
        </div>
      </section>
      <section><h3>Euler Tour</h3><div className="exact-route" aria-label="MST Euler Tour">{metadata.euler_tour?.length ? metadata.euler_tour.join(" → ") : "—"}</div></section>
      <section><h3>Shortcut Result</h3><div className="exact-route" aria-label="MST Shortcut Result">{metadata.shortcut_route?.length ? metadata.shortcut_route.join(" → ") : candidate.route.length ? candidate.route.join(" → ") : "—"}</div></section>
      <section><h3>Original Directed Matrix Evaluation</h3>{metricGrid(evaluation)}</section>
      <Callout className="approximation-notice" intent={Intent.WARNING} icon="info-sign" title="Approximation Guarantee">2-approx 보장은 symmetric metric TSP에서만 성립합니다. 위 실제 평가는 original directed TravelTimeMatrix와 시간 제약을 사용합니다.</Callout>
      <section className="raw-diagnostics">
        <Button alignText="left" fill icon="code" endIcon={rawOpen ? "chevron-up" : "chevron-down"} variant="minimal" onClick={() => setRawOpen((open) => !open)}>Raw Diagnostics</Button>
        <Collapse isOpen={rawOpen}><pre className={`${Classes.CODE_BLOCK} raw-diagnostics-output`} aria-label="MST Double-Tree Raw Diagnostics">{JSON.stringify(metadata, null, 2)}</pre></Collapse>
      </section>
    </div>
  );
}

function ChristofidesDetail({ candidates }: { candidates: SolverCandidate[] }) {
  const candidate = candidates.find((value) => value.strategy === "christofides");
  const [rawOpen, setRawOpen] = useState(false);
  useEffect(() => setRawOpen(false), [candidate?.strategy]);

  if (!candidate) {
    return <div className="benchmark-strategy-detail"><h2 className={Classes.HEADING}>Christofides</h2><NonIdealState icon="info-sign" title="실행 결과 없음" description="이 Job에서는 Christofides가 실행되지 않았거나 결과를 생성하지 못했습니다." /></div>;
  }

  const metadata = candidate.metadata;
  const matchingStrategy = metadata.matching_strategy === "bit_dp" ? "BitDP" : metadata.matching_strategy === "blossom" ? "Blossom" : metadata.matching_strategy ?? "—";
  const summary = [
    ["Feasible", candidate.feasible ? "Yes" : "No"],
    ["Selected", candidate.best ? "Yes · ★ Best" : "No"],
    ["Runtime", `${candidate.elapsed_ms} ms`],
    ["Latest Start", scoreValue(candidate, "latest_start")],
    ["Finish", scoreValue(candidate, "finish_time")],
    ["Travel", minuteValue(candidate, "travel_minutes")],
    ["Wait", minuteValue(candidate, "wait_minutes")],
  ];
  const mst = [
    ["MST Cost", metadata.mst_cost?.toLocaleString() ?? "—"],
    ["MST Edge Count", metadata.mst_edge_count?.toLocaleString() ?? "—"],
  ];
  const matching = [
    ["Matching Strategy", matchingStrategy],
    ["Matching Cost", metadata.matching_cost?.toLocaleString() ?? "—"],
    ["Matching Pair Count", metadata.matching_pairs?.length.toLocaleString() ?? "—"],
  ];
  const evaluation = [
    ["Travel", minuteValue(candidate, "travel_minutes")],
    ["Wait", minuteValue(candidate, "wait_minutes")],
    ["Latest Start", scoreValue(candidate, "latest_start")],
    ["Finish", scoreValue(candidate, "finish_time")],
    ["Feasible", candidate.feasible ? "Yes" : "No"],
  ];
  const metricGrid = (values: string[][]) => <dl className="exact-metrics-grid">{values.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>;

  return (
    <div className="benchmark-strategy-detail christofides-detail">
      <div className="benchmark-detail-heading"><div><h2 className={Classes.HEADING}>Christofides</h2><code>{candidate.strategy}</code></div>{candidate.best && <Tag intent={Intent.PRIMARY} minimal>★ Best</Tag>}</div>
      <section><h3>Summary</h3>{metricGrid(summary)}</section>
      <section>
        <h3>MST</h3>
        {metricGrid(mst)}
        <div className="benchmark-table-scroll"><table className={`${Classes.HTML_TABLE} ${Classes.HTML_TABLE_BORDERED} ${Classes.HTML_TABLE_STRIPED} mst-edge-table`} aria-label="Christofides MST Edges"><thead><tr><th>From</th><th>To</th><th>Symmetric Distance</th></tr></thead><tbody>{metadata.mst_edges?.length ? metadata.mst_edges.map((edge, index) => <tr key={`${edge.from}-${edge.to}-${index}`}><td>{edge.from}</td><td>{edge.to}</td><td>{edge.distance}</td></tr>) : <tr><td colSpan={3} className={Classes.TEXT_MUTED}>MST edge 정보가 없습니다.</td></tr>}</tbody></table></div>
      </section>
      <section><h3>Odd Vertices</h3>{metricGrid([["Odd Vertex Count", metadata.odd_vertex_count?.toLocaleString() ?? "—"]])}<div className="exact-route" aria-label="Christofides Odd Vertices">{metadata.odd_vertices?.length ? metadata.odd_vertices.join(" · ") : "—"}</div></section>
      <section>
        <h3>Perfect Matching</h3>
        {metricGrid(matching)}
        <div className="benchmark-table-scroll"><table className={`${Classes.HTML_TABLE} ${Classes.HTML_TABLE_BORDERED} ${Classes.HTML_TABLE_STRIPED} mst-edge-table`} aria-label="Christofides Matching Pairs"><thead><tr><th>Left</th><th>Right</th><th>Symmetric Distance</th></tr></thead><tbody>{metadata.matching_pairs?.length ? metadata.matching_pairs.map((pair, index) => <tr key={`${pair.left}-${pair.right}-${index}`}><td>{pair.left}</td><td>{pair.right}</td><td>{pair.distance}</td></tr>) : <tr><td colSpan={3} className={Classes.TEXT_MUTED}>Matching pair 정보가 없습니다.</td></tr>}</tbody></table></div>
      </section>
      <section><h3>Euler / Shortcut</h3><div className="route-stage"><span className={Classes.TEXT_MUTED}>Euler Tour</span><div className="exact-route" aria-label="Christofides Euler Tour">{metadata.euler_tour?.length ? metadata.euler_tour.join(" → ") : "—"}</div></div><div className="route-stage"><span className={Classes.TEXT_MUTED}>Hamiltonian Route</span><div className="exact-route" aria-label="Christofides Shortcut Route">{metadata.shortcut_route?.length ? metadata.shortcut_route.join(" → ") : candidate.route.length ? candidate.route.join(" → ") : "—"}</div></div></section>
      <section><h3>Original Directed Matrix Evaluation</h3>{metricGrid(evaluation)}</section>
      <Callout className="approximation-notice" intent={Intent.WARNING} icon="info-sign" title="Approximation Guarantee">1.5-approx 보장은 symmetric metric TSP에서만 성립합니다. 위 실제 평가는 original directed TravelTimeMatrix와 시간 제약을 사용합니다.</Callout>
      <section className="raw-diagnostics">
        <Button alignText="left" fill icon="code" endIcon={rawOpen ? "chevron-up" : "chevron-down"} variant="minimal" onClick={() => setRawOpen((open) => !open)}>Raw Diagnostics</Button>
        <Collapse isOpen={rawOpen}><pre className={`${Classes.CODE_BLOCK} raw-diagnostics-output`} aria-label="Christofides Raw Diagnostics">{JSON.stringify(metadata, null, 2)}</pre></Collapse>
      </section>
    </div>
  );
}

function SimulatedAnnealingDetail({ item, candidates }: { item: StrategyNavigationItem; candidates: SolverCandidate[] }) {
  const matches = candidates
    .filter((candidate) => item.matches(candidate.strategy))
    .sort((left, right) => saRunNumber(left.strategy) - saRunNumber(right.strategy));
  const timeline = candidates
    .filter((candidate) => candidate.strategy.startsWith("sa_"))
    .sort((left, right) => saRunNumber(left.strategy) - saRunNumber(right.strategy));
  const candidate = matches.find((value) => value.best) ?? matches[0];
  const [rawOpen, setRawOpen] = useState(false);
  useEffect(() => setRawOpen(false), [item.id, candidate?.strategy]);

  if (!candidate) {
    return <div className="benchmark-strategy-detail"><h2 className={Classes.HEADING}>{item.label}</h2><NonIdealState icon="info-sign" title="실행 결과 없음" description="이 Job에서는 해당 Simulated Annealing 전략이 실행되지 않았습니다." /></div>;
  }

  const metadata = candidate.metadata;
  const summary = [
    ["Initial Strategy", metadata.initial_strategy ?? item.label.replace(/^SA \(|\)$/g, "")],
    ["Seed", metadata.seed?.toLocaleString() ?? "—"],
    ["Feasible", candidate.feasible ? "Yes" : "No"],
    ["Selected", candidate.best ? "Yes · ★ Best" : "No"],
    ["Runtime", `${candidate.elapsed_ms} ms`],
    ["Latest Start", scoreValue(candidate, "latest_start")],
    ["Finish", scoreValue(candidate, "finish_time")],
    ["Travel", minuteValue(candidate, "travel_minutes")],
    ["Wait", minuteValue(candidate, "wait_minutes")],
  ];
  const acceptanceRate = metadata.iteration_count !== undefined && metadata.accepted_moves !== undefined
    ? metadata.iteration_count === 0 ? "0.0%" : `${((metadata.accepted_moves / metadata.iteration_count) * 100).toFixed(1)}%`
    : undefined;
  const statistics = [
    ["Iterations", metadata.iteration_count?.toLocaleString()],
    ["Accepted Moves", metadata.accepted_moves?.toLocaleString()],
    ["Improved Moves", metadata.improved_moves?.toLocaleString()],
    ["Acceptance Rate", acceptanceRate],
    ["Initial Temperature", metadata.initial_temperature],
    ["Final Temperature", metadata.final_temperature],
    ["Cooling Rate", metadata.cooling_rate],
  ].filter((value): value is string[] => value[1] !== undefined);
  const improvement = metadata.initial_score !== undefined && metadata.final_score !== undefined
    ? metadata.initial_score - metadata.final_score
    : undefined;
  const scores = [
    ["Initial Score", metadata.initial_score?.toLocaleString()],
    ["Final Score", metadata.final_score?.toLocaleString()],
    ["Improvement", improvement === undefined ? undefined : `${improvement > 0 ? "+" : ""}${improvement.toLocaleString()}`],
  ].filter((value): value is string[] => value[1] !== undefined);
  const moves = [
    ["Swap Attempts", metadata.swap_move_count?.toLocaleString()],
    ["Relocate Attempts", metadata.relocate_move_count?.toLocaleString()],
    ["2-opt Attempts", metadata.two_opt_move_count?.toLocaleString()],
  ].filter((value): value is string[] => value[1] !== undefined);
  const bestFeasible = metadata.best_feasible ?? candidate.feasible;
  const metricGrid = (values: string[][]) => <dl className="exact-metrics-grid">{values.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>;

  return (
    <div className="benchmark-strategy-detail simulated-annealing-detail">
      <div className="benchmark-detail-heading"><div><h2 className={Classes.HEADING}>{item.label}</h2><code>{candidate.strategy}</code></div><div className="benchmark-detail-tags">{matches.length > 1 && <Tag minimal>{matches.length} runs 중 대표 결과</Tag>}{candidate.best && <Tag intent={Intent.PRIMARY} minimal>★ Best</Tag>}</div></div>
      <section><h3>Summary</h3>{metricGrid(summary)}</section>
      {statistics.length > 0 && <section><h3>Search Statistics</h3>{metricGrid(statistics)}</section>}
      {scores.length > 0 && <section><h3>Score Improvement</h3>{metricGrid(scores)}</section>}
      <section>
        <h3>Route Comparison</h3>
        <div className="sa-route-comparison">
          <div><span className={Classes.TEXT_MUTED}>Initial Route</span><div className="exact-route" aria-label={`${item.label} Initial Route`}>{metadata.initial_route?.length ? metadata.initial_route.join(" → ") : "—"}</div></div>
          <div><span className={Classes.TEXT_MUTED}>Final Route</span><div className="exact-route" aria-label={`${item.label} Final Route`}>{metadata.final_route?.length ? metadata.final_route.join(" → ") : candidate.route.length ? candidate.route.join(" → ") : "—"}</div></div>
        </div>
      </section>
      {moves.length > 0 && <section><h3>Move Statistics</h3>{metricGrid(moves)}</section>}
      <section>
        <h3>Run Timeline</h3>
        <div className="benchmark-table-scroll"><table className={`${Classes.HTML_TABLE} ${Classes.HTML_TABLE_BORDERED} ${Classes.HTML_TABLE_STRIPED}`} aria-label={`${item.label} Run Timeline`}>
          <thead><tr><th>Run</th><th>Initializer</th><th>Seed</th><th>Time</th><th>Initial</th><th>Final</th><th>Best Updated</th></tr></thead>
          <tbody>{timeline.map((run) => <tr key={run.strategy}><td>{saRunNumber(run.strategy)}</td><td>{run.metadata.initial_strategy ? run.metadata.initial_strategy.replace(/\b\w/g, (letter) => letter.toUpperCase()) : "—"}</td><td>{run.metadata.seed?.toLocaleString() ?? "—"}</td><td>{run.elapsed_ms} ms</td><td>{run.metadata.initial_score?.toLocaleString() ?? "—"}</td><td>{run.metadata.final_score?.toLocaleString() ?? "—"}</td><td>{run.metadata.improved_global_best ? "Yes" : "No"}</td></tr>)}</tbody>
        </table></div>
      </section>
      <section><h3>Feasibility</h3><Callout intent={bestFeasible ? Intent.SUCCESS : Intent.DANGER} icon={bestFeasible ? "tick-circle" : "error"} title={`Best Feasible Solution: ${bestFeasible ? "Yes" : "No"}`}>{bestFeasible ? "최종 결과는 탐색 중 발견한 best feasible solution입니다." : "탐색에서 feasible solution을 찾지 못했습니다."}</Callout></section>
      <section className="raw-diagnostics">
        <Button alignText="left" fill icon="code" endIcon={rawOpen ? "chevron-up" : "chevron-down"} variant="minimal" onClick={() => setRawOpen((open) => !open)}>Raw Diagnostics</Button>
        <Collapse isOpen={rawOpen}><pre className={`${Classes.CODE_BLOCK} raw-diagnostics-output`} aria-label={`${item.label} Raw Diagnostics`}>{JSON.stringify(metadata, null, 2)}</pre></Collapse>
      </section>
    </div>
  );
}

function Overview({ candidates, diagnostics }: { candidates: SolverCandidate[]; diagnostics?: SolverDiagnostics }) {
  const maximumRuntime = Math.max(1, ...candidates.map((candidate) => candidate.elapsed_ms));
  return (
    <div className="benchmark-overview">
      <div>
        <h2 className={Classes.HEADING}>Overview</h2>
        <p className={Classes.TEXT_MUTED}>동일 Job에서 실행된 solver candidate를 비교합니다.</p>
      </div>
      {diagnostics && <dl className="exact-metrics-grid">
        <div><dt>Total Budget</dt><dd>{diagnostics.total_budget_ms.toLocaleString()} ms</dd></div>
        <div><dt>Total Elapsed</dt><dd>{diagnostics.total_elapsed_ms.toLocaleString()} ms</dd></div>
        <div><dt>Baseline Time</dt><dd>{diagnostics.baseline_elapsed_ms.toLocaleString()} ms</dd></div>
        <div><dt>SA Time</dt><dd>{diagnostics.sa_elapsed_ms.toLocaleString()} ms</dd></div>
        <div><dt>SA Runs</dt><dd>{diagnostics.sa_run_count.toLocaleString()}</dd></div>
        <div><dt>Global Best Updates</dt><dd>{diagnostics.global_best_updates.toLocaleString()}</dd></div>
        <div><dt>Termination Reason</dt><dd>{diagnostics.termination_reason}</dd></div>
      </dl>}
      <div className="benchmark-table-scroll">
        <table className={`${Classes.HTML_TABLE} ${Classes.HTML_TABLE_BORDERED} ${Classes.HTML_TABLE_STRIPED}`}>
          <thead><tr><th>Strategy</th><th>Feasible</th><th>Latest Start</th><th>Finish</th><th>Travel</th><th>Wait</th><th>Runtime</th></tr></thead>
          <tbody>
            {candidates.map((candidate) => <tr key={candidate.strategy}>
              <td>{candidate.strategy} {candidate.best && <Tag intent={Intent.PRIMARY} minimal>★ Best</Tag>}</td>
              <td>{candidate.feasible ? "Yes" : "No"}</td>
              <td>{scoreValue(candidate, "latest_start")}</td>
              <td>{scoreValue(candidate, "finish_time")}</td>
              <td>{minuteValue(candidate, "travel_minutes")}</td>
              <td>{minuteValue(candidate, "wait_minutes")}</td>
              <td>{candidate.elapsed_ms} ms</td>
            </tr>)}
          </tbody>
        </table>
      </div>
      <section className="runtime-comparison" aria-labelledby="runtime-comparison-title">
        <h3 id="runtime-comparison-title">Runtime 비교</h3>
        <div className="runtime-bars">
          {candidates.map((candidate) => {
            const width = Math.max(2, (candidate.elapsed_ms / maximumRuntime) * 100);
            return <div className="runtime-row" key={candidate.strategy}>
              <span title={candidate.strategy}>{candidate.strategy}</span>
              <div className="runtime-track">
                <div className={`runtime-bar ${candidate.best ? "is-best" : ""}`} style={{ width: `${width}%` }} role="img" aria-label={`${candidate.strategy} runtime ${candidate.elapsed_ms} ms`} />
              </div>
              <strong>{candidate.elapsed_ms} ms</strong>
            </div>;
          })}
        </div>
      </section>
    </div>
  );
}

function StrategyDetail({ item, candidates }: { item: StrategyNavigationItem; candidates: SolverCandidate[] }) {
  const matches = candidates.filter((candidate) => item.matches(candidate.strategy));
  const candidate = matches.find((value) => value.best) ?? matches[0];
  const [rawOpen, setRawOpen] = useState(false);

  useEffect(() => setRawOpen(false), [item.id, candidate?.strategy]);

  if (!candidate) {
    return <div className="benchmark-strategy-detail"><h2 className={Classes.HEADING}>{item.label}</h2><NonIdealState icon="info-sign" title="실행 결과 없음" description="이 Job에서는 해당 전략이 실행되지 않았거나 적용 대상이 아니었습니다." /></div>;
  }

  return (
    <div className="benchmark-strategy-detail">
      <div className="benchmark-detail-heading">
        <div><h2 className={Classes.HEADING}>{item.label}</h2><code>{candidate.strategy}</code></div>
        {matches.length > 1 && <Tag minimal>{matches.length} runs 중 대표 결과</Tag>}
      </div>
      <CandidateFacts candidate={candidate} />
      <div className="benchmark-route"><span className={Classes.TEXT_MUTED}>Route</span><strong>{candidate.route.length ? candidate.route.join(" → ") : "—"}</strong></div>
      <section className="raw-diagnostics">
        <Button alignText="left" fill icon="code" endIcon={rawOpen ? "chevron-up" : "chevron-down"} variant="minimal" onClick={() => setRawOpen((open) => !open)}>Raw Diagnostics</Button>
        <Collapse isOpen={rawOpen}>
          <pre className={`${Classes.CODE_BLOCK} raw-diagnostics-output`} aria-label={`${item.label} Raw Diagnostics`}>{JSON.stringify(candidate, null, 2)}</pre>
        </Collapse>
      </section>
    </div>
  );
}

export function SolverBenchmarkDialog({ candidates, diagnostics, locationCount, dark, isOpen, onClose }: SolverBenchmarkDialogProps) {
  const [selectedView, setSelectedView] = useState<BenchmarkView>("overview");
  useEffect(() => { if (isOpen) setSelectedView("overview"); }, [isOpen]);
  const selectedItem = useMemo(() => STRATEGY_NAVIGATION.find((item) => item.id === selectedView), [selectedView]);

  return (
    <Dialog className="solver-benchmark-dialog" isOpen={isOpen} onClose={onClose} portalClassName={dark ? Classes.DARK : undefined} title="Solver Benchmark" icon="comparison" canEscapeKeyClose>
      <DialogBody className="solver-benchmark-body">
        <aside className="benchmark-sidebar">
          <nav aria-label="Solver Benchmark 탐색">
            <Button alignText="left" fill active={selectedView === "overview"} aria-current={selectedView === "overview" ? "page" : undefined} icon="dashboard" variant="minimal" onClick={() => setSelectedView("overview")}>Overview</Button>
            <div className="benchmark-nav-divider" />
            {STRATEGY_NAVIGATION.map((item) => {
              const count = candidates.filter((candidate) => item.matches(candidate.strategy)).length;
              return <Button key={item.id} alignText="left" fill active={selectedView === item.id} aria-current={selectedView === item.id ? "page" : undefined} variant="minimal" onClick={() => setSelectedView(item.id)}>{item.label}{count > 0 && <Tag minimal round>{count}</Tag>}</Button>;
            })}
          </nav>
        </aside>
        <main className="benchmark-content">
          {selectedView === "overview" ? <Overview candidates={candidates} diagnostics={diagnostics} /> : selectedView === "exact" ? <ExactBitDpDetail candidates={candidates} locationCount={locationCount} /> : selectedView === "clustered" ? <ClusteredDetail candidates={candidates} /> : selectedView === "mst" ? <MstDoubleTreeDetail candidates={candidates} /> : selectedView === "christofides" ? <ChristofidesDetail candidates={candidates} /> : selectedView.startsWith("sa-") && selectedItem ? <SimulatedAnnealingDetail item={selectedItem} candidates={candidates} /> : selectedItem ? <StrategyDetail item={selectedItem} candidates={candidates} /> : null}
        </main>
      </DialogBody>
    </Dialog>
  );
}
