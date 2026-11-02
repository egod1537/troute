use super::diagnostics::SaInitializer;

/// Replaceable ordering policy for SA initializers. The default guarantees the
/// four baseline sources first, then interleaves warm starts and deterministic
/// restarts.
pub trait InitializerSelectionPolicy: Send + Sync {
    fn select(&self, run_index: u64, has_global_best: bool) -> SaInitializer;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RoundRobinInitializerSelectionPolicy;

impl InitializerSelectionPolicy for RoundRobinInitializerSelectionPolicy {
    fn select(&self, run_index: u64, has_global_best: bool) -> SaInitializer {
        const BASELINES: [SaInitializer; 4] = [
            SaInitializer::Greedy,
            SaInitializer::MstDoubleTree,
            SaInitializer::Christofides,
            SaInitializer::Clustered,
        ];
        if run_index < BASELINES.len() as u64 {
            return BASELINES[run_index as usize];
        }
        let restart = (run_index - BASELINES.len() as u64) % 5;
        if restart == 0 && has_global_best {
            SaInitializer::GlobalBest
        } else {
            BASELINES[restart.saturating_sub(1) as usize % BASELINES.len()]
        }
    }
}

pub fn deterministic_sa_seed(base_seed: u64, initializer: SaInitializer, round: u64) -> u64 {
    let tag = match initializer {
        SaInitializer::Greedy => 1_u64,
        SaInitializer::MstDoubleTree => 2,
        SaInitializer::Christofides => 3,
        SaInitializer::Clustered => 4,
        SaInitializer::GlobalBest => 5,
    };
    let mut value = base_seed ^ tag.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ round.rotate_left(17);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
