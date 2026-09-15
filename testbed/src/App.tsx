import { useEffect, useState } from "react";
import {
  Alignment,
  Button,
  ButtonGroup,
  Callout,
  Card,
  Classes,
  Divider,
  Intent,
  Navbar,
  NavbarDivider,
  NavbarGroup,
  NavbarHeading,
  Tag,
  TextArea,
} from "@blueprintjs/core";
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
import { BUILD_COMMIT } from "./commit";
import { sample } from "./sample";

type HealthState = "checking" | "online" | "offline";
type RequestKind = "health" | "route";

function healthIntent(health: HealthState) {
  if (health === "online") return Intent.SUCCESS;
  if (health === "offline") return Intent.DANGER;
  return Intent.PRIMARY;
}

function statusIntent(status: number) {
  if (status >= 500) return Intent.DANGER;
  if (status >= 400) return Intent.WARNING;
  if (status >= 200 && status < 300) return Intent.SUCCESS;
  return Intent.NONE;
}

function CommitIndicator() {
  const tag = (
    <Tag
      aria-label={`Commit ${BUILD_COMMIT.shortSha}`}
      className={`${Classes.MONOSPACE_TEXT} navbar-commit`}
      icon="git-commit"
      minimal
      title={
        BUILD_COMMIT.fullSha
          ? `Commit ${BUILD_COMMIT.fullSha}`
          : "Commit unknown"
      }
    >
      <span className="navbar-commit-prefix">commit </span>
      {BUILD_COMMIT.shortSha}
    </Tag>
  );

  if (!BUILD_COMMIT.url) return tag;
  return (
    <a
      aria-label={`Commit ${BUILD_COMMIT.fullSha}`}
      className="navbar-commit-link"
      href={BUILD_COMMIT.url}
      target="_blank"
      rel="noreferrer"
      title={`Commit ${BUILD_COMMIT.fullSha}`}
    >
      {tag}
    </a>
  );
}

