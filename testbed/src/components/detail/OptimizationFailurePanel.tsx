import { Callout, Card, Classes, Intent } from "@blueprintjs/core";
import type {
  FailureDetail,
  FailureSuggestion,
  RouteInput,
  StoredJobError,
} from "../../api";

const MAX_VISIBLE_SUGGESTIONS = 3;

interface SuggestionPresentation {
  title: string;
  describe: (
    suggestion: FailureSuggestion,
    locationName: string,
  ) => string;
}

const SUGGESTION_PRESENTATIONS: Record<string, SuggestionPresentation> = {
  REDUCE_STAY_TIME: {
    title: "체류시간 줄이기",
    describe: (_suggestion, locationName) =>
      `${locationName}의 체류시간을 영업 종료 전에 마칠 수 있는 범위로 줄여 보세요.`,
  },
  START_EARLIER: {
    title: "더 일찍 출발",
    describe: (suggestion) =>
      suggestion.required_shift_minutes !== undefined
        ? `현재보다 최소 ${suggestion.required_shift_minutes}분 일찍 출발하면 시간 제약을 줄일 수 있습니다.`
        : "출발 시간을 앞당겨 시간 제약을 줄여 보세요.",
  },
  MOVE_LOCATION_EARLIER: {
    title: "방문 순서 앞당기기",
    describe: (suggestion, locationName) =>
      suggestion.required_shift_minutes !== undefined
        ? `${locationName} 방문을 최소 ${suggestion.required_shift_minutes}분 앞당겨 보세요.`
        : `${locationName}을 더 이른 순서에 방문해 보세요.`,
  },
  CHANGE_TRAVEL_MODE: {
    title: "이동수단 변경",
    describe: (suggestion) =>
      `${formatTravelMode(suggestion.suggested_value)}으로 경로를 다시 계산할 수 있습니다.`,
  },
  REMOVE_LOCATION: {
    title: "장소 제외",
    describe: (_suggestion, locationName) =>
      `${locationName}을 일정에서 제외했을 때 가능한 경로가 확인되었습니다.`,
  },
  SPLIT_DAY: {
    title: "일정 나누기",
    describe: () =>
      "방문 장소를 여러 날로 나누면 하루 일정의 시간 제약을 줄일 수 있습니다.",
  },
};

interface OptimizationFailurePanelProps {
  failure?: StoredJobError;
  fallbackError?: string;
  request: RouteInput;
}

export function OptimizationFailurePanel({
  failure,
  fallbackError,
  request,
}: OptimizationFailurePanelProps) {
  const structuredFailure = isStoredJobError(failure) ? failure : undefined;
  const cause = describeFailure(
    structuredFailure?.failure_detail,
    structuredFailure,
    fallbackError,
    request,
  );
  const suggestions = Array.isArray(structuredFailure?.suggestions)
    ? structuredFailure.suggestions.slice(0, MAX_VISIBLE_SUGGESTIONS)
    : [];

  if (!cause && suggestions.length === 0) return null;

  return (
    <Callout
      className="optimization-failure"
      icon="error"
      intent={Intent.DANGER}
      role="alert"
      title="최적화할 수 없습니다"
    >
      {cause && (
        <section className="failure-cause" aria-labelledby="failure-cause-title">
          <h2 id="failure-cause-title">원인</h2>
          <p>{cause}</p>
        </section>
      )}

      {suggestions.length > 0 && (
        <section
          className="failure-recommendations"
          aria-labelledby="failure-recommendations-title"
        >
          <h2 id="failure-recommendations-title">추천</h2>
          <ol className="failure-suggestion-list">
            {suggestions.map((suggestion, index) => (
              <SuggestionCard
                key={`${String(suggestion.type)}-${suggestion.location_id ?? index}`}
                suggestion={suggestion}
                request={request}
              />
            ))}
          </ol>
        </section>
      )}
    </Callout>
  );
}

function SuggestionCard({
  suggestion,
  request,
}: {
  suggestion: FailureSuggestion;
  request: RouteInput;
}) {
  const presentation = SUGGESTION_PRESENTATIONS[String(suggestion.type)];
  const locationName = findLocationName(request, suggestion.location_id);

  // Unknown suggestion types deliberately expose only the server-provided
  // reason. This remains useful when the contract grows without implying that
  // this client knows how to apply a new action.
  if (!presentation) {
    return (
      <li>
        <Card className="failure-suggestion-card" compact elevation={0}>
          <p className="failure-suggestion-reason">{suggestion.reason}</p>
        </Card>
      </li>
    );
  }

  const change = describeChange(suggestion, request, locationName);
  return (
    <li>
      <Card className="failure-suggestion-card" compact elevation={0}>
        <h3 className={Classes.HEADING}>{presentation.title}</h3>
        <p className="failure-suggestion-reason">
          {presentation.describe(suggestion, locationName)}
        </p>
        {change && (
          <div className="failure-suggestion-change" aria-label="추천 변경값">
            <span>{change.before}</span>
            <span aria-hidden="true">→</span>
            <strong>{change.after}</strong>
          </div>
        )}
      </Card>
    </li>
  );
}

