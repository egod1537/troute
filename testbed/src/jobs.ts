import type { ApiResponse, RouteInput, RouteResponse } from "./api";

export type TestbedJobStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed";

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

/** Browser-session observation model; it is not the server job API contract. */
export interface TestbedJob {
  id: string;
  status: TestbedJobStatus;
  createdAt: number;
  completedAt?: number;
  progress: number;
  stage?: string;
  message?: string;
  error?: string;
  request: RouteInput;
  timeline: TimelineEntry[];
  response?: ApiResponse;
  route?: RouteResponse;
}

export const JOB_STATUS_LABELS: Record<TestbedJobStatus, string> = {
  pending: "대기",
  running: "실행 중",
  completed: "완료",
  failed: "오류",
};
