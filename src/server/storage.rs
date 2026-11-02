use std::{error::Error, path::Path, sync::Arc};

use troute::{
    observation::JobObservationRecorder,
    storage::{FileJobStore, FileJobTimelineStore, JobStore},
};

/// Persistent server dependencies and the recovery result produced while they
/// are initialized.
pub(super) struct StorageRuntime {
    job_store: Arc<FileJobStore>,
    observation_recorder: JobObservationRecorder,
    recovered_jobs: usize,
}

impl StorageRuntime {
    pub(super) fn initialize(data_dir: &Path) -> Result<Self, Box<dyn Error>> {
        let job_store = Arc::new(FileJobStore::new(data_dir)?);
        let recovered_jobs = job_store.recover_interrupted()?;
        let observation_recorder =
            JobObservationRecorder::new(Arc::new(FileJobTimelineStore::new(data_dir)?));

        Ok(Self {
            job_store,
            observation_recorder,
            recovered_jobs,
        })
    }

    pub(super) fn job_store(&self) -> Arc<FileJobStore> {
        Arc::clone(&self.job_store)
    }

    pub(super) fn observation_recorder(&self) -> JobObservationRecorder {
        self.observation_recorder.clone()
    }

    pub(super) fn recovered_jobs(&self) -> usize {
        self.recovered_jobs
    }
}

#[cfg(test)]
mod tests;
