export const API_BASE = (
  import.meta.env.VITE_TROUTE_API_BASE_URL || "/api"
).replace(/\/$/, "");
export const ROUTE_PATH = (
  import.meta.env.VITE_TROUTE_ROUTE_PATH ?? "/optimize"
).trim();

export type TravelMode = "TRANSIT" | "DRIVING" | "WALKING" | "BICYCLING";
export type StartPolicy = "FIXED" | "EARLIEST" | "LATEST";
export type VisitPolicy = "ALL_REQUIRED" | "BEST_EFFORT_ON_FAILURE";
export type PortfolioPolicy =
  | "CURRENT"
  | "PARALLEL_ALL"
  | "ROUND_ROBIN"
  | "WEIGHTED_FAIR"
  | "RACING"
  | "UCB1"
  | "THOMPSON_SAMPLING"
  | "INSTANCE_AWARE";
export type RouteProviderName =
  | "google"
  | "kakao-mobility"
  | "kakao-maps"
  | "ekispert"
  | "navitime"
  | "otp";

export interface RouteInput {
  job_id: string;
  locations: {
    id: string;
    name?: string;
    place_id: string;
    latitude?: number;
    longitude?: number;
    open_time: string;
    close_time: string;
    stay_minutes: number;
    required?: boolean;
    priority?: number;
  }[];
  visit_policy?: VisitPolicy;
  start_policy?: StartPolicy;
  start_time?: string;
  travel_date?: string;
  time_zone?: string;
  travel_mode?: TravelMode;
  country_code?: string;
  route_provider?: RouteProviderName;
  use_euclidean_matrix?: boolean;
  travel_time_matrix?: number[][];
  sparse_travel_time_matrix?: (number | null)[][];
  debug?: {
    min_job_duration_ms?: number;
  };
  portfolio_experiment?: {
    policy: PortfolioPolicy;
    total_budget_ms?: number;
    logical_cpu_slots?: number;
    seed?: number;
    solver_ids?: string[];
    detailed_trace?: boolean;
  };
}

export interface RouteResponse {
  route: {
    order: number;
    location_id: string;
    arrival_time: string;
    service_start_time?: string;
    departure_time?: string;
    wait_minutes?: number;
    stay_minutes?: number;
  }[];
  total_travel_minutes: number;
  start_policy?: StartPolicy;
  selected_start_time?: string;
  partial?: boolean;
  visited_count?: number;
  skipped_locations?: {
    location_id: string;
    reason: "NOT_SELECTED_DUE_TO_CONSTRAINTS";
  }[];
  result_quality?: "PROVEN" | "BEST_FOUND";
  selected_provider?: RouteProviderName;
  provider_selection_reason?: string;
  provider_selection_source?: string;
  country_code?: string;
  mode?: TravelMode;
  matrix_departure_time?: string;
  matrix_time_zone?: string;
  time_dependent_revalidation?: {
    attempted: boolean;
    applied: boolean;
    revalidated_leg_count: number;
    changed_leg_count: number;
    max_delta_minutes: number;
    schedule_feasible?: boolean;
    warning_code?: string;
    warning_detail?: string;
  };
  solver_candidates?: SolverCandidate[];
  solver_diagnostics?: SolverDiagnostics;
  solver_telemetry?: PortfolioTelemetrySummary;
}

export interface RouteProviderPolicyDiagnostics {
  routeProviderMode: string;
  overrideEnabled: boolean;
  policy: {
    countries: Record<
      string,
      {
        modes?: Partial<Record<TravelMode, RouteProviderName>>;
        defaultProvider?: RouteProviderName;
      }
    >;
    modeDefaults?: Partial<Record<TravelMode, RouteProviderName>>;
    defaultProvider?: RouteProviderName;
  };
  providers: {
    provider: RouteProviderName;
    available: boolean;
    reason?: string;
    capabilities: TravelMode[];
  }[];
  routes: {
    country?: string;
    mode?: TravelMode;
    provider: RouteProviderName;
    available: boolean;
    reason?: string;
  }[];
}

export interface SolverDiagnostics {
  total_budget_ms: number;
  total_elapsed_ms: number;
  baseline_elapsed_ms: number;
  sa_elapsed_ms: number;
  sa_run_count: number;
  global_best_updates: number;
  termination_reason: string;
}

