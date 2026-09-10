export const API_BASE = (
  import.meta.env.VITE_TROUTE_API_BASE_URL || "/api"
).replace(/\/$/, "");
// Intentionally unset: the current Rust binary exposes only GET /health.
export const ROUTE_PATH = (import.meta.env.VITE_TROUTE_ROUTE_PATH || "").trim();

export interface RouteInput {
  locations: {
    id: string;
    place_id: string;
    open_time: string;
    close_time: string;
    stay_minutes: number;
  }[];
  start_location_id: string;
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
  } catch (error) {
    throw new Error(`Invalid JSON: ${(error as Error).message}`);
  }
  if (
    !object(input) ||
    !Array.isArray(input.locations) ||
    !input.locations.length ||
    !nonempty(input.start_location_id) ||
    !time(input.start_time)
  ) {
    throw new Error(
      "Input requires locations, start_location_id, and start_time (HH:MM).",
    );
  }
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
        `locations[${index}] requires id, place_id, HH:MM opening/closing times, and non-negative integer stay_minutes.`,
      );
    }
  }
  // Domain validation and all route calculations belong to the backend.
  return input as unknown as RouteInput;
}

export function parseRoute(response: ApiResponse): RouteResponse {
  const data = response.body;
  if (object(data) && ("error" in data || data.status === "infeasible")) {
    throw new ApiError(
      "The backend reported a solver error or infeasible route. Inspect the raw response.",
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
      "Malformed route response: expected route stops and total_travel_minutes.",
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
      `API connection failed: ${(error as Error).message}. Check the API and proxy configuration.`,
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
        ? "Malformed response: the API did not return JSON."
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
      "Malformed health response: expected HTTP 200 and status ok.",
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
      "No route endpoint is configured. This API currently supports health checks only.",
    );
  const response = await request(ROUTE_PATH, input);
  return { response, route: parseRoute(response) };
}
