export const API_BASE = (
  import.meta.env.VITE_TROUTE_API_BASE_URL || "/api"
).replace(/\/$/, "");
// Intentionally unset: the current Rust binary exposes only GET /health.
export const ROUTE_PATH = (import.meta.env.VITE_TROUTE_ROUTE_PATH || "").trim();

export interface RouteInput {
  job_id: string;
  locations: {
    id: string;
    place_id: string;
    open_time: string;
    close_time: string;
    stay_minutes: number;
  }[];
  start_time: string;
}

export interface RouteResponse {
  route: {
    order: number;
    location_id: string;
    arrival_time: string;
    departure_time?: string;
  }[];
  total_travel_minutes: number;
}

export type StoredJobStatus = "pending" | "running" | "completed" | "failed";

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
  created_at: number;
  updated_at: number;
}

export interface StoredJobError {
  code: string;
  message: string;
  detail: string;
}

export interface StoredJobRecord {
  request: RouteInput;
  state: StoredJobState;
  result?: RouteResponse | null;
  error?: StoredJobError | null;
}

export interface StoredTimelineEntry {
  id: string;
  pair_id: string;
  timestamp_ms: number;
  direction: "REQUEST" | "RESPONSE";
  source: "testbed" | "troute" | "trasolve";
  target: "testbed" | "troute" | "trasolve";
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
const unsigned = (value: unknown): value is number =>
  typeof value === "number" &&
  Number.isInteger(value) &&
  value >= 0 &&
  value <= 4294967295;

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
  const locationIds = new Set<string>();
  for (const [index, location] of input.locations.entries()) {
    if (
      !object(location) ||
      !nonempty(location.id) ||
      !nonempty(location.place_id) ||
      !time(location.open_time) ||
      !time(location.close_time) ||
      !unsigned(location.stay_minutes)
    ) {
      throw new Error(
        `locations[${index}]에는 id, place_id, HH:MM 형식의 open_time/close_time, 0 이상의 정수 stay_minutes가 필요합니다.`,
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
  return data as unknown as RouteResponse;
}

async function request(
  path: string,
  payload?: RouteInput,
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
      signal: AbortSignal.timeout(30_000),
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

async function integrationJson<T>(path: string): Promise<T> {
  const response = await fetch(`${API_BASE}/${path.replace(/^\//, "")}`, {
    cache: "no-store",
  });
  if (!response.ok) throw new Error(`HTTP ${response.status} ${response.statusText}`);
  return (await response.json()) as T;
}

export async function listRecentJobs(limit = 50): Promise<StoredJobSummary[]> {
  const response = await integrationJson<{ jobs: StoredJobSummary[] }>(
    `/integration/jobs?limit=${limit}`,
  );
  return response.jobs;
}

export function getStoredJob(jobId: string): Promise<StoredJobRecord> {
  return integrationJson(`/integration/jobs/${encodeURIComponent(jobId)}`);
}

export async function getStoredTimeline(
  jobId: string,
): Promise<StoredTimelineEntry[]> {
  const response = await integrationJson<{
    job_id: string;
    entries: StoredTimelineEntry[];
  }>(`/integration/jobs/${encodeURIComponent(jobId)}/timeline`);
  return response.entries;
}