export interface SolverCandidate {
  strategy: string;
  solver_id?: string;
  run_id?: number;
  best: boolean;
  route: string[];
  feasible: boolean;
  objective_score?: {
    latest_start: string;
    start_time?: string;
    finish_time: string;
    travel_minutes: number;
    wait_minutes: number;
    visited_optional_count?: number;
    visited_optional_priority?: number;
  };
  elapsed_ms: number;
  metadata: {
    job_queued_at_ms?: number;
    job_started_at_ms?: number;
    job_finished_at_ms?: number;
    outcome?: "COMPLETED" | "TIMEOUT" | "CANCELLED" | "UNSUPPORTED" | "FAILED" | "INFEASIBLE";
    proof_status?: "OPTIMAL" | "INFEASIBLE" | "FEASIBLE" | "UNKNOWN";
    objective_phase?: string;
    best_bound?: number;
    relative_gap_ppm?: number;
    thread_count?: number;
    source_version?: string;
    native_status?: string;
    child_process_id?: number;
    exit_code?: number;
    state_count?: number;
    frontier_state_count?: number;
    frontier_cell_count?: number;
    cluster_count?: number;
    cluster_sizes?: number[];
    cluster_strategy?: string;
    cluster_order_strategy?: string;
    cluster_order?: number[];
    cluster_details?: {
      cluster: number;
      members: string[];
      route: string[];
      entry?: string;
      exit?: string;
      state_count: number;
      frontier_state_count: number;
    }[];
    score_before_improvement?: number;
    score_after_improvement?: number;
    improvement_strategy?: string;
    swap_enabled?: boolean;
    relocate_enabled?: boolean;
    two_opt_enabled?: boolean;
    symmetric_distance_strategy?: string;
    mst_cost?: number;
    mst_edge_count?: number;
    mst_edges?: {
      from: string;
      to: string;
      distance: number;
    }[];
    euler_tour?: string[];
    shortcut_route?: string[];
    odd_vertices?: string[];
    odd_vertex_count?: number;
    matching_strategy?: string;
    matching_cost?: number;
    matching_pairs?: {
      left: string;
      right: string;
      distance: number;
    }[];
    initial_strategy?: string;
    initial_route?: string[];
    final_route?: string[];
    initial_score?: number;
    final_score?: number;
    initial_temperature?: string;
    final_temperature?: string;
    cooling_rate?: string;
    swap_move_count?: number;
    relocate_move_count?: number;
    two_opt_move_count?: number;
    insert_optional_move_count?: number;
    remove_optional_move_count?: number;
    replace_optional_move_count?: number;
    accepted_worse_moves?: number;
    infeasible_candidates?: number;
    accepted_infeasible_moves?: number;
    best_feasible?: boolean;
    iteration_count?: number;
    accepted_moves?: number;
    improved_moves?: number;
    seed?: number;
    improved_global_best?: boolean;
    timed_out: boolean;
    error?: string;
    queued_ms?: number;
    reserved_slot_ms?: number;
    cpu_ms?: number;
    peak_rss_bytes?: number;
    memory_hint_bytes?: number;
    scheduler_reason?: string;
    scheduler_score?: string;
  };
}

export interface ObjectiveVector {
  visited_optional_count: number;
  visited_optional_priority: number;
  start_policy: StartPolicy;
  selected_start: number;
  finish: number;
  travel: number;
  wait: number;
}

export type SolverTelemetryEventType =
  | "RUN_QUEUED"
  | "RUN_STARTED"
  | "FIRST_FEASIBLE"
  | "INCUMBENT_IMPROVED"
  | "BOUND_UPDATED"
  | "RUN_FINISHED"
  | "RUN_TIMED_OUT"
  | "RUN_CANCELLED"
  | "RUN_FAILED"
  | "GLOBAL_BEST_UPDATED"
  | "SCHEDULER_DECISION"
  | "RESOURCE_ALLOCATED"
  | "RESOURCE_RELEASED"
  | "ARM_ELIMINATED"
  | "PROGRESS";

