import {
  type Dispatch,
  type SetStateAction,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { getStoredJob, listRecentJobs } from "./api";
import {
  mergeStoredJob,
  sortJobsNewestFirst,
  type TestbedJob,
} from "./jobs";

export const JOB_LIST_REFRESH_INTERVAL_MS = 2_000;

interface JobListRefreshOptions {
  jobs: TestbedJob[];
  setJobs: Dispatch<SetStateAction<TestbedJob[]>>;
  setSelectedJobId: Dispatch<SetStateAction<string | null>>;
}

export function useJobListRefresh({
  jobs,
  setJobs,
  setSelectedJobId,
}: JobListRefreshOptions) {
  const jobsRef = useRef(jobs);
  const requestRef = useRef<Promise<void> | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const mountedRef = useRef(true);
  const [refreshingJobs, setRefreshingJobs] = useState(false);
  const [jobRefreshFailed, setJobRefreshFailed] = useState(false);

  useEffect(() => {
    jobsRef.current = jobs;
  }, [jobs]);

  const refreshJobs = useCallback((): Promise<void> => {
    if (requestRef.current) return requestRef.current;

    const controller = new AbortController();
    abortRef.current = controller;
    setRefreshingJobs(true);
    const request = (async () => {
      try {
        const summaries = await listRecentJobs(50, controller.signal);
        const existingById = new Map(
          jobsRef.current.map((job) => [job.id, job]),
        );
        const changed = summaries.filter(
          (summary) =>
            existingById.get(summary.job_id)?.serverUpdatedAt !==
            summary.updated_at,
        );
        const records = await Promise.all(
          changed.map((summary) =>
            getStoredJob(summary.job_id, controller.signal),
          ),
        );
        if (controller.signal.aborted || !mountedRef.current) return;

        const recordsById = new Map(
          records.map((record) => [record.state.job_id, record]),
        );
        const serverIds = new Set(summaries.map((summary) => summary.job_id));
        const mergedServerJobs = summaries.flatMap((summary) => {
          const existing = existingById.get(summary.job_id);
          const record = recordsById.get(summary.job_id);
          if (record) return [mergeStoredJob(record, existing)];
          return existing ? [existing] : [];
        });
        const browserOnlyJobs = jobsRef.current.filter(
          (job) => job.serverUpdatedAt === undefined && !serverIds.has(job.id),
        );
        const merged = sortJobsNewestFirst([
          ...mergedServerJobs,
          ...browserOnlyJobs,
        ]);
        jobsRef.current = merged;
        setJobs(merged);
        setSelectedJobId((selected) =>
          selected && merged.some((job) => job.id === selected)
            ? selected
            : (merged[0]?.id ?? null),
        );
        setJobRefreshFailed(false);
      } catch (error) {
        if ((error as Error).name !== "AbortError" && mountedRef.current) {
          setJobRefreshFailed(true);
        }
      }
    })();
    requestRef.current = request;
    void request.finally(() => {
      if (requestRef.current !== request) return;
      requestRef.current = null;
      abortRef.current = null;
      if (mountedRef.current) setRefreshingJobs(false);
    });
    return request;
  }, [setJobs, setSelectedJobId]);

  useEffect(() => {
    mountedRef.current = true;
    void refreshJobs();
    const interval = window.setInterval(() => {
      if (document.visibilityState !== "hidden") void refreshJobs();
    }, JOB_LIST_REFRESH_INTERVAL_MS);
    const onVisibilityChange = () => {
      if (document.visibilityState === "visible") void refreshJobs();
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      mountedRef.current = false;
      window.clearInterval(interval);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      abortRef.current?.abort();
    };
  }, [refreshJobs]);

  return { refreshJobs, refreshingJobs, jobRefreshFailed };
}