function describeFailure(
  detail: FailureDetail | undefined,
  failure: StoredJobError | undefined,
  fallbackError: string | undefined,
  request: RouteInput,
): string {
  if (detail) {
    switch (detail.type) {
      case "TIME_WINDOW_VIOLATION":
        return detail.close_time
          ? `영업 종료 시각 ${detail.close_time} 전까지 ${findLocationName(request, detail.location_id)} 방문을 마칠 수 없습니다.`
          : `${findLocationName(request, detail.location_id)}의 영업시간을 지킬 수 없습니다.`;
      case "OUTSIDE_SINGLE_DAY":
        return "모든 장소를 하루 안에 방문할 수 없습니다.";
      case "NO_FEASIBLE_ROUTE":
        return "입력한 영업시간과 체류시간을 모두 만족하는 일정을 찾지 못했습니다.";
      case "ROUTING_UNAVAILABLE":
        return "장소 사이의 이동 경로 정보를 불러오지 못했습니다.";
      case "ROUTING_PAIR_FAILED":
        return `${findLocationName(request, detail.from_location_id)}에서 ${findLocationName(request, detail.to_location_id)}(으)로 이동하는 경로를 찾지 못했습니다.`;
      case "START_POLICY_INFEASIBLE":
        return "현재 출발 정책과 출발 시각으로는 영업시간을 만족할 수 없습니다.";
      case "PROVIDER_RESOLUTION":
        return "요청 조건에 맞는 경로 제공자를 선택할 수 없습니다.";
      case "PROVIDER_NOT_CONFIGURED":
        return "선택된 경로 제공자가 현재 설정되어 있지 않습니다.";
      case "UNSUPPORTED_PROVIDER_CAPABILITY":
        return "선택된 경로 제공자가 이 이동수단을 지원하지 않습니다.";
      case "INVALID_REQUEST":
        return "입력한 일정 조건을 확인해 주세요.";
      case "CANCELLED":
        return "경로 최적화가 취소되었습니다.";
      case "INTERNAL":
        return "경로 최적화 중 예상하지 못한 문제가 발생했습니다.";
    }
  }

  return failure?.message || failure?.detail || fallbackError || "";
}

function describeChange(
  suggestion: FailureSuggestion,
  request: RouteInput,
  locationName: string,
): { before: string; after: string } | null {
  switch (String(suggestion.type)) {
    case "REDUCE_STAY_TIME":
      return change(
        minutes(suggestion.current_minutes ?? suggestion.current_value),
        minutes(suggestion.suggested_max_minutes ?? suggestion.suggested_value),
      );
    case "START_EARLIER":
      return change(
        formatValue(suggestion.current_value) || request.start_time || "현재 출발 시간",
        suggestion.suggested_latest_start ||
          formatValue(suggestion.suggested_value) ||
          (suggestion.required_shift_minutes !== undefined
            ? `${suggestion.required_shift_minutes}분 앞당기기`
            : "더 이른 시간"),
      );
    case "MOVE_LOCATION_EARLIER":
      return change(
        formatValue(suggestion.current_value) || "현재 방문 순서",
        formatValue(suggestion.suggested_value) ||
          (suggestion.required_shift_minutes !== undefined
            ? `${suggestion.required_shift_minutes}분 이상 앞당기기`
            : "더 이른 방문 순서"),
      );
    case "CHANGE_TRAVEL_MODE":
      return change(
        formatTravelMode(suggestion.current_value || request.travel_mode),
        formatTravelMode(suggestion.suggested_value),
      );
    case "REMOVE_LOCATION":
      return change(`${locationName} 포함`, `${locationName} 제외`);
    case "SPLIT_DAY":
      return change(
        formatValue(suggestion.current_value) || "하루 일정",
        formatValue(suggestion.suggested_value) || "여러 날 일정",
      );
    default:
      return null;
  }
}

function findLocationName(request: RouteInput, locationId?: string): string {
  if (!locationId) return "해당 장소";
  const location = request.locations.find(({ id }) => id === locationId);
  return location?.name?.trim() || locationId;
}

function minutes(value: unknown): string {
  return typeof value === "number" ? `${value}분` : formatValue(value);
}

function formatTravelMode(value: unknown): string {
  const labels: Record<string, string> = {
    TRANSIT: "대중교통",
    DRIVING: "자동차",
    WALKING: "도보",
    BICYCLING: "자전거",
  };
  const formatted = formatValue(value);
  return labels[formatted] || formatted || "다른 이동수단";
}

function formatValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  if (value === null || value === undefined) return "";
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function change(before: string, after: string) {
  return before && after ? { before, after } : null;
}

function isStoredJobError(value: unknown): value is StoredJobError {
  return typeof value === "object" && value !== null;
}
