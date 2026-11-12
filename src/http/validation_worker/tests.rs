use super::*;
use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse},
    events::OptimizationEventReporter,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar, Mutex,
    },
    time::Duration,
};

struct GateExecutor {
    gate: Arc<(Mutex<bool>, Condvar)>,
    started: mpsc::Sender<()>,
    calls: AtomicUsize,
}

impl JobExecutor for GateExecutor {
    fn execute(
        &self,
        _: OptimizeRouteRequest,
        _: &dyn OptimizationEventReporter,
        _: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        panic!("validation must never execute optimization")
    }

    fn validate_route(
        &self,
        _: ValidateRouteRequest,
        _: &CancellationToken,
    ) -> Result<ValidateRouteResponse, OptimizationServiceError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.started.send(()).unwrap();
            let (gate, signal) = &*self.gate;
            let mut released = gate.lock().unwrap();
            while !*released {
                released = signal.wait(released).unwrap();
            }
        }
        Ok(ValidateRouteResponse {
            input_valid: true,
            schedule_feasible: Some(true),
            blocks_optimization: false,
            matrix_source: "supplied",
            issues: vec![],
            stops: vec![],
            feasibility: None,
        })
    }
}

fn request() -> ValidateRouteRequest {
    serde_json::from_value(serde_json::json!({"route":{"job_id":"queue", "locations":[
        {"id":"a","place_id":"a","open_time":"09:00","close_time":"18:00","stay_minutes":0},
        {"id":"b","place_id":"b","open_time":"09:00","close_time":"18:00","stay_minutes":0}
    ]},"check_feasibility":true}))
    .unwrap()
}

#[tokio::test]
async fn queue_is_bounded_and_cancelled_waiters_never_run() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, began) = mpsc::channel();
    let executor = Arc::new(GateExecutor {
        gate: gate.clone(),
        started,
        calls: AtomicUsize::new(0),
    });
    let worker = ValidationWorker::start(executor.clone(), None, 1);
    let first = worker
        .submit(request(), CancellationToken::new())
        .unwrap_or_else(|_| panic!("worker unavailable"));
    began.recv_timeout(Duration::from_secs(2)).unwrap();
    let cancellation = CancellationToken::new();
    let queued = worker
        .submit(request(), cancellation.clone())
        .unwrap_or_else(|_| panic!("queue should have one free slot"));
    assert!(matches!(
        worker.submit(request(), CancellationToken::new()),
        Err(QueueError::Full)
    ));
    cancellation.cancel();
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    first.await.unwrap().unwrap();
    assert!(queued.await.is_err());
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    worker
        .submit(request(), CancellationToken::new())
        .unwrap_or_else(|_| panic!("queue did not recover"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
}
