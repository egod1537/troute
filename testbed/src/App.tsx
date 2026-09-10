import { useEffect, useMemo, useRef, useState } from "react";
import {
  API_BASE,
  ROUTE_PATH,
  ApiError,
  checkHealth,
  parseInput,
  runRoute,
  type ApiResponse,
  type RouteResponse,
} from "./api";
import { LatencyChart, type Measurement } from "./LatencyChart";
import { sample } from "./sample";

export function App() {
  const [input, setInput] = useState(JSON.stringify(sample, null, 2));
  const [health, setHealth] = useState<"checking" | "online" | "offline">(
    "checking",
  );
  const [busy, setBusy] = useState(false);
  const [validation, setValidation] = useState<{
    valid: boolean;
    message: string;
  } | null>(null);
  const [error, setError] = useState("");
  const [response, setResponse] = useState<ApiResponse | null>(null);
  const [route, setRoute] = useState<RouteResponse | null>(null);
  const [measurements, setMeasurements] = useState<Measurement[]>([]);
  const [copied, setCopied] = useState(false);
  const sequence = useRef(0);
  const locations = useMemo(() => {
    try {
      const value = JSON.parse(input);
      return Array.isArray(value?.locations) ? value.locations.length : null;
    } catch {
      return null;
    }
  }, [input]);

  function validate() {
    try {
      const value = parseInput(input);
      setValidation({
        valid: true,
        message: "Valid v0 input shape. Feasibility is checked by the backend.",
      });
      return value;
    } catch (error) {
      setValidation({ valid: false, message: (error as Error).message });
      return null;
    }
  }

  function remember(value: ApiResponse, kind: Measurement["kind"]) {
    setResponse(value);
    const point = {
      id: ++sequence.current,
      latency: value.request_latency_ms,
      kind,
    };
    setMeasurements((previous) => [...previous, point].slice(-40));
  }

  async function execute(kind: Measurement["kind"]) {
    const payload = kind === "route" ? validate() : null;
    if (kind === "route" && !payload) return;
    setBusy(true);
    setError("");
    setResponse(null);
    setCopied(false);
    if (kind === "health") setHealth("checking");
    else setRoute(null);
    try {
      if (kind === "health") {
        remember(await checkHealth(), kind);
        setHealth("online");
      } else if (payload) {
        const result = await runRoute(payload);
        remember(result.response, kind);
        setRoute(result.route);
      }
    } catch (error) {
      setError((error as Error).message);
      if (error instanceof ApiError && error.response)
        remember(error.response, kind);
      if (kind === "health") setHealth("offline");
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    void execute("health");
  }, []);

  const raw = response
    ? response.body !== null
      ? JSON.stringify(response.body, null, 2)
      : response.raw
    : "";
  return (
    <div className="workspace">
      <header className="topbar">
        <a className="brand" href="#top" aria-label="troute testbed">
          <span className="brand-icon">↱</span>
          <strong>troute</strong>
          <span className="brand-divider">/</span>
          <span>testbed</span>
        </a>
        <div className="topbar-meta">
          <span className="tag">DEVELOPER TOOL</span>
          <span className="branch">
            <span>⑂</span> main deployment
          </span>
        </div>
      </header>

      <main id="top">
        <div className="page-heading">
          <div>
            <div className="eyebrow">
              ROUTING WORKBENCH <span>v0.1</span>
            </div>
            <h1>Inspect the journey.</h1>
            <p>Compose an input. Run a request. Understand the result.</p>
          </div>
          <div className="connection">
            <div className="connection-label">API CONNECTION</div>
            <div className="connection-row">
              <span className={`status ${health}`}>
                <i />
                API:{" "}
                {health === "online"
                  ? "Online"
                  : health === "offline"
                    ? "Offline"
                    : "Checking"}
              </span>
              <button
                className="icon-button"
                aria-label="Refresh API health"
                disabled={busy}
                onClick={() => void execute("health")}
              >
                ↻
              </button>
            </div>
            <code>{API_BASE}</code>
          </div>
        </div>

        {!ROUTE_PATH && (
          <div className="notice">
            <span className="notice-symbol">i</span>
            <div>
              <strong>Health check mode</strong>
              <span>
                The API currently exposes only <code>GET /health</code>. Route
                execution and solver timing are not available yet.
              </span>
            </div>
            <span className="tag">NO SOLVER RUN</span>
          </div>
        )}

        <div className="main-grid">
          <section className="panel input-panel" aria-labelledby="input-title">
            <div className="panel-heading">
              <div>
                <span className="section-number">01</span>
                <h2 id="input-title">Input</h2>
              </div>
              <span className="subtle mono">v0 · JSON</span>
            </div>
            <div className="editor-toolbar">
              <span className="file-label">request.json</span>
              <div>
                <button
                  className="text-button"
                  onClick={() => {
                    const parsed = validate();
                    if (parsed) setInput(JSON.stringify(parsed, null, 2));
                  }}
                >
                  Format
                </button>
                <button
                  className="text-button"
                  onClick={() => {
                    setInput(JSON.stringify(sample, null, 2));
                    setValidation(null);
                  }}
                >
                  Load sample
                </button>
              </div>
            </div>
            <label className="sr-only" htmlFor="route-input">
              Route input JSON
            </label>
            <textarea
              id="route-input"
              spellCheck={false}
              autoCapitalize="off"
              value={input}
              onChange={(event) => {
                setInput(event.target.value);
                setValidation(null);
              }}
            />
            <div className="editor-footer">
              <span>{locations ?? "—"} locations</span>
              <span>UTF-8 · JSON</span>
            </div>
            <p className="input-note">
              Sample is an input-format example. Its placeholder Place IDs are
              not connected to a routing provider.
            </p>
            {validation && (
              <p
                role={validation.valid ? "status" : "alert"}
                className={`validation ${validation.valid ? "valid" : "invalid"}`}
              >
                {validation.message}
              </p>
            )}
            <div className="run-actions">
              <button className="secondary" disabled={busy} onClick={validate}>
                Validate JSON
              </button>
              <button
                className="primary"
                disabled={busy}
                onClick={() => void execute(ROUTE_PATH ? "route" : "health")}
              >
                <span aria-hidden="true">{busy ? "◌" : "▶"}</span>
                {busy
                  ? "Running…"
                  : ROUTE_PATH
                    ? "Run route"
                    : "Run health check"}
              </button>
            </div>
            <p className="run-note">
              {ROUTE_PATH
                ? `POST ${ROUTE_PATH} · calculations run on the backend`
                : "GET /health · input payload is not submitted"}
            </p>
          </section>

          <div className="result-column">
            <section
              className="panel route-panel"
              aria-labelledby="route-title"
            >
              <div className="panel-heading">
                <div>
                  <span className="section-number">02</span>
                  <h2 id="route-title">Route result</h2>
                </div>
                <span className="tag neutral">
                  {route ? "LAST ROUTE" : "AWAITING ROUTE API"}
                </span>
              </div>
              {route ? (
                <>
                  <div className="route-flow" aria-label="Visit order">
                    {route.route.map((stop, index) => (
                      <span key={`${stop.order}-${index}`}>
                        <b>{stop.location_id}</b>
                        {index < route.route.length - 1 && <i>→</i>}
                      </span>
                    ))}
                  </div>
                  <div className="table-scroll">
                    <table>
                      <thead>
                        <tr>
                          <th>Order</th>
                          <th>Location</th>
                          <th>Arrival</th>
                          <th>Departure</th>
                        </tr>
                      </thead>
                      <tbody>
                        {route.route.map((stop, index) => (
                          <tr key={index}>
                            <td>{stop.order}</td>
                            <td>{stop.location_id}</td>
                            <td>{stop.arrival_time}</td>
                            <td>{stop.departure_time ?? "—"}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                  <div className="route-total">
                    Total travel time{" "}
                    <strong>{route.total_travel_minutes} min</strong>
                  </div>
                </>
              ) : (
                <div className="empty-route">
                  <div className="empty-path">
                    <i />
                    <span />
                    <i />
                    <span />
                    <i />
                  </div>
                  <h3>No route has been calculated</h3>
                  <p>
                    {ROUTE_PATH
                      ? "Submit an input to see visit order and arrival / departure times."
                      : "Visit order and arrival / departure times will appear when a route endpoint is available."}
                  </p>
                  <div className="empty-columns">
                    ORDER <span>LOCATION</span>
                    <span>ARRIVAL</span>
                    <span>DEPARTURE</span>
                  </div>
                </div>
              )}
            </section>

            <section
              className="panel performance-panel"
              aria-labelledby="performance-title"
            >
              <div className="panel-heading">
                <div>
                  <span className="section-number">03</span>
                  <h2 id="performance-title">Performance</h2>
                </div>
                <span className="subtle mono">this session</span>
              </div>
              <div className="metrics">
                <div>
                  <span>Input locations</span>
                  <strong>{locations ?? "—"}</strong>
                </div>
                <div>
                  <span>Last request</span>
                  <strong>
                    {response ? response.request_latency_ms.toFixed(1) : "—"}
                    <small>ms</small>
                  </strong>
                </div>
                <div>
                  <span>Solver latency</span>
                  <strong className="unavailable">—</strong>
                  <small>not provided</small>
                </div>
                <div>
                  <span>Last route travel</span>
                  <strong>
                    {route?.total_travel_minutes ?? "—"}
                    <small>min</small>
                  </strong>
                </div>
              </div>
              <div className="chart-heading">
                <h3>HTTP round-trip latency</h3>
                <span>
                  <i className="legend-dot health" />
                  Health <i className="legend-dot route" />
                  Route
                </span>
              </div>
              <LatencyChart measurements={measurements} />
              <div className="chart-footer">
                <span>Last 40 completed HTTP requests · resets on refresh</span>
                <button
                  className="text-button"
                  disabled={!measurements.length}
                  onClick={() => setMeasurements([])}
                >
                  Clear trace
                </button>
              </div>
              <p className="timing-note">
                Browser → API → browser time, including proxy and response
                transfer. This is not solver execution time.
              </p>
            </section>
          </div>
        </div>

        <section className="panel raw-panel" aria-labelledby="raw-title">
          <div className="panel-heading">
            <div>
              <span className="section-number">04</span>
              <h2 id="raw-title">Raw response</h2>
              {response && (
                <span
                  className={`http-status ${response.status >= 400 ? "failed" : ""}`}
                >
                  HTTP {response.status}
                </span>
              )}
            </div>
            <div className="raw-actions">
              <span className="subtle mono">
                {response
                  ? `${response.method} ${response.path}`
                  : "No response"}
              </span>
              <button
                className="text-button"
                disabled={!response}
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(raw);
                    setCopied(true);
                  } catch {
                    setError(
                      "Copy failed. Select and copy the raw response manually.",
                    );
                  }
                }}
              >
                {copied ? "Copied" : "Copy JSON"}
              </button>
            </div>
          </div>
          {error && (
            <div className="request-error" role="alert">
              <strong>Request failed</strong>
              <span>{error}</span>
            </div>
          )}
          <pre aria-label="Raw API response">
            {raw || (busy ? "Waiting for the API…" : "No response received.")}
          </pre>
        </section>
        <footer>
          <span>
            <b>troute</b> / developer testbed
          </span>
          <span>
            Local session only <i /> No benchmark persistence
          </span>
        </footer>
      </main>
    </div>
  );
}
