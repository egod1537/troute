import type { RouteInput, RouteResponse } from "../../api";
import { SEOUL_LOCATION_COORDINATES } from "../../placePresets";

interface RouteVisualizationProps {
  request: RouteInput;
  result: RouteResponse;
}

interface MapPoint {
  id: string;
  name: string;
  order: number;
  x: number;
  y: number;
}

interface MapBounds {
  minimumLatitude: number;
  minimumLongitude: number;
  latitudeRange: number;
  longitudeRange: number;
}

const MAP_WIDTH = 620;
const MAP_HEIGHT = 340;
const MAP_PADDING = 42;

function timeToMinutes(value?: string): number | undefined {
  if (!value || !/^\d{2}:\d{2}$/.test(value)) return undefined;
  const [hours, minutes] = value.split(":").map(Number);
  return hours * 60 + minutes;
}

function elapsedMinutes(from?: string, to?: string): number | undefined {
  const start = timeToMinutes(from);
  const end = timeToMinutes(to);
  if (start === undefined || end === undefined) return undefined;
  return end >= start ? end - start : end + 24 * 60 - start;
}

function routeLegMinutes(
  request: RouteInput,
  result: RouteResponse,
  destinationIndex: number,
): number | undefined {
  if (destinationIndex === 0) return undefined;
  const from = result.route[destinationIndex - 1];
  const to = result.route[destinationIndex];
  const fromIndex = request.locations.findIndex((location) => location.id === from.location_id);
  const toIndex = request.locations.findIndex((location) => location.id === to.location_id);
  const matrixValue =
    request.travel_time_matrix?.[fromIndex]?.[toIndex] ??
    request.sparse_travel_time_matrix?.[fromIndex]?.[toIndex];
  if (typeof matrixValue === "number" && Number.isInteger(matrixValue)) return matrixValue;
  return elapsedMinutes(
    from.departure_time ?? from.service_start_time ?? from.arrival_time,
    to.arrival_time,
  );
}

function getMapBounds(locationIds: string[]): MapBounds | null {
  const coordinates = locationIds.map((id) => SEOUL_LOCATION_COORDINATES[id]);
  if (coordinates.length < 2 || coordinates.some((coordinate) => !coordinate)) return null;
  const latitudes = coordinates.map((coordinate) => coordinate.latitude);
  const longitudes = coordinates.map((coordinate) => coordinate.longitude);
  const minimumLatitude = Math.min(...latitudes);
  const maximumLatitude = Math.max(...latitudes);
  const minimumLongitude = Math.min(...longitudes);
  const maximumLongitude = Math.max(...longitudes);
  return {
    minimumLatitude,
    minimumLongitude,
    latitudeRange: Math.max(maximumLatitude - minimumLatitude, 0.01),
    longitudeRange: Math.max(maximumLongitude - minimumLongitude, 0.01),
  };
}

function buildMapPoints(locationIds: string[], bounds: MapBounds): MapPoint[] | null {
  const coordinates = locationIds.map((id) => SEOUL_LOCATION_COORDINATES[id]);
  if (coordinates.length < 2 || coordinates.some((coordinate) => !coordinate)) return null;
  return locationIds.map((id, index) => {
    const coordinate = coordinates[index];
    return {
      id,
      name: coordinate.name,
      order: index + 1,
      x:
        MAP_PADDING +
        ((coordinate.longitude - bounds.minimumLongitude) / bounds.longitudeRange) *
          (MAP_WIDTH - MAP_PADDING * 2),
      y:
        MAP_HEIGHT -
        MAP_PADDING -
        ((coordinate.latitude - bounds.minimumLatitude) / bounds.latitudeRange) *
          (MAP_HEIGHT - MAP_PADDING * 2),
    };
  });
}

interface RouteMapProps {
  points: MapPoint[];
  variant: "input" | "result";
  title: string;
  ariaLabel: string;
}