export function App() {
  const [input, setInput] = useState(JSON.stringify(sample, null, 2));
  const [health, setHealth] = useState<HealthState>("checking");
  const [busy, setBusy] = useState(false);
  const [validation, setValidation] = useState<{
    valid: boolean;
    message: string;
  } | null>(null);
  const [error, setError] = useState("");
  const [response, setResponse] = useState<ApiResponse | null>(null);
  const [route, setRoute] = useState<RouteResponse | null>(null);
  const [copied, setCopied] = useState(false);

  function validate() {
    try {
      const value = parseInput(input);
      setValidation({ valid: true, message: "Valid" });
      return value;
    } catch (error) {
      setValidation({ valid: false, message: (error as Error).message });
      return null;
    }
  }

  async function execute(kind: RequestKind) {
    const payload = kind === "route" ? validate() : null;
    if (kind === "route" && !payload) return;

    setBusy(true);
    setError("");
    setResponse(null);
    setRoute(null);
    setCopied(false);
    if (kind === "health") setHealth("checking");

    try {
      if (kind === "health") {
        setResponse(await checkHealth());
        setHealth("online");
      } else if (payload) {
        const result = await runRoute(payload);
        setResponse(result.response);
        setRoute(result.route);
      }
    } catch (error) {
      setError((error as Error).message);
      if (error instanceof ApiError && error.response) {
        setResponse(error.response);
      }
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
  const healthLabel =
    health === "online"
      ? "Online"
      : health === "offline"
        ? "Offline"
        : "Checking";

  return (
    <div className="app-shell">
      <Navbar className="app-navbar">
        <NavbarGroup align={Alignment.START}>
          <NavbarHeading>troute testbed</NavbarHeading>
          <NavbarDivider />
          <code className={`${Classes.MONOSPACE_TEXT} navbar-endpoint`}>
            {API_BASE} · {ROUTE_PATH ? `POST ${ROUTE_PATH}` : "GET /health only"}
          </code>
          <NavbarDivider />
          <CommitIndicator />
        </NavbarGroup>
        <NavbarGroup align={Alignment.END}>
          <span className={Classes.TEXT_MUTED}>API</span>
          <Tag
            aria-label={`API ${healthLabel}`}
            icon={
              health === "online"
                ? "tick-circle"
                : health === "offline"
                  ? "error"
                  : "time"
            }
            intent={healthIntent(health)}
            minimal
          >
            {healthLabel}
          </Tag>
          <Button
            aria-label="Refresh API health"
            title="Refresh API health"
            icon="refresh"
            loading={busy && health === "checking"}
            disabled={busy}
            variant="minimal"
            onClick={() => void execute("health")}
          />
        </NavbarGroup>
      </Navbar>

      <main className="playground">
        <Card className="workspace-card request-card" elevation={1} compact>
          <div className="card-heading">
            <h1 className={Classes.HEADING}>Request</h1>
            <ButtonGroup size="small" variant="minimal">
              <Button
                icon="code"
                disabled={busy}
                onClick={() => {
                  const parsed = validate();
                  if (parsed) setInput(JSON.stringify(parsed, null, 2));
                }}
              >
                Format
              </Button>
              <Button
                icon="reset"
                disabled={busy}
                onClick={() => {
                  setInput(JSON.stringify(sample, null, 2));
                  setValidation(null);
                }}
              >
                Reset sample
              </Button>
            </ButtonGroup>
          </div>
          <Divider />

          <div className="request-content">
            <TextArea
              aria-label="Request JSON"
              className="json-editor"
              fill
              intent={
                validation && !validation.valid ? Intent.DANGER : Intent.NONE
              }
              spellCheck={false}
              autoCapitalize="off"
              value={input}
              onChange={(event) => {
                setInput(event.target.value);
                setValidation(null);
              }}
            />

            {!ROUTE_PATH && (
              <Callout compact intent={Intent.PRIMARY} title="Health-only mode">
                Run checks GET /health; the request body is not submitted.
              </Callout>
            )}

            {validation && !validation.valid && (
              <Callout
                compact
                intent={Intent.DANGER}
                role="alert"
                title="Invalid request"
              >
                {validation.message}
              </Callout>
            )}

            <div className="request-actions">
              <div aria-live="polite">
                {validation?.valid && (
                  <Tag icon="tick" intent={Intent.SUCCESS} minimal>
                    Valid
                  </Tag>
                )}
              </div>
              <ButtonGroup>
                <Button icon="tick" disabled={busy} onClick={validate}>
                  Validate
                </Button>
                <Button
                  icon="play"
                  intent={Intent.PRIMARY}
                  loading={busy}
                  disabled={busy}
                  onClick={() => void execute(ROUTE_PATH ? "route" : "health")}
                >
                  Run
                </Button>
              </ButtonGroup>
            </div>
          </div>
        </Card>

        <Card className="workspace-card response-card" elevation={1} compact>
          <div className="card-heading">
            <h1 className={Classes.HEADING}>Response</h1>
            <div className="response-metadata" aria-live="polite">
              {response ? (
                <>
                  <Tag intent={statusIntent(response.status)}>
                    HTTP {response.status}
                  </Tag>
                  <Tag icon="stopwatch" minimal>
                    {response.request_latency_ms.toFixed(1)} ms
                  </Tag>
                </>
              ) : (
                <span className={Classes.TEXT_MUTED}>No response yet</span>
              )}
            </div>
          </div>
          <Divider />

          <div className="response-content">
            {error && (
              <Callout
                compact
                intent={Intent.DANGER}
                role="alert"
                title="Request failed"
              >
                {error}
              </Callout>
            )}

            {route && (
              <>
                <section className="route-summary" aria-labelledby="route-title">
                  <div className="route-overview">
                    <div>
                      <h2 id="route-title" className={Classes.HEADING}>
                        Route
                      </h2>
                      <div aria-label="Visit order">
                        {route.route.map((stop) => stop.location_id).join(" → ")}
                      </div>
                    </div>
                    <div>
                      <span className={Classes.TEXT_MUTED}>Total travel</span>
                      <strong>{route.total_travel_minutes} min</strong>
                    </div>
                  </div>
                  <div className="table-scroll">
                    <table
                      className={`${Classes.HTML_TABLE} ${Classes.HTML_TABLE_BORDERED} ${Classes.HTML_TABLE_STRIPED}`}
                    >
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
                          <tr key={`${stop.order}-${index}`}>
                            <td>{stop.order}</td>
                            <td>{stop.location_id}</td>
                            <td>{stop.arrival_time}</td>
                            <td>{stop.departure_time ?? "—"}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </section>
                <Divider />
              </>
            )}

            <section className="raw-response" aria-labelledby="raw-title">
              <div className="raw-heading">
                <h2 id="raw-title" className={Classes.HEADING}>
                  Raw response
                </h2>
                <Button
                  icon={copied ? "tick" : "clipboard"}
                  intent={copied ? Intent.SUCCESS : Intent.NONE}
                  variant="minimal"
                  size="small"
                  disabled={!response}
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(raw);
                      setCopied(true);
                    } catch {
                      setError(
                        "Copy failed. Select and copy the response manually.",
                      );
                    }
                  }}
                >
                  {copied ? "Copied" : "Copy"}
                </Button>
              </div>
              <pre
                className={`${Classes.CODE_BLOCK} raw-output`}
                aria-label="Raw API response"
                aria-busy={busy}
              >
                {raw || (busy ? "Waiting for the API…" : "No response yet.")}
              </pre>
            </section>
          </div>
        </Card>
      </main>
    </div>
  );
}