export interface SolverTelemetryEvent {
  schema_version: number;
  job_id: string;
  event_sequence: number;
  solver_id?: string;
  run_id?: number;
  scheduler_policy: PortfolioPolicy;
  event_type: SolverTelemetryEventType;
  elapsed_from_job_start_ms: number;
  recorded_at_unix_ms?: number;
  payload: {
    status?: string;
    reason?: string;
    message?: string;
    objective?: ObjectiveVector;
    previous_objective?: ObjectiveVector;
    route_fingerprint?: string;
    source_proof_status?: string;
    wall_ms?: number;
    cpu_ms?: number;
    peak_rss_bytes?: number;
    memory_hint_bytes?: number;
    reserved_slot_ms?: number;
    configured_threads?: number;
    logical_cpu_slots?: number;
    measured?: boolean;
    metric_source?: string;
    phase?: string;
    native_bound?: string;
    relative_gap_ppm?: number;
    reward_ppm?: number;
    selected_arm?: string;
    decision_score?: string;
    budget_remaining_ms?: number;
    state_fingerprint?: string;
    policy_version?: string;
    sample_count?: number;
    estimated_budget_saved_ms?: number;
  };
}

export interface SolverRunSummary {
  solver_id: string;
  run_id: number;
  status: string;
  feasible: boolean;
  selected: boolean;
  wall_ms: number;
  cpu_ms?: number;
  peak_rss_bytes?: number;
  memory_hint_bytes?: number;
  reserved_slot_ms: number;
  configured_threads: number;
  objective?: ObjectiveVector;
  proof_status?: string;
  seed?: number;
}

export interface PortfolioTelemetrySummary {
  schema_version: number;
  policy: PortfolioPolicy;
  policy_version: string;
  total_budget_ms: number;
  logical_cpu_slots: number;
  seed: number;
  instance_rule_id?: string;
  instance_model_version?: string;
  instance_feature_schema_version?: number;
  instance_selected_solver_ids?: string[];
  instance_weight_ppm?: Record<string, number>;
  instance_fallback_reasons?: string[];
  instance_features?: Record<string, string>;
  trace_collected: boolean;
  runs: SolverRunSummary[];
  events: SolverTelemetryEvent[];
}

export interface SolverCapabilities {
  schema_version: number;
  default_policy: PortfolioPolicy;
  experiment_override_enabled: boolean;
  limits: {
    max_total_budget_ms: number;
    max_logical_cpu_slots: number;
    max_trace_events: number;
  };
  solvers: {
    id: string;
    name: string;
    family: string;
    version?: string;
    capabilities: Record<string, boolean>;
    execution_model: string;
    supported_size: { minimum: number; maximum?: number };
    configured_threads: number;
    resource_cost_hint: {
      logical_cpu_slots: number;
      native_threads: number;
      memory_bytes?: number;
    };
    unavailable_reason?: string;
  }[];
  policies: {
    id: PortfolioPolicy;
    name: string;
    version: string;
    preemptive: boolean;
    description: string;
  }[];
}

export interface SolverTelemetryPage {
  schema_version: number;
  job_id: string;
  collected: boolean;
  summary?: PortfolioTelemetrySummary;
  events: SolverTelemetryEvent[];
  next_after: number;
  has_more: boolean;
}

export type StoredJobStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed"
  | "cancelled";

export interface StoredJobState {
  job_id: string;
  status: StoredJobStatus;
  stage?: string | null;
  progress: number;
  last_message?: string | null;
  created_at: number;
  updated_at: number;
  completed_at?: number | null;
}

export interface StoredJobSummary {
  job_id: string;
  status: StoredJobStatus;
  travel_mode?: TravelMode;
  created_at: number;
  updated_at: number;
}

export type FailureSuggestionType =
  | "REDUCE_STAY_TIME"
  | "MOVE_LOCATION_EARLIER"
  | "MOVE_LOCATION_LATER"
  | "START_EARLIER"
  | "START_LATER"
  | "CHANGE_START_POLICY"
  | "CHANGE_START_LOCATION"
  | "CHANGE_END_LOCATION"
  | "REMOVE_LOCATION"
  | "CHANGE_TRAVEL_MODE"
  | "SPLIT_DAY"
  | "RETRY_ROUTING";

export interface FailureSuggestion {
  type: FailureSuggestionType;
  reason: string;
  confidence: "exact" | "heuristic";
  location_id?: string;
  from_location_id?: string;
  to_location_id?: string;
  current_value?: unknown;
  suggested_value?: unknown;
  current_minutes?: number;
  suggested_max_minutes?: number;
  required_shift_minutes?: number;
  suggested_latest_start?: string;
}

