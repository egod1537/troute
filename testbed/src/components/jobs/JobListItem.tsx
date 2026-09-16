import { Classes, Intent, Tag } from "@blueprintjs/core";
import { JOB_STATUS_LABELS, type TestbedJob } from "../../jobs";

const STATUS_INTENTS = {
  pending: Intent.NONE,
  running: Intent.PRIMARY,
  completed: Intent.SUCCESS,
  failed: Intent.DANGER,
} as const;

interface JobListItemProps {
  job: TestbedJob;
  selected: boolean;
  onSelect: () => void;
}

export function JobListItem({ job, selected, onSelect }: JobListItemProps) {
  return (
    <button
      type="button"
      className="job-list-item"
      role="option"
      aria-selected={selected}
      data-job-id={job.id}
      data-status={job.status}
      onClick={onSelect}
    >
      <span className="job-list-item-primary">
        <span className={`${Classes.MONOSPACE_TEXT} job-list-item-id`}>
          {job.id}
        </span>
        <Tag intent={STATUS_INTENTS[job.status]} minimal>
          {JOB_STATUS_LABELS[job.status]}
        </Tag>
      </span>
      <span className={`${Classes.TEXT_MUTED} job-list-item-secondary`}>
        {job.progress}% · {new Date(job.createdAt).toLocaleTimeString()}
      </span>
    </button>
  );
}
