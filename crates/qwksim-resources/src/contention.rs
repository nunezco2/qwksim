//! `ResourceContentionView` — the **shared-constraint rivalry**
//! aggregate consumed by every per-resource agent's utility
//! function (FLAG-J closed).
//!
//! Each fluid pool (network, scratch I/O, …) emits a scalar
//! `rivalry ∈ [0, 1]`. The bargaining utility (FLAG-C) wants a
//! single composite term to drop into its
//! `γ · (1 − rivalry)` slot. Today there are two contributors —
//! [`FluidBandwidthPool`] (intra-site network) and
//! [`ScratchIoPool`] — but the type is open: future fluid pools
//! (e.g. NUMA memory bandwidth aggregates) compose via the same
//! `with_*` builder pattern.
//!
//! Aggregation rule: **noisy-OR**
//!
//! ```text
//! total_rivalry = 1 − ∏ (1 − r_i)
//! ```
//!
//! Properties (all checked by unit tests):
//!
//! - Range: `total_rivalry ∈ [0, 1]` whenever every `r_i ∈ [0, 1]`.
//! - Monotonicity: increasing any `r_i` weakly increases the
//!   total.
//! - Saturation: if any `r_i == 1` the total is `1`; if every
//!   `r_i == 0` the total is `0`.
//! - Commutativity: order of `with_*` builder calls does not
//!   affect the result.
//!
//! FLAG-J is closed on the in-utility shared-constraint
//! formulation — see [`plan/decisions/flag-j.md`] — so this view
//! is a *value type* (no agent of its own; not in the Nash
//! product population).

use crate::{FluidBandwidthPool, ScratchIoPool};

/// Snapshot of per-resource rivalry, aggregated for the
/// bargaining utility's shared-constraint term.
///
/// Cheap to copy; constructed at every bargaining round from the
/// current state of each contributing pool via [`snapshot`] or
/// piecemeal via the `with_*` builders.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ResourceContentionView {
    network_rivalry: f64,
    scratch_rivalry: f64,
}

impl ResourceContentionView {
    /// All-zero view — every contributor is idle. Equivalent to
    /// `Default::default()` but spells the intent at call sites.
    pub fn idle() -> Self {
        Self::default()
    }

    /// Build a view from the live rivalry of each contributing
    /// pool. Equivalent to chaining the `with_*` builders.
    pub fn snapshot(network: &FluidBandwidthPool, scratch: &ScratchIoPool) -> Self {
        Self::idle()
            .with_network_rivalry(network.rivalry())
            .with_scratch_rivalry(scratch.rivalry())
    }

    /// Set the network rivalry contribution. `r` is clamped to
    /// `[0, 1]` to keep the noisy-OR aggregate well-defined under
    /// future pool implementations that might emit slightly
    /// out-of-range values due to f64 round-off.
    pub fn with_network_rivalry(mut self, r: f64) -> Self {
        self.network_rivalry = clamp_01(r);
        self
    }

    /// Set the scratch-I/O rivalry contribution.
    pub fn with_scratch_rivalry(mut self, r: f64) -> Self {
        self.scratch_rivalry = clamp_01(r);
        self
    }

    /// Current network rivalry (`∈ [0, 1]`).
    pub fn network_rivalry(&self) -> f64 {
        self.network_rivalry
    }

    /// Current scratch-I/O rivalry (`∈ [0, 1]`).
    pub fn scratch_rivalry(&self) -> f64 {
        self.scratch_rivalry
    }

    /// Aggregate shared-constraint rivalry term — what FLAG-C's
    /// utility consumes. Computed as
    /// `1 − (1 − network_rivalry) · (1 − scratch_rivalry)`
    /// (noisy-OR).
    pub fn total_rivalry(&self) -> f64 {
        let r_net = self.network_rivalry;
        let r_scratch = self.scratch_rivalry;
        let complement = (1.0 - r_net) * (1.0 - r_scratch);
        // Floating-point round-off may push the result a hair
        // outside [0, 1]; clamp before returning so downstream
        // consumers can assume a clean range without their own
        // clamp.
        clamp_01(1.0 - complement)
    }
}