export interface FailureDetail {
  type:
    | "INVALID_REQUEST"
    | "ROUTING_UNAVAILABLE"
    | "ROUTING_PAIR_FAILED"
    | "PROVIDER_RESOLUTION"
    | "PROVIDER_NOT_CONFIGURED"
    | "UNSUPPORTED_PROVIDER_CAPABILITY"
    | "NO_FEASIBLE_ROUTE"
    | "START_POLICY_INFEASIBLE"
    | "TIME_WINDOW_VIOLATION"
    | "OUTSIDE_SINGLE_DAY"
    | "CANCELLED"
    | "INTERNAL";
  location_id?: string;
  arrival_time?: string;
  service_start_time?: string;
  required_departure?: string;
  close_time?: string;
  stay_minutes?: number;
  from_location_id?: string;
  to_location_id?: string;
  start_policy?: StartPolicy;
  selected_start_time?: string;
}

export interface StoredJobError {
  code: string;
  message: string;
  detail: string;
  failure_detail?: FailureDetail;
  suggestions?: FailureSuggestion[];
}

export interface StoredJobRecord extends StoredJobState {
  request: RouteInput;
  result?: RouteResponse | null;
  error?: StoredJobError | null;
}

export interface JobClientState {
  job_id: string;
  status: StoredJobStatus;
  stage: string | null;
  progress: number;
  last_message: string | null;
  result: RouteResponse | null;
  error: StoredJobError | null;
}

export interface StoredJobEvent {
  sequence: number;
  updated_at: number;
  state: JobClientState;
}

export interface StoredTimelineEntry {
  id: string;
  pair_id: string;
  timestamp_ms: number;
  direction: "REQUEST" | "RESPONSE";
  source: "testbed" | "troute";
  target: "testbed" | "troute";
  method?: string | null;
  path?: string | null;
  status?: number | null;
  latency_ms?: number | null;
  headers?: Record<string, string> | null;
  query?: unknown;
  body?: unknown;
  raw?: string | null;
  error?: string | null;
}

export interface ApiResponse {
  method: string;
  path: string;
  status: number;
  body: unknown;
  raw: string;
  request_latency_ms: number;
}

export class ApiError extends Error {
  constructor(
    message: string,
    public response?: ApiResponse,
  ) {
    super(message);
  }
}

const object = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const nonempty = (value: unknown): value is string =>
  typeof value === "string" && value.trim().length > 0;
const time = (value: unknown): value is string =>
  typeof value === "string" && /^(?:[01]\d|2[0-3]):[0-5]\d$/.test(value);
export const isTenMinuteTime = (value: unknown): value is string =>
  time(value) && Number(value.slice(3)) % 10 === 0;
const unsigned = (value: unknown): value is number =>
  typeof value === "number" &&
  Number.isInteger(value) &&
  value >= 0 &&
  value <= 4294967295;
export const isTenMinuteDuration = (value: unknown): value is number =>
  unsigned(value) && value % 10 === 0;
const isTravelMode = (value: unknown): value is TravelMode =>
  value === "TRANSIT" ||
  value === "DRIVING" ||
  value === "WALKING" ||
  value === "BICYCLING";
const isRouteProviderName = (value: unknown): value is RouteProviderName =>
  value === "google" ||
  value === "kakao-mobility" ||
  value === "kakao-maps" ||
  value === "ekispert" ||
  value === "navitime" ||
  value === "otp";
const isStartPolicy = (value: unknown): value is StartPolicy =>
  value === "FIXED" || value === "EARLIEST" || value === "LATEST";

