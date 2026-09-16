import { NonIdealState } from "@blueprintjs/core";
import type { TestbedJob } from "../../jobs";
import { JobListItem } from "./JobListItem";

interface JobListProps {
  jobs: TestbedJob[];
  selectedJobId: string | null;
  onSelect: (jobId: string) => void;
}

export function JobList({ jobs, selectedJobId, onSelect }: JobListProps) {
  if (jobs.length === 0) {
    return (
      <NonIdealState
        className="job-list-empty"
        icon="inbox"
        title="아직 Job이 없습니다"
        description="새 Job을 생성해 최적화 요청을 실행하세요."
      />
    );
  }

  const orderedJobs = [...jobs].sort(
    (left, right) => right.createdAt - left.createdAt,
  );

  return (
    <div className="job-list" role="listbox" aria-label="테스트베드 Jobs">
      {orderedJobs.map((job) => (
        <JobListItem
          key={job.id}
          job={job}
          selected={job.id === selectedJobId}
          onSelect={() => onSelect(job.id)}
        />
      ))}
    </div>
  );
}
