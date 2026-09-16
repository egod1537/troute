import type {
  ApiResponse,
  RouteInput,
  RouteResponse,
  StoredJobRecord,
} from "./api";

export type TestbedJobStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed"
  | "cancelled";

export interface TimelineEntry {
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

/** Testbed view model hydrated from server records and local request lifecycle. */
export interface TestbedJob {
  id: string;
  status: TestbedJobStatus;
  createdAt: number;
  updatedAt: number;
  completedAt?: number;
  progress: number;
  stage?: string;
  message?: string;
  error?: string;
  request: RouteInput;
  timeline: TimelineEntry[];
  response?: ApiResponse;
  route?: RouteResponse;
  /** Last persisted version merged from the server; absent for browser-only jobs. */
  serverUpdatedAt?: number;
}

export const JOB_STATUS_LABELS: Record<TestbedJobStatus, string> = {
  pending: "대기",
  running: "실행 중",
  completed: "완료",
  failed: "오류",
  cancelled: "취소됨",
};

export function mergeStoredJob(
  record: StoredJobRecord,
  existing?: TestbedJob,
): TestbedJob {
  const error = record.error
    ? `${record.error.code}: ${record.error.message}${record.error.detail ? ` (${record.error.detail})` : ""}`
    : undefined;
  return {
    id: record.state.job_id,
    status: record.state.status,
    createdAt: record.state.created_at,
    updatedAt: record.state.updated_at,
    completedAt: record.state.completed_at ?? undefined,
    progress: record.state.progress,
    stage: record.state.stage ?? undefined,
    message: record.state.last_message ?? undefined,
    error,
    request: record.request,
    timeline: existing?.timeline ?? [],
    response: existing?.response,
    route: record.result ?? existing?.route,
    serverUpdatedAt: record.state.updated_at,
  };
}

export function sortJobsNewestFirst(jobs: TestbedJob[]): TestbedJob[] {
  return [...jobs].sort(
    (left, right) =>
      right.createdAt - left.createdAt || left.id.localeCompare(right.id),
  );
}
