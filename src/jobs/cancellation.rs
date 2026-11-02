use super::*;

#[derive(Clone, Default)]
pub(super) struct JobCancellationRegistry {
    jobs: Arc<Mutex<HashMap<String, JobActivity>>>,
}

impl JobCancellationRegistry {
    pub(super) fn insert(&self, job_id: &str, activity: JobActivity) {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(job_id.to_owned(), activity);
    }

    pub(super) fn get(&self, job_id: &str) -> Option<JobActivity> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(job_id)
            .cloned()
    }

    pub(super) fn remove(&self, job_id: &str) {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(job_id);
    }
}
