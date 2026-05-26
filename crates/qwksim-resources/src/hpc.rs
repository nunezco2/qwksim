//! HPC partition agent (single-partition skeleton).
//!
//! Tracks `(total_cores, used_cores)`. Per **T1.6** the agent is
//! deliberately minimal: no GPU, no scratch, no network, no memory
//! bandwidth, no τ_adv staleness. Each of those lands in a later
//! Phase-2 PR.

use qwksim_core::event::{AgentId, SimTime};
use qwksim_scheduler::View;

use crate::{AdvertisedSummary, Allocation, ResourceAgent};

/// HPC partition resource agent. One per super-site in the
/// headline configuration.
#[derive(Debug, Clone, Copy)]
pub struct HpcPartitionAgent {
    id: AgentId,
    total_cores: u32,
    used_cores: u32,
}

impl HpcPartitionAgent {
    /// Build a partition agent with `total_cores` cores configured
    /// and zero in use.
    ///
    /// # Panics
    /// Panics if `total_cores == 0` — a zero-capacity partition is
    /// pathological in every headline scenario and would silently
    /// produce zero-utility allocations.
    pub fn new(id: AgentId, total_cores: u32) -> Self {
        assert!(
            total_cores > 0,
            "HpcPartitionAgent requires total_cores > 0"
        );
        Self {
            id,
            total_cores,
            used_cores: 0,
        }
    }

    /// Configured total core count.
    pub fn total_cores(&self) -> u32 {
        self.total_cores
    }

    /// Cores currently allocated.
    pub fn used_cores(&self) -> u32 {
        self.used_cores
    }

    /// Cores not currently allocated (computed live, not cached).
    pub fn true_free_cores(&self) -> u32 {
        self.total_cores - self.used_cores
    }
}

impl ResourceAgent for HpcPartitionAgent {
    fn id(&self) -> AgentId {
        self.id
    }

    fn advertised_summary(&self, _now: SimTime) -> AdvertisedSummary {
        AdvertisedSummary {
            free_cores: self.true_free_cores(),
            total_cores: self.total_cores,
            // HPC partition does not own GPUs; the GPU pool agent
            // populates those fields on its own summary.
            ..Default::default()
        }
    }

    fn utility(&self, _alloc: &Allocation, _view: &View<'_>) -> f64 {
        // FLAG-C utility formula (`α·deadline_slack/deadline
        // + β·realised_fidelity + γ·(1 − rivalry)
        // − δ·degradation_penalty`) lands with the Nash-bargaining
        // solver in T4.x; today the stub returns a constant so the
        // trait is callable from downstream tests.
        1.0
    }

    fn accept(&mut self, alloc: Allocation, _now: SimTime) {
        // Defensive: the bargaining solver guarantees feasibility,
        // but saturating at `total_cores` keeps the invariant
        // `advertised.free_cores ≤ true_free_cores` from breaking
        // under buggy callers.
        let new = self.used_cores.saturating_add(alloc.cores);
        debug_assert!(
            new <= self.total_cores,
            "over-committed HpcPartitionAgent {id}: used {new} > total {total}",
            id = self.id,
            total = self.total_cores
        );
        self.used_cores = new.min(self.total_cores);
    }

    fn release(&mut self, alloc: &Allocation, _now: SimTime) {
        self.used_cores = self.used_cores.saturating_sub(alloc.cores);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwksim_scheduler::{AdvertisedState, GlobalState, LocalState};

    fn alloc(cores: u32) -> Allocation {
        Allocation {
            cores,
            ..Default::default()
        }
    }

    #[test]
    fn fresh_agent_reports_full_capacity_as_free() {
        let a = HpcPartitionAgent::new(1, 64);
        let s = a.advertised_summary(0);
        assert_eq!(s.total_cores, 64);
        assert_eq!(s.free_cores, 64);
        assert_eq!(a.true_free_cores(), 64);
    }

    #[test]
    fn accept_then_release_preserves_capacity_and_advertised_summary() {
        let mut a = HpcPartitionAgent::new(1, 64);

        a.accept(alloc(10), 100);
        let s = a.advertised_summary(100);
        assert_eq!(s.free_cores, 54);
        assert_eq!(a.true_free_cores(), 54);
        assert!(
            s.free_cores <= a.true_free_cores(),
            "advertised summary must never exceed true free capacity"
        );

        a.accept(alloc(20), 110);
        let s = a.advertised_summary(110);
        assert_eq!(s.free_cores, 34);
        assert!(s.free_cores <= a.true_free_cores());

        a.release(&alloc(10), 200);
        let s = a.advertised_summary(200);
        assert_eq!(s.free_cores, 44);
        assert!(s.free_cores <= a.true_free_cores());

        a.release(&alloc(20), 300);
        let s = a.advertised_summary(300);
        assert_eq!(s.free_cores, 64);
        assert!(s.free_cores <= a.true_free_cores());
        assert_eq!(a.used_cores(), 0);
    }

    #[test]
    fn advertised_summary_never_exceeds_true_free_under_admit_release_churn() {
        let mut a = HpcPartitionAgent::new(7, 256);

        // Hand-crafted churn: a mix of accept and release sizes
        // that exercises the saturate-on-release path too.
        let script: &[(bool, u32)] = &[
            (true, 32),
            (true, 64),
            (true, 8),
            (false, 32),
            (true, 100),
            (false, 64),
            (false, 8),
            (true, 1),
            (false, 100),
            (false, 1),
        ];

        for &(is_accept, cores) in script {
            if is_accept {
                a.accept(alloc(cores), 0);
            } else {
                a.release(&alloc(cores), 0);
            }
            let s = a.advertised_summary(0);
            assert!(
                s.free_cores <= a.true_free_cores(),
                "advertised free_cores ({}) exceeded true free ({}) after {} of {} cores",
                s.free_cores,
                a.true_free_cores(),
                if is_accept { "accept" } else { "release" },
                cores
            );
            assert_eq!(
                s.free_cores,
                a.total_cores - a.used_cores(),
                "advertised summary must mirror live (total - used)"
            );
        }
        // After all releases, used_cores returns to zero.
        assert_eq!(a.used_cores(), 0);
    }

    #[test]
    fn release_of_more_than_used_saturates_at_zero() {
        // The bargaining solver guarantees release-matches-accept,
        // but a defensive saturate-sub keeps the invariant
        // unbreakable under buggy callers.
        let mut a = HpcPartitionAgent::new(1, 16);
        a.accept(alloc(4), 0);
        a.release(&alloc(100), 0);
        assert_eq!(a.used_cores(), 0);
        assert_eq!(a.advertised_summary(0).free_cores, 16);
    }

    #[test]
    fn utility_runs_under_both_view_variants() {
        let a = HpcPartitionAgent::new(1, 16);
        let g = GlobalState;
        let l = LocalState;
        let ad = AdvertisedState;

        let u_oracular = a.utility(&alloc(4), &View::Oracular(&g));
        let u_local = a.utility(
            &alloc(4),
            &View::Local {
                local: &l,
                advertised: &ad,
            },
        );

        // Stub utility is a constant; what matters is that both
        // calls type-check and produce a finite value.
        assert!(u_oracular.is_finite());
        assert!(u_local.is_finite());
    }

    #[test]
    #[should_panic(expected = "total_cores > 0")]
    fn zero_capacity_partition_rejects_in_constructor() {
        HpcPartitionAgent::new(0, 0);
    }
}
