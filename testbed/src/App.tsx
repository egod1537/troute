import { useEffect, useMemo, useState } from "react";
import { Classes } from "@blueprintjs/core";
import {
  ApiError,
  cancelStoredJob,
  checkHealth,
  getRouteProviderPolicy,
  getStoredJob,
  getStoredTimeline,
  runRoute,
  subscribeToStoredJob,
  type RouteInput,
  type RouteProviderPolicyDiagnostics,
} from "./api";
import { AppHeader, type HealthState } from "./components/AppHeader";
import { JobDetail } from "./components/detail/JobDetail";
import { JobSidebar } from "./components/jobs/JobSidebar";
import { NewJobDialog } from "./components/jobs/NewJobDialog";
import { ProviderPolicyPanel } from "./components/ProviderPolicyPanel";
import {
  mergeStoredJob,
  mergeStoredJobEvent,
  type TestbedJob,
} from "./jobs";
import { type ThemeMode, useTheme } from "./theme";
import { useJobListRefresh } from "./useJobListRefresh";

interface AppProps {
  initialThemeMode?: ThemeMode;
}

export function App({ initialThemeMode }: AppProps) {
  const { mode: themeMode, resolvedTheme, selectMode } =
    useTheme(initialThemeMode);
  const [health, setHealth] = useState<HealthState>("checking");
  const [healthRefreshing, setHealthRefreshing] = useState(false);
  const [providerPolicy, setProviderPolicy] =
    useState<RouteProviderPolicyDiagnostics | null>(null);
  const [providerPolicyError, setProviderPolicyError] = useState("");
  const [jobs, setJobs] = useState<TestbedJob[]>([]);
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [newJobOpen, setNewJobOpen] = useState(false);
  const [cancellingJobId, setCancellingJobId] = useState<string | null>(null);
  const [cancelError, setCancelError] = useState("");
  const { refreshJobs, refreshingJobs, jobRefreshFailed } = useJobListRefresh({
    jobs,
    setJobs,
    setSelectedJobId,
  });
  const selectedJob =
    jobs.find((job) => job.id === selectedJobId) ?? null;

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
    void getRouteProviderPolicy()
      .then((policy) => {
        setProviderPolicy(policy);
        setProviderPolicyError("");
      })
      .catch((error: Error) =>
        setProviderPolicyError(`Provider policy 조회 실패: ${error.message}`),
      );
  }, []);

  useEffect(() => {
    if (!selectedJobId) return;
    setCancelError("");
    let disposed = false;
    let inFlight = false;
    let controller: AbortController | null = null;
    let closeEvents: (() => void) | undefined;
    const refreshSelectedJob = async () => {
      if (inFlight) return;
      inFlight = true;
      controller = new AbortController();
      try {
        const [record, timeline] = await Promise.all([
          getStoredJob(selectedJobId, controller.signal),
          getStoredTimeline(selectedJobId, controller.signal),
        ]);
        if (disposed) return;
        setJobs((current) =>
          current.map((job) =>
            job.id === selectedJobId
              ? { ...mergeStoredJob(record, job), timeline }
              : job,
          ),
        );
      } catch {
        // A browser-created Job may not be persisted yet. SSE reconnect and
        // list discovery recover without replacing the UI with an error.
      } finally {
        inFlight = false;
      }
    };
    void refreshSelectedJob();
    const active =
      selectedJob?.status === "pending" || selectedJob?.status === "running";
    if (active) {
      closeEvents = subscribeToStoredJob(
        selectedJobId,
        (event) => {
          if (disposed) return;
          setJobs((current) =>
            current.map((job) =>
              job.id === selectedJobId
                ? mergeStoredJobEvent(event, job)
                : job,
            ),
          );
          if (
            event.status === "completed" ||
            event.status === "failed" ||
            event.status === "cancelled"
          ) {
            void getStoredTimeline(selectedJobId)
              .then((timeline) => updateJob(selectedJobId, { timeline }))
              .catch(() => undefined);
          }
        },
        () => void refreshSelectedJob(),
      );
    }
    return () => {
      disposed = true;
      closeEvents?.();
      controller?.abort();
    };
  }, [selectedJobId, selectedJob?.status]);

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
      updatedAt: Date.now(),
      stage: "optimizing",
      message: "POST /optimize 실행 중.",
    });

    try {
      const result = await runRoute(request);
      updateActiveJob(request.job_id, {
        status: "completed",
        updatedAt: Date.now(),
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
        updatedAt: Date.now(),
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
        updatedAt: Date.now(),
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
      updatedAt: Date.now(),
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

      <ProviderPolicyPanel
        diagnostics={providerPolicy}
        error={providerPolicyError}
      />

      <main className="job-workspace">
        <JobSidebar
          jobs={jobs}
          selectedJobId={selectedJobId}
          refreshing={refreshingJobs}
          refreshFailed={jobRefreshFailed}
          onNewJob={() => setNewJobOpen(true)}
          onRefresh={() => void refreshJobs()}
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