fn clamp_01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn idle_view_has_zero_rivalry_everywhere() {
        let v = ResourceContentionView::idle();
        assert_eq!(v.network_rivalry(), 0.0);
        assert_eq!(v.scratch_rivalry(), 0.0);
        assert_eq!(v.total_rivalry(), 0.0);
    }

    #[test]
    fn out_of_range_inputs_are_clamped_to_unit_interval() {
        let v = ResourceContentionView::idle()
            .with_network_rivalry(-0.5)
            .with_scratch_rivalry(1.2);
        assert_eq!(v.network_rivalry(), 0.0);
        assert_eq!(v.scratch_rivalry(), 1.0);
        // scratch is saturated → noisy-OR pegs the total at 1.
        assert!(approx_eq(v.total_rivalry(), 1.0));
    }

    #[test]
    fn total_rivalry_obeys_noisy_or_formula() {
        // 0.3, 0.4 → 1 - (1 - 0.3)(1 - 0.4) = 1 - 0.42 = 0.58
        let v = ResourceContentionView::idle()
            .with_network_rivalry(0.3)
            .with_scratch_rivalry(0.4);
        assert!(approx_eq(v.total_rivalry(), 0.58));
    }

    #[test]
    fn either_input_at_one_saturates_total_to_one() {
        for (net, scr) in [(1.0, 0.0), (0.0, 1.0), (1.0, 0.7), (0.4, 1.0)] {
            let v = ResourceContentionView::idle()
                .with_network_rivalry(net)
                .with_scratch_rivalry(scr);
            assert!(
                approx_eq(v.total_rivalry(), 1.0),
                "(net={net}, scr={scr}) total = {}",
                v.total_rivalry()
            );
        }
    }

    #[test]
    fn both_inputs_at_zero_yield_zero_total() {
        let v = ResourceContentionView::idle()
            .with_network_rivalry(0.0)
            .with_scratch_rivalry(0.0);
        assert_eq!(v.total_rivalry(), 0.0);
    }

    #[test]
    fn total_rivalry_stays_in_unit_interval_across_a_random_sweep() {
        // Hand-crafted lattice over [0, 1]^2 — no PRNG so the
        // test stays deterministic per Q6′ = R2.
        let steps = [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0];
        for &net in &steps {
            for &scr in &steps {
                let v = ResourceContentionView::idle()
                    .with_network_rivalry(net)
                    .with_scratch_rivalry(scr);
                let t = v.total_rivalry();
                assert!(
                    (0.0..=1.0).contains(&t),
                    "(net={net}, scr={scr}) total {} out of [0,1]",
                    t
                );
            }
        }
    }

    #[test]
    fn total_rivalry_is_monotonic_in_network_rivalry() {
        // Headline T2.6 acceptance gate — half 1.
        let fixed_scratch = 0.3;
        let xs = [0.0, 0.1, 0.2, 0.5, 0.9, 1.0];
        for w in xs.windows(2) {
            let v_lo = ResourceContentionView::idle()
                .with_network_rivalry(w[0])
                .with_scratch_rivalry(fixed_scratch);
            let v_hi = ResourceContentionView::idle()
                .with_network_rivalry(w[1])
                .with_scratch_rivalry(fixed_scratch);
            assert!(
                v_lo.total_rivalry() <= v_hi.total_rivalry(),
                "net {} -> {}: total {} > {}",
                w[0],
                w[1],
                v_lo.total_rivalry(),
                v_hi.total_rivalry()
            );
        }
    }

    #[test]
    fn total_rivalry_is_monotonic_in_scratch_rivalry() {
        // Headline T2.6 acceptance gate — half 2.
        let fixed_net = 0.4;
        let xs = [0.0, 0.1, 0.3, 0.6, 0.8, 1.0];
        for w in xs.windows(2) {
            let v_lo = ResourceContentionView::idle()
                .with_network_rivalry(fixed_net)
                .with_scratch_rivalry(w[0]);
            let v_hi = ResourceContentionView::idle()
                .with_network_rivalry(fixed_net)
                .with_scratch_rivalry(w[1]);
            assert!(
                v_lo.total_rivalry() <= v_hi.total_rivalry(),
                "scratch {} -> {}: total {} > {}",
                w[0],
                w[1],
                v_lo.total_rivalry(),
                v_hi.total_rivalry()
            );
        }
    }

    #[test]
    fn builders_are_commutative() {
        // Setting network then scratch must yield the same view
        // as scratch then network — noisy-OR is symmetric.
        let a = ResourceContentionView::idle()
            .with_network_rivalry(0.3)
            .with_scratch_rivalry(0.6);
        let b = ResourceContentionView::idle()
            .with_scratch_rivalry(0.6)
            .with_network_rivalry(0.3);
        assert_eq!(a, b);
        assert!(approx_eq(a.total_rivalry(), b.total_rivalry()));
    }

    #[test]
    fn snapshot_reads_rivalry_from_each_pool() {
        use crate::network::ActiveStream;
        let mut net = FluidBandwidthPool::new(1.0, 4);
        let mut scr = ScratchIoPool::new(1.0, 4);
        // n = 3 on net → rivalry = 2/4 = 0.5.
        for id in 0..3 {
            net.admit(
                ActiveStream {
                    id,
                    admitted_at: 0,
                    demand_bytes: 0,
                },
                0,
            );
        }
        // n = 2 on scratch → rivalry = 1/4 = 0.25.
        for id in 100..102 {
            scr.admit(
                ActiveStream {
                    id,
                    admitted_at: 0,
                    demand_bytes: 0,
                },
                0,
            );
        }
        let v = ResourceContentionView::snapshot(&net, &scr);
        assert!(approx_eq(v.network_rivalry(), 0.5));
        assert!(approx_eq(v.scratch_rivalry(), 0.25));
        // noisy-OR: 1 - 0.5 * 0.75 = 0.625
        assert!(approx_eq(v.total_rivalry(), 0.625));
    }
}
