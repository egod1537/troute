import { Button, Card, Classes, Divider } from "@blueprintjs/core";
import type { TestbedJob } from "../../jobs";
import { JobList } from "./JobList";

interface JobSidebarProps {
  jobs: TestbedJob[];
  selectedJobId: string | null;
  onNewJob: () => void;
  onSelect: (jobId: string) => void;
}

export function JobSidebar({
  jobs,
  selectedJobId,
  onNewJob,
  onSelect,
}: JobSidebarProps) {
  return (
    <Card className="job-sidebar" elevation={1} compact>
      <div className="sidebar-heading">
        <h1 className={Classes.HEADING}>Jobs</h1>
        <Button icon="plus" intent="primary" onClick={onNewJob}>
          새 Job
        </Button>
      </div>
      <Divider />
      <JobList
        jobs={jobs}
        selectedJobId={selectedJobId}
        onSelect={onSelect}
      />
    </Card>
  );
}
