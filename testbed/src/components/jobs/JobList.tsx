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
        title="No jobs yet"
        description="Create a job to run an optimization request."
      />
    );
  }

  const orderedJobs = [...jobs].sort(
    (left, right) => right.createdAt - left.createdAt,
  );

  return (
    <div className="job-list" role="listbox" aria-label="Testbed jobs">
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
