import { useEffect, useState } from "react";
import { Classes, Intent, ProgressBar, Tag } from "@blueprintjs/core";
import { JOB_STATUS_LABELS, type TestbedJob } from "../../jobs";

const STATUS_INTENTS = {
  pending: Intent.NONE,
  running: Intent.PRIMARY,
  completed: Intent.SUCCESS,
  failed: Intent.DANGER,
} as const;

function formatElapsed(milliseconds: number) {
  const totalSeconds = Math.max(0, milliseconds) / 1000;
  if (totalSeconds < 60) return `${totalSeconds.toFixed(1)}s`;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = Math.floor(totalSeconds % 60);
  return `${minutes}m ${seconds.toString().padStart(2, "0")}s`;
}

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
          <span className={Classes.TEXT_MUTED}>Frontend observation model</span>
        </div>
        <Tag intent={STATUS_INTENTS[job.status]}>
          {JOB_STATUS_LABELS[job.status]}
        </Tag>
      </div>

      <dl className="job-facts">
        <div>
          <dt>Status</dt>
          <dd>{JOB_STATUS_LABELS[job.status]}</dd>
        </div>
        <div>
          <dt>Progress</dt>
          <dd>{job.progress}%</dd>
        </div>
        <div>
          <dt>Stage</dt>
          <dd>{job.stage ?? "—"}</dd>
        </div>
        <div>
          <dt>Created</dt>
          <dd>{new Date(job.createdAt).toLocaleString()}</dd>
        </div>
        <div>
          <dt>Elapsed</dt>
          <dd>{elapsed}</dd>
        </div>
      </dl>
      <ProgressBar
        aria-label={`Job progress ${job.progress}%`}
        intent={STATUS_INTENTS[job.status]}
        value={job.progress / 100}
        animate={job.status === "running"}
        stripes={job.status === "running"}
      />
    </section>
  );
}
