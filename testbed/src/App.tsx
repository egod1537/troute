import { useEffect, useMemo, useState } from "react";
import { Classes } from "@blueprintjs/core";
import {
  ApiError,
  cancelStoredJob,
  checkHealth,
  getStoredJob,
  getStoredTimeline,
  listRecentJobs,
  runRoute,
  type RouteInput,
  type StoredJobRecord,
} from "./api";
import { AppHeader, type HealthState } from "./components/AppHeader";
import { JobDetail } from "./components/detail/JobDetail";
import { JobSidebar } from "./components/jobs/JobSidebar";
import { NewJobDialog } from "./components/jobs/NewJobDialog";
import type { TestbedJob } from "./jobs";
import { type ThemeMode, useTheme } from "./theme";

interface AppProps {
  initialThemeMode?: ThemeMode;
}

export function App({ initialThemeMode }: AppProps) {
  const { mode: themeMode, resolvedTheme, selectMode } =
    useTheme(initialThemeMode);
  const [health, setHealth] = useState<HealthState>("checking");
  const [healthRefreshing, setHealthRefreshing] = useState(false);
  const [jobs, setJobs] = useState<TestbedJob[]>([]);
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [newJobOpen, setNewJobOpen] = useState(false);
  const [cancellingJobId, setCancellingJobId] = useState<string | null>(null);
  const [cancelError, setCancelError] = useState("");

  async function refreshHealth() {
    setHealthRefreshing(true);
    setHealth("checking");
    try {
      await checkHealth();
      setHealth("online");
    } catch {
      setHealth("offline");
    } finally {
      setHealthRefreshing(false);
    }
  }

  useEffect(() => {
    void refreshHealth();
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const summaries = await listRecentJobs();
        const records = await Promise.all(
          summaries.map((summary) => getStoredJob(summary.job_id)),
        );
        if (cancelled) return;
        const restored = records.map(jobFromStoredRecord);
        setJobs((current) => {
          const currentIds = new Set(current.map((job) => job.id));
          return [
            ...current,
            ...restored.filter((job) => !currentIds.has(job.id)),
          ].sort((left, right) => right.createdAt - left.createdAt);
        });
        setSelectedJobId((current) => current ?? restored[0]?.id ?? null);
      } catch {
        // History is an integration aid; an unavailable history endpoint must
        // not prevent health checks or new optimize requests.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!selectedJobId) return;
    setCancelError("");
    let cancelled = false;
    void getStoredTimeline(selectedJobId)
      .then((timeline) => {
        if (!cancelled) updateJob(selectedJobId, { timeline });
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [selectedJobId]);

  function updateJob(jobId: string, update: Partial<TestbedJob>) {
    setJobs((current) =>
      current.map((job) => (job.id === jobId ? { ...job, ...update } : job)),
    );
  }

  function updateActiveJob(jobId: string, update: Partial<TestbedJob>) {
    setJobs((current) =>
      current.map((job) =>
        job.id === jobId && job.status !== "cancelled"
          ? { ...job, ...update }
          : job,
      ),
    );
  }

  async function executeJob(request: RouteInput) {
    // Let React commit the pending row before the network lifecycle starts.
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    updateActiveJob(request.job_id, {
      status: "running",
      stage: "optimizing",
      message: "POST /optimize 실행 중.",
    });

    try {
      const result = await runRoute(request);
      updateActiveJob(request.job_id, {
        status: "completed",
        completedAt: Date.now(),
        progress: 100,
        stage: "completed",
        message: "최적화 요청 완료.",
        response: result.response,
        route: result.route,
      });
    } catch (error) {
      updateActiveJob(request.job_id, {
        status: "failed",
        completedAt: Date.now(),
        stage: "failed",
        message: "최적화 요청 실패.",
        error: (error as Error).message,
        response: error instanceof ApiError ? error.response : undefined,
      });
    } finally {
      void getStoredTimeline(request.job_id)
        .then((timeline) => updateJob(request.job_id, { timeline }))
        .catch(() => undefined);
    }
  }

  async function cancelJob(jobId: string) {
    setCancellingJobId(jobId);
    setCancelError("");
    try {
      await cancelStoredJob(jobId);
      updateJob(jobId, {
        status: "cancelled",
        completedAt: Date.now(),
        message: "요청에 의해 Job이 종료되었습니다.",
        error: undefined,
      });
      const timeline = await getStoredTimeline(jobId).catch(() => undefined);
      if (timeline) updateJob(jobId, { timeline });
    } catch (error) {
      setCancelError((error as Error).message);
      throw error;
    } finally {
      setCancellingJobId(null);
    }
  }

  function createJob(request: RouteInput) {
    const job: TestbedJob = {
      id: request.job_id,
      status: "pending",
      createdAt: Date.now(),
      progress: 0,
      stage: "queued",
      message: "최적화 요청 실행 대기 중.",
      request,
      timeline: [],
    };

    setJobs((current) => [job, ...current]);
    setSelectedJobId(job.id);
    setNewJobOpen(false);
    void executeJob(request);
  }

  const selectedJob =
    jobs.find((job) => job.id === selectedJobId) ?? null;
  const existingJobIds = useMemo(
    () => new Set(jobs.map((job) => job.id)),
    [jobs],
  );

  return (
    <div
      className={`app-shell ${resolvedTheme === "dark" ? Classes.DARK : ""}`}
      data-theme={resolvedTheme}
    >
      <AppHeader
        health={health}
        healthRefreshing={healthRefreshing}
        themeMode={themeMode}
        onRefreshHealth={() => void refreshHealth()}
        onThemeChange={selectMode}
      />

      <main className="job-workspace">
        <JobSidebar
          jobs={jobs}
          selectedJobId={selectedJobId}
          onNewJob={() => setNewJobOpen(true)}
          onSelect={setSelectedJobId}
        />
        <JobDetail
          job={selectedJob}
          dark={resolvedTheme === "dark"}
          cancelling={cancellingJobId === selectedJob?.id}
          cancelError={cancelError}
          onCancel={cancelJob}
        />
      </main>

      <NewJobDialog
        isOpen={newJobOpen}
        dark={resolvedTheme === "dark"}
        existingJobIds={existingJobIds}
        onClose={() => setNewJobOpen(false)}
        onCreate={createJob}
      />
    </div>
  );
}

function jobFromStoredRecord(record: StoredJobRecord): TestbedJob {
  const error = record.error
    ? `${record.error.code}: ${record.error.message}${record.error.detail ? ` (${record.error.detail})` : ""}`
    : undefined;
  return {
    id: record.state.job_id,
    status: record.state.status,
    createdAt: record.state.created_at,
    completedAt: record.state.completed_at ?? undefined,
    progress: record.state.progress,
    stage: record.state.stage ?? undefined,
    message: record.state.last_message ?? undefined,
    error,
    request: record.request,
    timeline: [],
    route: record.result ?? undefined,
  };
}
