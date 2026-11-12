use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        mpsc::{self, SyncSender, TrySendError},
        Arc,
    },
};
use tokio::sync::oneshot;

use crate::{
    affinity::pin_current_thread,
    cancellation::CancellationToken,
    jobs::JobExecutor,
    service::OptimizationServiceError,
    solver::SolverError,
    validation::{ValidateRouteRequest, ValidateRouteResponse},
};

type Reply = Result<ValidateRouteResponse, OptimizationServiceError>;
struct Work {
    request: ValidateRouteRequest,
    cancellation: CancellationToken,
    reply: oneshot::Sender<Reply>,
}

#[derive(Clone)]
pub(super) struct ValidationWorker {
    sender: Option<SyncSender<Work>>,
}

pub(super) enum QueueError {
    Full,
    Unavailable,
}

impl ValidationWorker {
    pub(super) fn start(
        executor: Arc<dyn JobExecutor>,
        cpu: Option<usize>,
        capacity: usize,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Work>(capacity.max(1));
        let (ready, initialized) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("troute-validate".to_owned())
            .spawn(move || {
                if let Some(cpu) = cpu {
                    if let Err(error) = pin_current_thread(cpu) {
                        let _ = ready.send(Err(error));
                        return;
                    }
                }
                let _ = ready.send(Ok(()));
                while let Ok(work) = receiver.recv() {
                    if work.cancellation.is_cancelled() || work.reply.is_closed() {
                        continue;
                    }
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        executor.validate_route(work.request, &work.cancellation)
                    }))
                    .unwrap_or_else(|_| {
                        Err(OptimizationServiceError::Solver(SolverError::Failed(
                            "validation worker panicked".to_owned(),
                        )))
                    });
                    let _ = work.reply.send(result);
                }
            });
        if spawn.is_ok()
            && matches!(
                initialized.recv_timeout(std::time::Duration::from_secs(5)),
                Ok(Ok(()))
            )
        {
            Self {
                sender: Some(sender),
            }
        } else {
            crate::logging::logger().startup_warning(
                "dedicated fast-validation worker could not start; fast validation will return 503",
            );
            Self { sender: None }
        }
    }

    pub(super) fn submit(
        &self,
        request: ValidateRouteRequest,
        cancellation: CancellationToken,
    ) -> Result<oneshot::Receiver<Reply>, QueueError> {
        let sender = self.sender.as_ref().ok_or(QueueError::Unavailable)?;
        let (reply, result) = oneshot::channel();
        sender
            .try_send(Work {
                request,
                cancellation,
                reply,
            })
            .map_err(|error| match error {
                TrySendError::Full(_) => QueueError::Full,
                TrySendError::Disconnected(_) => QueueError::Unavailable,
            })?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
