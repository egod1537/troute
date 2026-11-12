use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::solver::{
    ObjectiveEvaluator, SolutionMetrics, SolverCandidate, SolverProgressEvent,
    SolverProgressObserver, SolverSolution,
};

const PREVIEW_WINDOW: Duration = Duration::from_secs(1);

type Preview = (SolverSolution, SolutionMetrics);

struct Window {
    started: Instant,
    candidate: Option<Preview>,
    last_order: Option<Vec<usize>>,
}

/// Global incumbent tracking and map sampling deliberately have separate state.
/// The solver's accumulator/selector remain authoritative for final selection.
pub(super) struct PreviewObserver<'a, E> {
    evaluator: &'a E,
    observer: &'a dyn SolverProgressObserver,
    best: Mutex<Option<SolutionMetrics>>,
    window: Mutex<Window>,
}

impl<'a, E: ObjectiveEvaluator> PreviewObserver<'a, E> {
    pub(super) fn new(evaluator: &'a E, observer: &'a dyn SolverProgressObserver) -> Self {
        Self {
            evaluator,
            observer,
            best: Mutex::new(None),
            window: Mutex::new(Window {
                started: Instant::now(),
                candidate: None,
                last_order: None,
            }),
        }
    }

    pub(super) fn flush(&self) {
        let mut window = self
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((solution, metrics)) = window.candidate.take() {
            window.last_order = Some(solution.visit_order.clone());
            self.observer.on_preview_solution(&solution, &metrics);
        }
    }
}

impl<E: ObjectiveEvaluator> SolverProgressObserver for PreviewObserver<'_, E> {
    fn on_progress(&self, event: SolverProgressEvent) {
        self.observer.on_progress(event);
    }

    fn is_enabled(&self) -> bool {
        self.observer.is_enabled()
    }

    fn on_search_sample(&self, solution: &SolverSolution, metrics: &SolutionMetrics) {
        // Accepted SA trajectory samples are already bounded by the run. Keep
        // them even when an entire run finishes inside one wall-clock window.
        self.observer.on_search_sample(solution, metrics);
    }

    fn on_candidate(&self, candidate: &SolverCandidate) {
        if !self.is_enabled() || !candidate.feasible {
            return;
        }
        let (Some(solution), Some(metrics)) = (&candidate.route, candidate.objective_score) else {
            return;
        };
        {
            let mut best = self
                .best
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if best
                .as_ref()
                .is_none_or(|current| self.evaluator.compare(&metrics, current).is_lt())
            {
                *best = Some(metrics);
                self.observer.on_best_solution(solution, &metrics);
            }
        }
        let mut window = self
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if window.started.elapsed() >= PREVIEW_WINDOW {
            if let Some((previous, previous_metrics)) = window.candidate.take() {
                window.last_order = Some(previous.visit_order.clone());
                self.observer
                    .on_preview_solution(&previous, &previous_metrics);
            }
            window.started = Instant::now();
        }
        // Repeating the same map path adds no visible search information.
        if window.last_order.as_ref() == Some(&solution.visit_order) {
            return;
        }
        if window
            .candidate
            .as_ref()
            .is_none_or(|(_, current)| self.evaluator.compare(&metrics, current).is_lt())
        {
            window.candidate = Some((solution.clone(), metrics));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::OptimizeRouteRequest,
        cancellation::CancellationToken,
        domain::OptimizationProblem,
        matrix::TravelTimeMatrix,
        solver::{DefaultObjectivePolicy, SolverCandidateMetadata},
    };

    #[derive(Default)]
    struct Recorder {
        best: Mutex<Vec<Vec<usize>>>,
        previews: Mutex<Vec<Vec<usize>>>,
    }
    impl SolverProgressObserver for Recorder {
        fn on_progress(&self, _: SolverProgressEvent) {}
        fn on_best_solution(&self, solution: &SolverSolution, _: &SolutionMetrics) {
            self.best.lock().unwrap().push(solution.visit_order.clone());
        }
        fn on_preview_solution(&self, solution: &SolverSolution, _: &SolutionMetrics) {
            self.previews
                .lock()
                .unwrap()
                .push(solution.visit_order.clone());
        }
    }

    #[test]
    fn window_can_show_a_worse_distinct_route_without_regressing_the_global_incumbent() {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "preview-window",
            "locations": (0..5).map(|i| serde_json::json!({"id": i.to_string(), "place_id": i.to_string(), "open_time": "00:00", "close_time": "23:50", "stay_minutes": 0})).collect::<Vec<_>>(),
            "start_policy": "FIXED", "start_time": "09:00"
        })).unwrap();
        let problem: OptimizationProblem = request.try_into().unwrap();
        let matrix = TravelTimeMatrix::new(
            (0..5)
                .map(|from| {
                    (0..5)
                        .map(|to| {
                            if from == to {
                                0
                            } else if to == from + 1 {
                                10
                            } else {
                                40
                            }
                        })
                        .collect()
                })
                .collect(),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let input = crate::solver::SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &cancellation,
        };
        let evaluator = DefaultObjectivePolicy;
        let recorder = Recorder::default();
        let observer = PreviewObserver::new(&evaluator, &recorder);
        let candidate = |order: Vec<usize>| {
            let solution = SolverSolution { visit_order: order };
            let metrics = evaluator.evaluate(input, &solution).unwrap();
            SolverCandidate {
                strategy: "sample".to_owned(),
                route: Some(solution),
                feasible: true,
                objective_score: Some(metrics),
                elapsed: Duration::ZERO,
                metadata: SolverCandidateMetadata::default(),
            }
        };
        let best = candidate(vec![0, 1, 2, 3, 4]);
        let worse = candidate(vec![0, 2, 1, 3, 4]);
        let worst = candidate(vec![0, 3, 2, 1, 4]);
        observer.on_candidate(&best);
        observer.flush();
        observer.on_candidate(&best); // The already shown map must not suppress new routes.
        observer.on_candidate(&worst);
        observer.on_candidate(&worse);
        observer.flush();
        assert_eq!(*recorder.best.lock().unwrap(), vec![vec![0, 1, 2, 3, 4]]);
        assert_eq!(
            *recorder.previews.lock().unwrap(),
            vec![vec![0, 1, 2, 3, 4], vec![0, 2, 1, 3, 4]]
        );
        observer.on_candidate(&worse);
        observer.flush();
        assert_eq!(
            recorder.previews.lock().unwrap().len(),
            2,
            "identical map paths must not generate another frame"
        );
    }
}
