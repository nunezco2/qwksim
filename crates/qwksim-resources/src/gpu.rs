//! GPU pool resource agent — integer-GPU-count allocations against
//! a per-site envelope (Q7.3 = α homogeneous count).
//!
//! Mirrors [`super::HpcPartitionAgent`] structurally: tracks
//! `(total_gpus, used_gpus)`, advertises the live `(total − used)`
//! delta, and accepts / releases [`Allocation`]s by `alloc.gpus`
//! (the `cores` field of the same allocation is owned by the HPC
//! partition agent and is ignored here).
//!
//! No GPU tiering today (Q7.3 = α): the pool is a single integer
//! count of identical accelerators. Heterogeneous tiers (β) would
//! land as a follow-up Phase-2 PR keyed off a separate
//! `Allocation::gpu_tier` enum.

use qwksim_core::event::{AgentId, SimTime};
use qwksim_scheduler::View;

use crate::{AdvertisedSummary, Allocation, ResourceAgent};

/// GPU pool resource agent. One per super-site in the headline
/// configuration; population is small (`k ≤ 32` in the Q7.6
/// envelopes) so a `u32` count is plenty.
#[derive(Debug, Clone, Copy)]
pub struct GpuPoolAgent {
    id: AgentId,
    total_gpus: u32,
    used_gpus: u32,
}

impl GpuPoolAgent {
    /// Build a pool agent with `total_gpus` GPUs configured and
    /// zero in use.
    ///
    /// # Panics
    /// Panics if `total_gpus == 0` — a zero-capacity GPU pool is
    /// pathological (any allocation request would land
    /// in-saturate and produce zero-utility outcomes).
    pub fn new(id: AgentId, total_gpus: u32) -> Self {
        assert!(total_gpus > 0, "GpuPoolAgent requires total_gpus > 0");
        Self {
            id,
            total_gpus,
            used_gpus: 0,
        }
    }

    /// Configured total GPU count.
    pub fn total_gpus(&self) -> u32 {
        self.total_gpus
    }

    /// GPUs currently allocated.
    pub fn used_gpus(&self) -> u32 {
        self.used_gpus
    }

    /// GPUs not currently allocated (computed live, not cached).
    pub fn true_free_gpus(&self) -> u32 {
        self.total_gpus - self.used_gpus
    }
}

impl ResourceAgent for GpuPoolAgent {
    fn id(&self) -> AgentId {
        self.id
    }

    fn advertised_summary(&self, _now: SimTime) -> AdvertisedSummary {
        AdvertisedSummary {
            free_gpus: self.true_free_gpus(),
            total_gpus: self.total_gpus,
            // The GPU pool does not own CPU cores; the HPC
            // partition agent populates those fields on its own
            // summary.
            ..Default::default()
        }
    }

    fn utility(&self, _alloc: &Allocation, _view: &View<'_>) -> f64 {
        // FLAG-C utility lands with the Nash-bargaining solver in
        // T4.x; today the stub returns a constant so the trait is
        // callable from downstream tests.
        1.0
    }

    fn accept(&mut self, alloc: Allocation, _now: SimTime) {
        // Defensive saturate-at-total: the bargaining solver
        // guarantees feasibility but a defensive bound keeps the
        // invariant `advertised.free_gpus ≤ true_free_gpus` from
        // breaking under buggy callers.
        let new = self.used_gpus.saturating_add(alloc.gpus);
        debug_assert!(
            new <= self.total_gpus,
            "over-committed GpuPoolAgent {id}: used {new} > total {total}",
            id = self.id,
            total = self.total_gpus
        );
        self.used_gpus = new.min(self.total_gpus);
    }

