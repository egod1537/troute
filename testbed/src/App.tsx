import { useEffect, useMemo, useState } from "react";
import { Classes } from "@blueprintjs/core";
import { ApiError, checkHealth, runRoute, type RouteInput } from "./api";
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

  function updateJob(jobId: string, update: Partial<TestbedJob>) {
    setJobs((current) =>
      current.map((job) => (job.id === jobId ? { ...job, ...update } : job)),
    );
  }

  async function executeJob(request: RouteInput) {
    // Let React commit the pending row before the network lifecycle starts.
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    updateJob(request.job_id, {
      status: "running",
      stage: "optimizing",
      message: "POST /optimize is in progress.",
    });

    try {
      const result = await runRoute(request);
      updateJob(request.job_id, {
        status: "completed",
        completedAt: Date.now(),
        progress: 100,
        stage: "completed",
        message: "The optimize request completed.",
        response: result.response,
        route: result.route,
      });
    } catch (error) {
      updateJob(request.job_id, {
        status: "failed",
        completedAt: Date.now(),
        stage: "failed",
        message: "The optimize request failed.",
        error: (error as Error).message,
        response: error instanceof ApiError ? error.response : undefined,
      });
    }
  }

  function createJob(request: RouteInput) {
    const job: TestbedJob = {
      id: request.job_id,
      status: "pending",
      createdAt: Date.now(),
      progress: 0,
      stage: "queued",
      message: "Waiting to start the optimize request.",
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
        <JobDetail job={selectedJob} />
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