export function parseInput(text: string): RouteInput {
  let input: unknown;
  try {
    input = JSON.parse(text);
  } catch {
    throw new Error("JSON 형식 오류: 올바른 JSON인지 확인하세요.");
  }
  if (
    !object(input) ||
    !nonempty(input.job_id) ||
    [...input.job_id].length > 128 ||
    !Array.isArray(input.locations) ||
    input.locations.length < 2
  ) {
    throw new Error(
      "요청에는 job_id(1~128자)와 start/destination을 포함한 2개 이상의 locations가 필요합니다.",
    );
  }
  if (input.start_policy !== undefined && !isStartPolicy(input.start_policy)) {
    throw new Error("start_policy는 FIXED, EARLIEST, LATEST 중 하나여야 합니다.");
  }
  if (input.start_time !== undefined && !time(input.start_time)) {
    throw new Error("start_time은 HH:MM 형식이어야 합니다.");
  }
  if (input.start_policy === "FIXED" && !time(input.start_time)) {
    throw new Error("FIXED start_policy에는 start_time(HH:MM)이 필요합니다.");
  }
  if (
    input.debug !== undefined &&
    (!object(input.debug) ||
      (input.debug.min_job_duration_ms !== undefined &&
        (!unsigned(input.debug.min_job_duration_ms) ||
          input.debug.min_job_duration_ms > 60_000)))
  ) {
    throw new Error(
      "debug.min_job_duration_ms는 0~60000 범위의 정수여야 합니다.",
    );
  }
  if (input.travel_mode !== undefined && !isTravelMode(input.travel_mode)) {
    throw new Error(
      "travel_mode는 TRANSIT, DRIVING, WALKING, BICYCLING 중 하나여야 합니다.",
    );
  }
  if (
    input.country_code !== undefined &&
    (typeof input.country_code !== "string" ||
      !/^[A-Za-z]{2}$/.test(input.country_code.trim()))
  ) {
    throw new Error("country_code는 ISO 3166-1 alpha-2 형식이어야 합니다.");
  }
  if (
    input.route_provider !== undefined &&
    !isRouteProviderName(input.route_provider)
  ) {
    throw new Error(
      "route_provider는 google, kakao-mobility, kakao-maps, ekispert, navitime, otp 중 하나여야 합니다.",
    );
  }
  const locationCount = input.locations.length;
  if (
    input.travel_time_matrix !== undefined &&
    input.sparse_travel_time_matrix !== undefined
  ) {
    throw new Error("travel_time_matrix와 sparse_travel_time_matrix는 동시에 보낼 수 없습니다.");
  }
  const hasSuppliedMatrix =
    input.travel_time_matrix !== undefined ||
    input.sparse_travel_time_matrix !== undefined;
  const useEuclideanMatrix =
    input.use_euclidean_matrix === true || !hasSuppliedMatrix;
  if (input.use_euclidean_matrix !== undefined && typeof input.use_euclidean_matrix !== "boolean") {
    throw new Error("use_euclidean_matrix는 boolean이어야 합니다.");
  }
  if (useEuclideanMatrix && hasSuppliedMatrix) {
    throw new Error("use_euclidean_matrix는 전달된 매트릭스와 함께 사용할 수 없습니다.");
  }
  if (input.travel_time_matrix !== undefined) {
    if (
      !Array.isArray(input.travel_time_matrix) ||
      input.travel_time_matrix.length !== locationCount
    ) {
      throw new Error(
        `travel_time_matrix는 locations와 같은 ${locationCount}x${locationCount} 크기여야 합니다.`,
      );
    }
    input.travel_time_matrix.forEach((row, rowIndex) => {
      if (
        !Array.isArray(row) ||
        row.length !== locationCount ||
        !row.every(unsigned)
      ) {
        throw new Error(
          `travel_time_matrix[${rowIndex}]는 0 이상의 정수 ${locationCount}개를 포함해야 합니다.`,
        );
      }
      if (row[rowIndex] !== 0) {
        throw new Error(
          `travel_time_matrix[${rowIndex}][${rowIndex}]는 0이어야 합니다.`,
        );
      }
    });
  }
  if (input.sparse_travel_time_matrix !== undefined) {
    const matrix = input.sparse_travel_time_matrix;
    if (!Array.isArray(matrix) || matrix.length !== locationCount) {
      throw new Error(
        `sparse_travel_time_matrix는 locations와 같은 ${locationCount}x${locationCount} 크기여야 합니다.`,
      );
    }
    matrix.forEach((row, rowIndex) => {
      if (
        !Array.isArray(row) ||
        row.length !== locationCount ||
        !row.every((value) => value === null || unsigned(value))
      ) {
        throw new Error(
          `sparse_travel_time_matrix[${rowIndex}]는 0 이상의 정수 또는 null ${locationCount}개를 포함해야 합니다.`,
        );
      }
      if (row[rowIndex] !== 0) {
        throw new Error(
          `sparse_travel_time_matrix[${rowIndex}][${rowIndex}]는 0이어야 합니다.`,
        );
      }
    });
  }
  const locationIds = new Set<string>();
  for (const [index, location] of input.locations.entries()) {
    if (object(location) && !isTenMinuteTime(location.open_time)) {
      throw new Error(`locations[${index}].open_time은 10분 단위 HH:MM이어야 합니다.`);
    }
    if (object(location) && !isTenMinuteTime(location.close_time)) {
      throw new Error(`locations[${index}].close_time은 10분 단위 HH:MM이어야 합니다.`);
    }
    if (object(location) && !isTenMinuteDuration(location.stay_minutes)) {
      throw new Error(`locations[${index}].stay_minutes는 0 이상의 10분 배수여야 합니다.`);
    }
    if (
      !object(location) ||
      !nonempty(location.id) ||
      (location.name !== undefined && typeof location.name !== "string") ||
      (hasSuppliedMatrix || useEuclideanMatrix
        ? typeof location.place_id !== "string"
        : !nonempty(location.place_id)) ||
      ((location.latitude === undefined) !== (location.longitude === undefined)) ||
      (location.latitude !== undefined &&
        (typeof location.latitude !== "number" ||
          !Number.isFinite(location.latitude) ||
          location.latitude < -90 ||
          location.latitude > 90)) ||
      (location.longitude !== undefined &&
        (typeof location.longitude !== "number" ||
          !Number.isFinite(location.longitude) ||
          location.longitude < -180 ||
          location.longitude > 180)) ||
      (useEuclideanMatrix &&
        (location.latitude === undefined || location.longitude === undefined)) ||
      typeof location.open_time !== "string" ||
      typeof location.close_time !== "string" ||
      typeof location.stay_minutes !== "number"
    ) {
      throw new Error(
        `locations[${index}]의 장소 정보, 좌표, 시간 또는 체류 시간이 올바르지 않습니다.`,
      );
    }
    if (locationIds.has(location.id)) {
      throw new Error(`locations의 id는 고유해야 합니다: ${location.id}`);
    }
    locationIds.add(location.id);
  }
  // Domain validation and all route calculations belong to the backend.
  return input as unknown as RouteInput;
}