    fn release(&mut self, alloc: &Allocation, _now: SimTime) {
        self.used_gpus = self.used_gpus.saturating_sub(alloc.gpus);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwksim_scheduler::{AdvertisedState, GlobalState, LocalState};

    fn gpus(n: u32) -> Allocation {
        Allocation {
            gpus: n,
            ..Default::default()
        }
    }

    #[test]
    fn fresh_agent_reports_full_capacity_as_free() {
        let a = GpuPoolAgent::new(2, 8);
        let s = a.advertised_summary(0);
        assert_eq!(s.total_gpus, 8);
        assert_eq!(s.free_gpus, 8);
        assert_eq!(s.total_cores, 0, "GPU pool does not own cores");
        assert_eq!(s.free_cores, 0);
        assert_eq!(a.true_free_gpus(), 8);
    }

    #[test]
    fn advertised_summary_is_integer_valued_after_admit_and_release() {
        let mut a = GpuPoolAgent::new(2, 8);

        a.accept(gpus(2), 100);
        let s = a.advertised_summary(100);
        assert_eq!(s.free_gpus, 6);
        assert_eq!(a.true_free_gpus(), 6);

        a.accept(gpus(3), 110);
        let s = a.advertised_summary(110);
        assert_eq!(s.free_gpus, 3);
        assert_eq!(a.true_free_gpus(), 3);
        assert!(s.free_gpus <= a.true_free_gpus());

        a.release(&gpus(2), 200);
        let s = a.advertised_summary(200);
        assert_eq!(s.free_gpus, 5);
        assert_eq!(a.true_free_gpus(), 5);

        a.release(&gpus(3), 300);
        let s = a.advertised_summary(300);
        assert_eq!(s.free_gpus, 8);
        assert_eq!(a.used_gpus(), 0);
    }

    #[test]
    fn advertised_summary_never_exceeds_true_free_under_admit_release_churn() {
        let mut a = GpuPoolAgent::new(3, 32);

        let script: &[(bool, u32)] = &[
            (true, 4),
            (true, 8),
            (true, 1),
            (false, 4),
            (true, 16),
            (false, 8),
            (false, 1),
            (true, 1),
            (false, 16),
            (false, 1),
        ];

        for &(is_accept, n) in script {
            if is_accept {
                a.accept(gpus(n), 0);
            } else {
                a.release(&gpus(n), 0);
            }
            let s = a.advertised_summary(0);
            assert!(
                s.free_gpus <= a.true_free_gpus(),
                "advertised free_gpus ({}) exceeded true free ({}) after {} of {} GPUs",
                s.free_gpus,
                a.true_free_gpus(),
                if is_accept { "accept" } else { "release" },
                n
            );
            assert_eq!(
                s.free_gpus,
                a.total_gpus - a.used_gpus(),
                "advertised summary must mirror live (total - used)"
            );
        }
        assert_eq!(a.used_gpus(), 0);
    }

    #[test]
    fn release_of_more_than_used_saturates_at_zero() {
        let mut a = GpuPoolAgent::new(1, 4);
        a.accept(gpus(2), 0);
        a.release(&gpus(100), 0);
        assert_eq!(a.used_gpus(), 0);
        assert_eq!(a.advertised_summary(0).free_gpus, 4);
    }

    #[test]
    fn cores_field_of_allocation_is_ignored() {
        // A workflow that wants both cores and GPUs sends the same
        // Allocation to both agents; the GPU pool reads only
        // alloc.gpus.
        let mut a = GpuPoolAgent::new(1, 4);
        let mixed = Allocation { cores: 64, gpus: 2 };
        a.accept(mixed, 0);
        assert_eq!(a.used_gpus(), 2, "GPU pool only consumes alloc.gpus");
    }

    #[test]
    fn utility_runs_under_both_view_variants() {
        let a = GpuPoolAgent::new(1, 4);
        let g = GlobalState;
        let l = LocalState;
        let ad = AdvertisedState;

        let u_oracular = a.utility(&gpus(2), &View::Oracular(&g));
        let u_local = a.utility(
            &gpus(2),
            &View::Local {
                local: &l,
                advertised: &ad,
            },
        );

        assert!(u_oracular.is_finite());
        assert!(u_local.is_finite());
    }

    #[test]
    #[should_panic(expected = "total_gpus > 0")]
    fn zero_capacity_pool_rejects_in_constructor() {
        GpuPoolAgent::new(0, 0);
    }
}
