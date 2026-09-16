import { useEffect, useState } from "react";
import {
  Button,
  Callout,
  Card,
  Classes,
  Divider,
  Intent,
  NonIdealState,
  Tag,
} from "@blueprintjs/core";
import type { TestbedJob } from "../../jobs";
import { JobSummary } from "./JobSummary";

function statusIntent(status: number) {
  if (status >= 500) return Intent.DANGER;
  if (status >= 400) return Intent.WARNING;
  if (status >= 200 && status < 300) return Intent.SUCCESS;
  return Intent.NONE;
}

export function JobDetail({ job }: { job: TestbedJob | null }) {
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState("");

  useEffect(() => {
    setCopied(false);
    setCopyError("");
  }, [job?.id]);

  if (!job) {
    return (
      <Card className="job-detail job-detail-empty" elevation={1} compact>
        <NonIdealState
          icon="search"
          title="실행 정보를 확인할 Job을 선택하세요."
          description="새 Job은 생성 즉시 사이드바에 표시됩니다."
        />
      </Card>
    );
  }

  const raw = job.response
    ? job.response.body !== null
      ? JSON.stringify(job.response.body, null, 2)
      : job.response.raw
    : "";

  return (
    <Card className="job-detail" elevation={1} compact>
      <div className="detail-heading">
        <h1 className={Classes.HEADING}>Job 상세</h1>
        <span className={Classes.TEXT_MUTED}>Timeline은 Task 3에서 제공</span>
      </div>
      <Divider />
      <div className="job-detail-content">
        <JobSummary job={job} />

        {job.error && (
          <Callout compact intent={Intent.DANGER} role="alert" title="요청 실패">
            {job.error}
          </Callout>
        )}

        {job.route && (
          <section className="route-summary" aria-labelledby="route-title">
            <div className="route-overview">
              <div>
                <h2 id="route-title" className={Classes.HEADING}>
                  경로
                </h2>
                <div aria-label="방문 순서">
                  {job.route.route.map((stop) => stop.location_id).join(" → ")}
                </div>
              </div>
              <div>
                <span className={Classes.TEXT_MUTED}>총 이동 시간</span>
                <strong>{job.route.total_travel_minutes}분</strong>
              </div>
            </div>
            <div className="table-scroll">
              <table
                className={`${Classes.HTML_TABLE} ${Classes.HTML_TABLE_BORDERED} ${Classes.HTML_TABLE_STRIPED}`}
              >
                <thead>
                  <tr>
                    <th>순서</th>
                    <th>위치</th>
                    <th>도착</th>
                    <th>출발</th>
                  </tr>
                </thead>
                <tbody>
                  {job.route.route.map((stop, index) => (
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
        )}

        <section className="raw-response" aria-labelledby="raw-title">
          <div className="raw-heading">
            <h2 id="raw-title" className={Classes.HEADING}>
              최근 Response
            </h2>
            <div className="response-metadata" aria-live="polite">
              {job.response && (
                <>
                  <Tag intent={statusIntent(job.response.status)}>
                    HTTP {job.response.status}
                  </Tag>
                  <Tag icon="stopwatch" minimal>
                    {job.response.request_latency_ms.toFixed(1)} ms
                  </Tag>
                </>
              )}
              <Button
                icon={copied ? "tick" : "clipboard"}
                intent={copied ? Intent.SUCCESS : Intent.NONE}
                variant="minimal"
                size="small"
                disabled={!job.response}
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(raw);
                    setCopied(true);
                  } catch {
                    setCopyError("복사 실패. 응답을 선택해 직접 복사하세요.");
                  }
                }}
              >
                {copied ? "복사됨" : "복사"}
              </Button>
            </div>
          </div>
          {copyError && <Callout intent={Intent.WARNING}>{copyError}</Callout>}
          <pre
            className={`${Classes.CODE_BLOCK} raw-output`}
            aria-label="Raw API 응답"
            aria-busy={job.status === "pending" || job.status === "running"}
          >
            {raw ||
              (job.status === "pending" || job.status === "running"
                ? "API 응답 대기 중…"
                : "아직 응답이 없습니다.")}
          </pre>
        </section>
      </div>
    </Card>
  );
}
