import { Button, Card, Classes, Divider } from "@blueprintjs/core";
import type { TestbedJob } from "../../jobs";
import { JobList } from "./JobList";

interface JobSidebarProps {
  jobs: TestbedJob[];
  selectedJobId: string | null;
  refreshing: boolean;
  refreshFailed: boolean;
  onNewJob: () => void;
  onRefresh: () => void;
  onSelect: (jobId: string) => void;
}

export function JobSidebar({
  jobs,
  selectedJobId,
  refreshing,
  refreshFailed,
  onNewJob,
  onRefresh,
  onSelect,
}: JobSidebarProps) {
  const refreshLabel = refreshing
    ? "Job 목록 새로고침 중"
    : refreshFailed
      ? "Job 목록 새로고침 실패. 다시 시도"
      : "Job 목록 새로고침";
  return (
    <Card className="job-sidebar" elevation={1} compact>
      <div className="sidebar-heading">
        <h1 className={Classes.HEADING}>Jobs</h1>
        <div className="sidebar-actions">
          <Button
            className={`jobs-refresh-button${refreshing ? " is-refreshing" : ""}`}
            icon="refresh"
            intent={refreshFailed ? "warning" : "none"}
            minimal
            small
            disabled={refreshing}
            aria-label={refreshLabel}
            title={refreshLabel}
            onClick={onRefresh}
          />
          <Button icon="plus" intent="primary" onClick={onNewJob}>
            새 Job
          </Button>
        </div>
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