export function parseRoute(response: ApiResponse): RouteResponse {
  const data = response.body;
  if (object(data) && ("error" in data || data.status === "infeasible")) {
    throw new ApiError(
      "backend가 solver 오류 또는 실행 불가능한 경로를 반환했습니다. Raw response를 확인하세요.",
      response,
    );
  }
  if (
    !object(data) ||
    !Array.isArray(data.route) ||
    !data.route.length ||
    !unsigned(data.total_travel_minutes) ||
    !data.route.every(
      (stop) =>
        object(stop) &&
        unsigned(stop.order) &&
        nonempty(stop.location_id) &&
        time(stop.arrival_time) &&
        (stop.service_start_time === undefined || time(stop.service_start_time)) &&
        (stop.departure_time === undefined || time(stop.departure_time)) &&
        (stop.wait_minutes === undefined || unsigned(stop.wait_minutes)) &&
        (stop.stay_minutes === undefined || unsigned(stop.stay_minutes)),
    )
  ) {
    throw new ApiError(
      "경로 응답 형식 오류: route 항목과 total_travel_minutes가 필요합니다.",
      response,
    );
  }
  if (
    (data.start_policy !== undefined && !isStartPolicy(data.start_policy)) ||
    (data.selected_start_time !== undefined && !time(data.selected_start_time))
  ) {
    throw new ApiError(
      "경로 응답 형식 오류: start policy 진단값이 올바르지 않습니다.",
      response,
    );
  }
  if (
    data.solver_candidates !== undefined &&
    (!Array.isArray(data.solver_candidates) ||
      !data.solver_candidates.every(
        (candidate) =>
          object(candidate) &&
          nonempty(candidate.strategy) &&
          typeof candidate.best === "boolean" &&
          Array.isArray(candidate.route) &&
          candidate.route.every(nonempty) &&
          typeof candidate.feasible === "boolean" &&
          unsigned(candidate.elapsed_ms) &&
          object(candidate.metadata),
      ))
  ) {
    throw new ApiError(
      "경로 응답 형식 오류: solver_candidates 항목이 올바르지 않습니다.",
      response,
    );
  }
  if (
    data.solver_diagnostics !== undefined &&
    (!object(data.solver_diagnostics) ||
      !unsigned(data.solver_diagnostics.total_budget_ms) ||
      !unsigned(data.solver_diagnostics.total_elapsed_ms) ||
      !unsigned(data.solver_diagnostics.baseline_elapsed_ms) ||
      !unsigned(data.solver_diagnostics.sa_elapsed_ms) ||
      !unsigned(data.solver_diagnostics.sa_run_count) ||
      !unsigned(data.solver_diagnostics.global_best_updates) ||
      !nonempty(data.solver_diagnostics.termination_reason))
  ) {
    throw new ApiError(
      "경로 응답 형식 오류: solver_diagnostics 항목이 올바르지 않습니다.",
      response,
    );
  }
  return data as unknown as RouteResponse;
}