function RouteMap({ points, variant, title, ariaLabel }: RouteMapProps) {
  const gridId = `route-map-grid-${variant}`;
  const arrowId = `route-map-arrow-${variant}`;
  return (
    <div className="route-map-panel">
      <div className="route-map-panel-heading">
        <strong>{title}</strong>
        <span>{points.map((point) => point.name).join(" → ")}</span>
      </div>
      <svg
        className={`route-map ${variant}`}
        viewBox={`0 0 ${MAP_WIDTH} ${MAP_HEIGHT}`}
        role="img"
        aria-label={ariaLabel}
      >
        <title>{points.map((point) => point.name).join("에서 ")} 순서의 서울 경로</title>
        <defs>
          <pattern id={gridId} width="32" height="32" patternUnits="userSpaceOnUse">
            <path d="M 32 0 L 0 0 0 32" className="route-map-grid-line" fill="none" />
          </pattern>
          <marker id={arrowId} markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto" markerUnits="strokeWidth">
            <path d="M0,0 L8,4 L0,8 z" className="route-map-arrow" />
          </marker>
        </defs>
        <rect width={MAP_WIDTH} height={MAP_HEIGHT} rx="14" className="route-map-background" />
        <rect width={MAP_WIDTH} height={MAP_HEIGHT} rx="14" fill={`url(#${gridId})`} />
        <g className="route-map-north" aria-hidden="true">
          <path d="M580 48 L580 76" />
          <path d="M574 56 L580 46 L586 56" />
          <text x="580" y="38">N</text>
        </g>
        {points.slice(1).map((point, index) => {
          const previous = points[index];
          return (
            <line
              key={`${previous.id}-${point.id}`}
              x1={previous.x}
              y1={previous.y}
              x2={point.x}
              y2={point.y}
              className="route-map-leg"
              markerEnd={`url(#${arrowId})`}
            />
          );
        })}
        {points.map((point, index) => {
          const markerType = index === 0
            ? "start"
            : index === points.length - 1
              ? "destination"
              : "waypoint";
          return (
            <g
              key={point.id}
              className={`route-map-point ${markerType}`}
              transform={`translate(${point.x} ${point.y})`}
            >
              <title>{point.order}. {point.name}</title>
              <circle r="15" />
              <text textAnchor="middle" dy="0.35em">{point.order}</text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}

export function RouteVisualization({ request, result }: RouteVisualizationProps) {
  const locationNames = new Map(
    request.locations.map((location) => [location.id, location.name || location.id]),
  );
  const inputLocationIds = request.locations.map((location) => location.id);
  const resultLocationIds = result.route.map((stop) => stop.location_id);
  const bounds = getMapBounds(inputLocationIds);
  const inputMapPoints = bounds ? buildMapPoints(inputLocationIds, bounds) : null;
  const resultMapPoints = bounds ? buildMapPoints(resultLocationIds, bounds) : null;
  const hasComparableMaps = Boolean(inputMapPoints && resultMapPoints);
  const changedIntermediateCount = resultLocationIds.reduce(
    (count, id, index) =>
      index > 0 && index < resultLocationIds.length - 1 && inputLocationIds[index] !== id
        ? count + 1
        : count,
    0,
  );
  const inputPath = inputLocationIds.map((id) => locationNames.get(id) ?? id).join(" → ");
  const resultPath = resultLocationIds.map((id) => locationNames.get(id) ?? id).join(" → ");
  const totalWait = result.route.reduce((sum, stop) => sum + (stop.wait_minutes ?? 0), 0);
  const totalStay = result.route.reduce((sum, stop) => sum + (stop.stay_minutes ?? 0), 0);
  const firstStop = result.route[0];
  const lastStop = result.route[result.route.length - 1];
  const scheduleMinutes = elapsedMinutes(
    result.selected_start_time ?? firstStop?.departure_time ?? firstStop?.arrival_time,
    lastStop?.departure_time ?? lastStop?.service_start_time ?? lastStop?.arrival_time,
  );

  return (
    <section className="route-visualization" aria-labelledby="route-visualization-title">
      <div className="route-visualization-heading">
        <div>
          <h3 id="route-visualization-title">결과 시각화</h3>
          <p>입력 순서와 최적화 결과, 이동·대기·체류 시간을 함께 표시합니다.</p>
        </div>
        <div className="route-visualization-metrics" aria-label="경로 요약 지표">
          <span><strong>{result.total_travel_minutes}</strong>분 이동</span>
          <span><strong>{totalWait}</strong>분 대기</span>
          <span><strong>{totalStay}</strong>분 체류</span>
          {scheduleMinutes !== undefined && <span><strong>{scheduleMinutes}</strong>분 일정</span>}
        </div>
      </div>

      <div className="route-order-comparison" aria-label="입력과 최적화 경로 비교">
        <div>
          <span>입력</span>
          <strong aria-label="입력 경로">{inputPath}</strong>
        </div>
        <span className="route-order-arrow" aria-hidden="true">→</span>
        <div>
          <span>결과</span>
          <strong aria-label="최적화 결과 경로">{resultPath}</strong>
        </div>
        <span className={`route-change-badge ${changedIntermediateCount === 0 ? "unchanged" : ""}`}>
          {changedIntermediateCount === 0 ? "순서 유지" : `경유지 ${changedIntermediateCount}곳 변경`}
        </span>
      </div>

      {result.time_dependent_revalidation && (
        <div
          className={`route-revalidation ${result.time_dependent_revalidation.applied ? "applied" : "warning"}`}
          role={result.time_dependent_revalidation.warning_code ? "status" : undefined}
        >
          <strong>실제 출발 시각 재검증</strong>
          <span>
            {result.time_dependent_revalidation.revalidated_leg_count}개 구간 확인 · {result.time_dependent_revalidation.changed_leg_count}개 변경 · 최대 {result.time_dependent_revalidation.max_delta_minutes}분 차이
          </span>
          {result.time_dependent_revalidation.warning_detail && (
            <small>{result.time_dependent_revalidation.warning_detail}</small>
          )}
        </div>
      )}

      <div className={`route-visualization-grid ${hasComparableMaps ? "has-map" : ""}`}>
        {inputMapPoints && resultMapPoints && (
          <article className="route-map-card">
            <div className="route-card-heading">
              <div>
                <h4>서울 경로 비교</h4>
                <p>두 지도는 동일한 실제 좌표와 배율을 사용합니다.</p>
              </div>
              <div className="route-map-legend" aria-label="지도 범례">
                <span className="start">출발</span>
                <span className="waypoint">경유</span>
                <span className="destination">도착</span>
              </div>
            </div>
            <div className="route-map-comparison">
              <RouteMap
                points={inputMapPoints}
                variant="input"
                title="입력 순서"
                ariaLabel="서울 입력 경로 지도"
              />
              <RouteMap
                points={resultMapPoints}
                variant="result"
                title="최적화 결과"
                ariaLabel="서울 최적화 결과 지도"
              />
            </div>
          </article>
        )}

        <article className="route-journey-card">
          <div className="route-card-heading">
            <div>
              <h4>방문 타임라인</h4>
              <p>{result.route.length}개 장소의 계산된 일정</p>
            </div>
          </div>
          <ol className="route-journey-list" aria-label="방문 타임라인">
            {result.route.map((stop, index) => {
              const legMinutes = routeLegMinutes(request, result, index);
              const locationName = locationNames.get(stop.location_id) ?? stop.location_id;
              const role = index === 0
                ? "출발"
                : index === result.route.length - 1
                  ? "도착"
                  : "경유";
              return (
                <li key={`${stop.order}-${stop.location_id}`}>
                  {index > 0 && (
                    <div className="route-journey-leg" aria-label={`${legMinutes ?? "알 수 없음"}분 이동`}>
                      <span />
                      <strong>{legMinutes === undefined ? "이동" : `${legMinutes}분 이동`}</strong>
                    </div>
                  )}
                  <div className="route-journey-stop">
                    <span className={`route-order-badge ${index === 0 ? "start" : index === result.route.length - 1 ? "destination" : ""}`}>
                      {index + 1}
                    </span>
                    <div className="route-stop-identity">
                      <span>{role}</span>
                      <strong>{locationName}</strong>
                      {locationName !== stop.location_id && <code>{stop.location_id}</code>}
                    </div>
                    <div className="route-stop-schedule">
                      <span><small>도착</small>{stop.arrival_time}</span>
                      {stop.departure_time && <span><small>출발</small>{stop.departure_time}</span>}
                      <span className="wait"><small>대기</small>{stop.wait_minutes ?? 0}분</span>
                      <span className="stay"><small>체류</small>{stop.stay_minutes ?? 0}분</span>
                    </div>
                  </div>
                </li>
              );
            })}
          </ol>
        </article>
      </div>
    </section>
  );
}
