export interface Measurement {
  id: number;
  latency: number;
  kind: "health" | "route";
}

export function LatencyChart({
  measurements,
}: {
  measurements: Measurement[];
}) {
  const max = Math.max(
    10,
    Math.ceil(Math.max(...measurements.map((item) => item.latency), 0) / 10) *
      10,
  );
  const x = (index: number) =>
    48 + (index * 502) / Math.max(measurements.length - 1, 1);
  const y = (latency: number) => 150 - (latency / max) * 120;
  return (
    <div className="chart">
      {!measurements.length ? (
        <div className="chart-empty">
          Run a request to start a session trace.
        </div>
      ) : (
        <svg
          viewBox="0 0 580 182"
          role="img"
          aria-label={`Request latency chart, ${measurements.length} session requests, in milliseconds`}
        >
          {[0, 0.5, 1].map((fraction) => (
            <g key={fraction}>
              <line
                x1="48"
                x2="550"
                y1={y(fraction * max)}
                y2={y(fraction * max)}
                className="grid-line"
              />
              <text x="36" y={y(fraction * max) + 4} textAnchor="end">
                {Math.round(fraction * max)}
              </text>
            </g>
          ))}
          <text x="12" y="15">
            ms
          </text>
          <polyline
            points={measurements
              .map((item, i) => `${x(i)},${y(item.latency)}`)
              .join(" ")}
            className="trace"
          />
          {measurements.map((item, index) => (
            <circle
              key={item.id}
              cx={x(index)}
              cy={y(item.latency)}
              r="4"
              className={item.kind}
            >
              <title>
                {item.kind} #{item.id}: {item.latency.toFixed(1)} ms
              </title>
            </circle>
          ))}
          <text x="48" y="175">
            #{measurements[0].id}
          </text>
          <text x="550" y="175" textAnchor="end">
            Request #{measurements[measurements.length - 1].id}
          </text>
        </svg>
      )}
    </div>
  );
}