async function request(
  path: string,
  payload?: RouteInput,
  signal?: AbortSignal,
): Promise<ApiResponse> {
  const method = payload ? "POST" : "GET";
  const started = performance.now();
  let response: Response;
  let raw: string;
  try {
    response = await fetch(`${API_BASE}/${path.replace(/^\//, "")}`, {
      method,
      headers: payload ? { "Content-Type": "application/json" } : undefined,
      body: payload ? JSON.stringify(payload) : undefined,
      signal: signal ?? AbortSignal.timeout(30_000),
      cache: "no-store",
    });
    raw = await response.text();
  } catch (error) {
    throw new ApiError(
      `API 연결 실패: ${(error as Error).message}. API와 proxy 설정을 확인하세요.`,
    );
  }
  const result: ApiResponse = {
    method,
    path,
    status: response.status,
    raw,
    body: null,
    request_latency_ms: performance.now() - started,
  };
  try {
    result.body = JSON.parse(raw);
  } catch {
    throw new ApiError(
      response.ok
        ? "응답 형식 오류: API가 JSON을 반환하지 않았습니다."
        : `HTTP ${response.status} ${response.statusText}`,
      result,
    );
  }
  if (!response.ok)
    throw new ApiError(
      `HTTP ${response.status} ${response.statusText}`,
      result,
    );
  return result;
}

export async function checkHealth(): Promise<ApiResponse> {
  const response = await request("/health");
  if (
    response.status !== 200 ||
    !object(response.body) ||
    response.body.status !== "ok"
  ) {
    throw new ApiError(
      "Health 응답 형식 오류: HTTP 200과 status ok가 필요합니다.",
      response,
    );
  }
  return response;
}

export function getRouteProviderPolicy(
  signal?: AbortSignal,
): Promise<RouteProviderPolicyDiagnostics> {
  return integrationJson("/route/providers/policy", signal);
}

export function getSolverCapabilities(
  signal?: AbortSignal,
): Promise<SolverCapabilities> {
  return integrationJson("/integration/solver-capabilities", signal);
}

export function getSolverTelemetry(
  jobId: string,
  after = 0,
  limit = 500,
  signal?: AbortSignal,
): Promise<SolverTelemetryPage> {
  return integrationJson(
    `/integration/jobs/${encodeURIComponent(jobId)}/solver-telemetry?after=${after}&limit=${limit}`,
    signal,
  );
}

export function subscribeToSolverEvents(
  jobId: string,
  after: number,
  onEvent: (event: SolverTelemetryEvent) => void,
  onDisconnect: () => void,
): () => void {
  const source = new EventSource(
    `${API_BASE}/integration/jobs/${encodeURIComponent(jobId)}/solver-events?after=${after}`,
  );
  source.addEventListener("solver_event", ((message: MessageEvent<string>) => {
    try {
      const event = JSON.parse(message.data) as SolverTelemetryEvent;
      if (event.event_sequence === Number(message.lastEventId)) onEvent(event);
    } catch {
      // Cursor GET remains the recovery path for malformed notifications.
    }
  }) as EventListener);
  source.onerror = onDisconnect;
  return () => source.close();
}

