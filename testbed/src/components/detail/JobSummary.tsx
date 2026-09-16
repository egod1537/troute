import { useEffect, useState } from "react";
import { Classes, Intent, ProgressBar, Tag } from "@blueprintjs/core";
import { JOB_STATUS_LABELS, type TestbedJob } from "../../jobs";

const STATUS_INTENTS = {
  pending: Intent.NONE,
  running: Intent.PRIMARY,
  completed: Intent.SUCCESS,
  failed: Intent.DANGER,
  cancelled: Intent.WARNING,
} as const;

function formatElapsed(milliseconds: number) {
  const totalSeconds = Math.max(0, milliseconds) / 1000;
  if (totalSeconds < 60) return `${totalSeconds.toFixed(1)}초`;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = Math.floor(totalSeconds % 60);
  return `${minutes}분 ${seconds.toString().padStart(2, "0")}초`;
}

const STAGE_LABELS: Record<string, string> = {
  queued: "대기",
  optimizing: "최적화",
  completed: "완료",
  failed: "오류",
  cancelled: "취소됨",
};

function useElapsed(job: TestbedJob) {
  const [, setTick] = useState(0);

  useEffect(() => {
    if (job.status !== "running" && job.status !== "pending") return;
    const interval = window.setInterval(() => setTick((tick) => tick + 1), 500);
    return () => window.clearInterval(interval);
  }, [job.status]);

  return formatElapsed((job.completedAt ?? Date.now()) - job.createdAt);
}

export function JobSummary({ job }: { job: TestbedJob }) {
  const elapsed = useElapsed(job);

  return (
    <section className="job-summary" aria-labelledby="job-summary-title">
      <div className="job-summary-heading">
        <div>
          <h2 id="job-summary-title" className={Classes.HEADING}>
            {job.id}
          </h2>
          <span className={Classes.TEXT_MUTED}>프론트엔드 관찰 모델</span>
        </div>
        <Tag intent={STATUS_INTENTS[job.status]}>
          {JOB_STATUS_LABELS[job.status]}
        </Tag>
      </div>

      <dl className="job-facts">
        <div>
          <dt>상태</dt>
          <dd>{JOB_STATUS_LABELS[job.status]}</dd>
        </div>
        <div>
          <dt>진행률</dt>
          <dd>{job.progress}%</dd>
        </div>
        <div>
          <dt>단계</dt>
          <dd>{job.stage ? (STAGE_LABELS[job.stage] ?? job.stage) : "—"}</dd>
        </div>
        <div>
          <dt>생성 시각</dt>
          <dd>
            {new Date(job.createdAt).toLocaleString("ko-KR", {
              hour12: false,
            })}
          </dd>
        </div>
        <div>
          <dt>경과 시간</dt>
          <dd>{elapsed}</dd>
        </div>
      </dl>
      <ProgressBar
        aria-label={`Job 진행률 ${job.progress}%`}
        intent={STATUS_INTENTS[job.status]}
        value={job.progress / 100}
        animate={job.status === "running"}
        stripes={job.status === "running"}
      />
    </section>
  );
}
