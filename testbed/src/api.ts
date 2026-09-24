export const API_BASE = (
  import.meta.env.VITE_TROUTE_API_BASE_URL || "/api"
).replace(/\/$/, "");
export const ROUTE_PATH = (
  import.meta.env.VITE_TROUTE_ROUTE_PATH ?? "/optimize"
).trim();

export type TravelMode = "TRANSIT" | "DRIVING" | "WALKING" | "BICYCLING";

export interface RouteInput {
  job_id: string;
  locations: {
    id: string;
    name?: string;
    place_id: string;
    open_time: string;
    close_time: string;
    stay_minutes: number;
  }[];
  start_time: string;
  travel_mode?: TravelMode;
  travel_time_matrix?: number[][];
  debug?: {
    min_job_duration_ms?: number;
  };
}

export interface RouteResponse {
  route: {
    order: number;
    location_id: string;
    arrival_time: string;
    departure_time?: string;
  }[];
  total_travel_minutes: number;
  solver_candidates?: SolverCandidate[];
}

export interface SolverCandidate {
  strategy: string;
  best: boolean;
  route: string[];
  feasible: boolean;
  objective_score?: {
    latest_start: string;
    finish_time: string;
    travel_minutes: number;
    wait_minutes: number;
  };
  elapsed_ms: number;
  metadata: {
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
    accepted_worse_moves?: number;
    infeasible_candidates?: number;
    accepted_infeasible_moves?: number;
    best_feasible?: boolean;
    iteration_count?: number;
    accepted_moves?: number;
    improved_moves?: number;
    seed?: number;
    timed_out: boolean;
    error?: string;
  };
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

export interface StoredJobError {
  code: string;
  message: string;
  detail: string;
}

export interface StoredJobRecord extends StoredJobState {
  request: RouteInput;
  result?: RouteResponse | null;
  error?: StoredJobError | null;
}

export interface StoredJobEvent extends StoredJobState {
  request?: RouteInput;
  result?: RouteResponse | null;
  error?: StoredJobError | null;
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
    input.locations.length < 2 ||
    !time(input.start_time)
  ) {
    throw new Error(
      "요청에는 job_id(1~128자), start와 destination을 포함한 2개 이상의 locations, start_time(HH:MM)이 필요합니다.",
    );
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
  const locationCount = input.locations.length;
  const hasSuppliedMatrix = input.travel_time_matrix !== undefined;
  if (hasSuppliedMatrix) {
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
  const locationIds = new Set<string>();
  for (const [index, location] of input.locations.entries()) {
    if (
      !object(location) ||
      !nonempty(location.id) ||
      (location.name !== undefined && typeof location.name !== "string") ||
      (hasSuppliedMatrix
        ? typeof location.place_id !== "string"
        : !nonempty(location.place_id)) ||
      !isTenMinuteTime(location.open_time) ||
      !isTenMinuteTime(location.close_time) ||
      !isTenMinuteDuration(location.stay_minutes)
    ) {
      throw new Error(
        `locations[${index}]에는 id, ${hasSuppliedMatrix ? "문자열 place_id" : "유효한 place_id"}, 10분 단위 open_time/close_time, 0 이상의 10분 배수 stay_minutes가 필요합니다.`,
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
        (stop.departure_time === undefined || time(stop.departure_time)),
    )
  ) {
    throw new ApiError(
      "경로 응답 형식 오류: route 항목과 total_travel_minutes가 필요합니다.",
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
): Promise<number[][]> {
  const response = await request("/integration/matrix", {
    ...input,
    travel_time_matrix: undefined,
  }, signal);
  if (
    !object(response.body) ||
    !Array.isArray(response.body.travel_time_matrix) ||
    !response.body.travel_time_matrix.every(
      (row) => Array.isArray(row) && row.every(unsigned),
    )
  ) {
    throw new ApiError("매트릭스 응답 형식이 올바르지 않습니다.", response);
  }
  return response.body.travel_time_matrix as number[][];
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
      onEvent(event);
      if (
        event.status === "completed" ||
        event.status === "failed" ||
        event.status === "cancelled"
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