export async function runRoute(
  input: RouteInput,
): Promise<{ response: ApiResponse; route: RouteResponse }> {
  if (!ROUTE_PATH)
    throw new Error(
      "경로 endpoint가 설정되지 않았습니다. 현재 API는 health check만 지원합니다.",
    );
  const response = await request(ROUTE_PATH, input);
  return { response, route: parseRoute(response) };
}

export async function fetchTravelTimeMatrix(
  input: RouteInput,
  signal?: AbortSignal,
): Promise<(number | null)[][]> {
  const response = await request("/integration/matrix", {
    ...input,
    use_euclidean_matrix: true,
    travel_time_matrix: undefined,
    sparse_travel_time_matrix: undefined,
  }, signal);
  if (
    !object(response.body) ||
    !Array.isArray(response.body.travel_time_matrix) ||
    !response.body.travel_time_matrix.every(
      (row) =>
        Array.isArray(row) &&
        row.every((value) => value === null || unsigned(value)),
    )
  ) {
    throw new ApiError("매트릭스 응답 형식이 올바르지 않습니다.", response);
  }
  return response.body.travel_time_matrix as (number | null)[][];
}

async function integrationJson<T>(path: string, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`${API_BASE}/${path.replace(/^\//, "")}`, {
    cache: "no-store",
    signal,
  });
  if (!response.ok) throw new Error(`HTTP ${response.status} ${response.statusText}`);
  return (await response.json()) as T;
}

export async function listRecentJobs(
  limit = 50,
  signal?: AbortSignal,
): Promise<StoredJobSummary[]> {
  const response = await integrationJson<{ jobs: StoredJobSummary[] }>(
    `/integration/jobs?limit=${limit}`,
    signal,
  );
  return response.jobs;
}

export function getStoredJob(
  jobId: string,
  signal?: AbortSignal,
): Promise<StoredJobRecord> {
  return integrationJson(
    `/integration/jobs/${encodeURIComponent(jobId)}`,
    signal,
  );
}

export function subscribeToStoredJob(
  jobId: string,
  onEvent: (event: StoredJobEvent) => void,
  onDisconnect: () => void,
): () => void {
  const source = new EventSource(
    `${API_BASE}/integration/jobs/${encodeURIComponent(jobId)}/events`,
  );
  const receive = (message: MessageEvent<string>) => {
    try {
      const event = JSON.parse(message.data) as StoredJobEvent;
      if (
        !object(event) ||
        !Number.isSafeInteger(event.sequence) ||
        event.sequence < 0 ||
        message.lastEventId !== String(event.sequence) ||
        !object(event.state)
      ) {
        return;
      }
      onEvent(event);
      if (
        event.state.status === "completed" ||
        event.state.status === "failed" ||
        event.state.status === "cancelled"
      ) {
        source.close();
      }
    } catch {
      // A malformed notification is ignored; GET remains the recovery path.
    }
  };
  for (const event of [
    "snapshot",
    "progress",
    "completed",
    "failed",
    "cancelled",
  ]) {
    source.addEventListener(event, receive as EventListener);
  }
  source.onerror = onDisconnect;
  return () => source.close();
}

export async function getStoredTimeline(
  jobId: string,
  signal?: AbortSignal,
): Promise<StoredTimelineEntry[]> {
  const response = await integrationJson<{
    job_id: string;
    entries: StoredTimelineEntry[];
  }>(`/integration/jobs/${encodeURIComponent(jobId)}/timeline`, signal);
  return response.entries;
}

export async function cancelStoredJob(
  jobId: string,
): Promise<{ job_id: string; status: "cancelled" }> {
  const response = await fetch(
    `${API_BASE}/integration/jobs/${encodeURIComponent(jobId)}/cancel`,
    { method: "POST", cache: "no-store" },
  );
  const body = (await response.json()) as {
    job_id?: string;
    status?: "cancelled";
    error?: { message?: string; detail?: string };
  };
  if (!response.ok) {
    const detail = body.error?.detail ? ` (${body.error.detail})` : "";
    throw new Error(
      body.error?.message
        ? `${body.error.message}${detail}`
        : `HTTP ${response.status} ${response.statusText}`,
    );
  }
  if (body.job_id !== jobId || body.status !== "cancelled") {
    throw new Error("Job 취소 응답 형식이 올바르지 않습니다.");
  }
  return { job_id: body.job_id, status: body.status };
}
