//! `ScratchIoPool` — per-partition fluid scratch I/O pool (Q7.5 =
//! s3 scratch capacity + I/O bandwidth model).
//!
//! Structurally identical to [`FluidBandwidthPool`](crate::FluidBandwidthPool):
//! same admit/release/current_share/rivalry surface, same linear
//! equal-share arithmetic, same τ_adv-stale-tolerant advertised
//! summary. The type is **distinct** so the bargaining utility
//! (FLAG-C) can apply a separate `γ` weight to scratch-I/O
//! rivalry vs. network-link rivalry, and so the per-partition
//! resource registry can hold one of each.
//!
//! Today this is a thin newtype around `FluidBandwidthPool`. If
//! scratch ever needs additional state beyond I/O bandwidth (e.g.
//! a capacity ceiling, or a per-stream offset cursor), the
//! divergence will land here without touching the network model.

use qwksim_core::event::SimTime;

use crate::bandwidth::{Bandwidth, StreamId};
use crate::network::{ActiveStream, FluidBandwidthPool, FluidPoolSummary};

/// Fluid scratch I/O bandwidth pool. Q7.5 = s3 closed; rivalry
/// composes into the shared-constraint pattern (FLAG-J).
#[derive(Debug, Clone)]
pub struct ScratchIoPool {
    inner: FluidBandwidthPool,
}

impl ScratchIoPool {
    /// Build a scratch I/O pool with the given `capacity` (bytes
    /// per second) and `saturation_count` (`n_cap` in the §4.2
    /// rivalry formula).
    pub fn new(capacity: Bandwidth, saturation_count: u32) -> Self {
        Self {
            inner: FluidBandwidthPool::new(capacity, saturation_count),
        }
    }

    /// Total configured I/O bandwidth capacity.
    pub fn capacity(&self) -> Bandwidth {
        self.inner.capacity()
    }

    /// Soft saturation count `n_cap`.
    pub fn saturation_count(&self) -> u32 {
        self.inner.saturation_count()
    }

    /// Number of streams currently issuing I/O against the pool.
    pub fn active_count(&self) -> usize {
        self.inner.active_count()
    }

    /// `true` iff a stream with `id` is currently active.
    pub fn is_active(&self, id: StreamId) -> bool {
        self.inner.is_active(id)
    }

    /// Admit `stream` against the pool. Replaces a same-`id`
    /// entry if one was already active.
    pub fn admit(&mut self, stream: ActiveStream, now: SimTime) {
        self.inner.admit(stream, now);
    }

    /// Release the stream identified by `id`. Returns the record
    /// that was active, or `None` if no such stream was open.
    pub fn release(&mut self, id: StreamId, now: SimTime) -> Option<ActiveStream> {
        self.inner.release(id, now)
    }

    /// Instantaneous bandwidth share `id` sees right now.
    pub fn current_share(&self, id: StreamId) -> Bandwidth {
        self.inner.current_share(id)
    }

    /// Bandwidth-share value common to every active stream right
    /// now. `None` if the pool is idle.
    pub fn current_common_share(&self) -> Option<Bandwidth> {
        self.inner.current_common_share()
    }

    /// Rivalry term in `[0, 1]`. Same formula as
    /// [`FluidBandwidthPool::rivalry`].
    pub fn rivalry(&self) -> f64 {
        self.inner.rivalry()
    }

    /// Snapshot of the pool's state for the bargaining solver.
    /// Today's snapshot is live; τ_adv staleness lands in T4.x.
    pub fn advertised_summary(&self, now: SimTime) -> FluidPoolSummary {
        self.inner.advertised_summary(now)
    }

    /// Borrow the underlying [`FluidBandwidthPool`] for
    /// diagnostics. Downstream simulator code should prefer the
    /// methods above.
    pub fn as_fluid(&self) -> &FluidBandwidthPool {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(id: StreamId) -> ActiveStream {
        ActiveStream {
            id,
            admitted_at: 0,
            demand_bytes: 0,
        }
    }

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn idle_pool_reports_zero_rivalry_and_zero_share() {
        let p = ScratchIoPool::new(1_000_000.0, 8);
        assert_eq!(p.active_count(), 0);
        assert_eq!(p.rivalry(), 0.0);
        assert_eq!(p.current_share(0), 0.0);
        assert_eq!(p.current_common_share(), None);
        let s = p.advertised_summary(0);
        assert_eq!(s.free_slots_until_saturation, 8);
    }

    #[test]
    fn n_streams_each_receive_capacity_over_n_and_bandwidth_is_conserved() {
        let capacity = 1_000_000_000.0;
        for &n in &[1u64, 2, 3, 4, 8, 16] {
            let mut p = ScratchIoPool::new(capacity, 16);
            for i in 0..n {
                p.admit(stream(i), 0);
            }
            let expected_share = capacity / n as f64;
            for i in 0..n {
                assert!(approx_eq(p.current_share(i), expected_share), "n={n}");
            }
            let total = p.current_common_share().unwrap() * n as f64;
            assert!(
                approx_eq(total, capacity),
                "n={n}: total {} ≠ capacity {capacity}",
                total
            );
        }
    }

    #[test]
    fn rivalry_increases_with_active_count_and_caps_at_one() {
        let mut p = ScratchIoPool::new(1.0, 4);
        assert!(approx_eq(p.rivalry(), 0.0));
        p.admit(stream(0), 0);
        assert!(approx_eq(p.rivalry(), 0.0));
        p.admit(stream(1), 0);
        assert!(approx_eq(p.rivalry(), 0.25));
        p.admit(stream(2), 0);
        assert!(approx_eq(p.rivalry(), 0.5));
        p.admit(stream(3), 0);
        assert!(approx_eq(p.rivalry(), 0.75));
        p.admit(stream(4), 0);
        assert!(approx_eq(p.rivalry(), 1.0));
        for i in 5..10 {
            p.admit(stream(i), 0);
        }
        assert!(approx_eq(p.rivalry(), 1.0));
    }

    #[test]
    fn release_returns_record_and_rebalances_shares() {
        let mut p = ScratchIoPool::new(900.0, 8);
        p.admit(
            ActiveStream {
                id: 1,
                admitted_at: 10,
                demand_bytes: 4096,
            },
            10,
        );
        p.admit(stream(2), 11);
        p.admit(stream(3), 12);
        for id in [1, 2, 3] {
            assert!(approx_eq(p.current_share(id), 300.0));
        }
        let released = p.release(2, 20).expect("active");
        assert_eq!(released.id, 2);
        for id in [1, 3] {
            assert!(approx_eq(p.current_share(id), 450.0));
        }
        assert_eq!(p.release(99, 30), None);
    }

    #[test]
    fn advertised_summary_matches_live_state_today() {
        let mut p = ScratchIoPool::new(1000.0, 4);
        p.admit(stream(0), 0);
        p.admit(stream(1), 0);
        let s = p.advertised_summary(0);
        assert_eq!(s.capacity, 1000.0);
        assert_eq!(s.active_count, 2);
        assert_eq!(s.saturation_count, 4);
        assert!(approx_eq(s.current_share, 500.0));
        assert!(approx_eq(s.rivalry, 0.25));
        assert_eq!(s.free_slots_until_saturation, 2);
    }

    #[test]
    #[should_panic(expected = "capacity must be finite and positive")]
    fn zero_capacity_pool_rejects() {
        let _ = ScratchIoPool::new(0.0, 4);
    }

    #[test]
    #[should_panic(expected = "saturation_count (n_cap) must be > 0")]
    fn zero_saturation_pool_rejects() {
        let _ = ScratchIoPool::new(100.0, 0);
    }
}
