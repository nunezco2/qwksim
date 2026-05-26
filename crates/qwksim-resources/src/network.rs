//! `FluidBandwidthPool` — the intra-site network model from §4.2
//! of the engineering plan, closing FLAG-I in favour of the
//! **linear equal-share fluid** sharing rule.
//!
//! Differences from [`MemoryBandwidthPool`](crate::MemoryBandwidthPool):
//!
//! - Each open stream is described by an [`ActiveStream`] record
//!   (admitted_at, demand_bytes) rather than a bare `StreamId`.
//!   Later phases will use these fields to predict transfer
//!   completion times.
//! - Exposes a [`FluidBandwidthPool::rivalry`] term used by the
//!   bargaining utility (FLAG-C closed formula). Rivalry is
//!   `min(1, (n − 1) / n_cap)` where `n_cap` is the per-link
//!   *soft saturation count*: when there are exactly `n_cap + 1`
//!   active streams the rivalry term reaches `1`.
//! - Exposes a [`FluidBandwidthPool::advertised_summary`] suitable
//!   for the local + advertised information regime. The summary
//!   today is live (no τ_adv staleness yet — that lands with the
//!   advertisement protocol in T4.x) and the proptest in this
//!   module asserts the invariant `advertised free ≤ true free`
//!   holds at every step of a randomised op sequence.

use std::collections::BTreeMap;

use qwksim_core::event::SimTime;

use crate::bandwidth::{Bandwidth, StreamId};

/// One open stream against a [`FluidBandwidthPool`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveStream {
    /// Opaque identifier for the stream; the simulator keeps
    /// these unique per-pool.
    pub id: StreamId,
    /// Simulator time at which the stream was admitted.
    pub admitted_at: SimTime,
    /// Total bytes the stream intends to transfer. `0` if the
    /// caller does not yet know.
    pub demand_bytes: u64,
}

/// What the bargaining solver sees about a `FluidBandwidthPool`
/// at simulator-time `now`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidPoolSummary {
    /// Total configured pool capacity.
    pub capacity: Bandwidth,
    /// Streams currently open against the pool.
    pub active_count: u32,
    /// Soft saturation count `n_cap` configured at pool
    /// construction.
    pub saturation_count: u32,
    /// Bandwidth share every active stream currently sees
    /// (`0` if `active_count == 0`).
    pub current_share: Bandwidth,
    /// Rivalry term in `[0, 1]` (see [`FluidBandwidthPool::rivalry`]).
    pub rivalry: f64,
    /// Headroom in stream-slots before rivalry hits `1`. Equal to
    /// `saturation_count - min(active_count, saturation_count)`
    /// today (live); becomes a τ_adv-stale snapshot once the
    /// advertisement protocol lands in T4.x. The proptest
    /// invariant requires this to never exceed the true headroom
    /// at the same instant.
    pub free_slots_until_saturation: u32,
}

/// Fluid intra-site network bandwidth pool. Linear equal-share
/// among active streams (FLAG-I closed).
#[derive(Debug, Clone)]
pub struct FluidBandwidthPool {
    capacity: Bandwidth,
    saturation_count: u32,
    // BTreeMap (not HashMap) keeps iteration order deterministic
    // under Q6′ = R2. The workspace clippy::disallowed_methods
    // gate (T1.11) backstops this choice.
    active_streams: BTreeMap<StreamId, ActiveStream>,
}

impl FluidBandwidthPool {
    /// Construct a pool with the given `capacity` (bytes per
    /// second) and `saturation_count` (`n_cap` in the rivalry
    /// formula).
    ///
    /// # Panics
    /// Panics if `capacity` is non-finite or non-positive, or if
    /// `saturation_count` is zero.
    pub fn new(capacity: Bandwidth, saturation_count: u32) -> Self {
        assert!(
            capacity.is_finite() && capacity > 0.0,
            "FluidBandwidthPool::new: capacity must be finite and positive, got {capacity}"
        );
        assert!(
            saturation_count > 0,
            "FluidBandwidthPool::new: saturation_count (n_cap) must be > 0"
        );
        Self {
            capacity,
            saturation_count,
            active_streams: BTreeMap::new(),
        }
    }

    /// Total configured pool capacity.
    pub fn capacity(&self) -> Bandwidth {
        self.capacity
    }

    /// Soft saturation count `n_cap`.
    pub fn saturation_count(&self) -> u32 {
        self.saturation_count
    }

    /// Number of currently-active streams.
    pub fn active_count(&self) -> usize {
        self.active_streams.len()
    }

    /// `true` iff a stream with `id` is currently active.
    pub fn is_active(&self, id: StreamId) -> bool {
        self.active_streams.contains_key(&id)
    }

    /// Admit `stream` against the pool. If a stream with the same
    /// `id` is already active, its record is replaced (the new
    /// `admitted_at` / `demand_bytes` overwrite the old).
    pub fn admit(&mut self, stream: ActiveStream, _now: SimTime) {
        self.active_streams.insert(stream.id, stream);
    }

    /// Release the stream identified by `id`. Returns the record
    /// that was active, or `None` if no such stream was open.
    pub fn release(&mut self, id: StreamId, _now: SimTime) -> Option<ActiveStream> {
        self.active_streams.remove(&id)
    }

    /// Instantaneous bandwidth share `id` sees right now:
    /// `capacity / N` if active, `0` otherwise.
    pub fn current_share(&self, id: StreamId) -> Bandwidth {
        if self.active_streams.is_empty() || !self.active_streams.contains_key(&id) {
            0.0
        } else {
            self.capacity / self.active_streams.len() as f64
        }
    }

    /// Rivalry term in `[0, 1]`, as defined in §4.2:
    /// `min(1, (n − 1) / n_cap)` where `n` is the active count
    /// and `n_cap = self.saturation_count`. Used by the
    /// bargaining utility's shared-constraint rivalry component
    /// (FLAG-J: shared-constraint pattern, no extra rivalry agent).
    pub fn rivalry(&self) -> f64 {
        let n = self.active_streams.len() as f64;
        if n < 1.0 {
            0.0
        } else {
            ((n - 1.0) / self.saturation_count as f64).min(1.0)
        }
    }

    /// Bandwidth-share value common to every active stream right
    /// now. `None` if the pool is idle.
    pub fn current_common_share(&self) -> Option<Bandwidth> {
        if self.active_streams.is_empty() {
            None
        } else {
            Some(self.capacity / self.active_streams.len() as f64)
        }
    }

    /// Snapshot of the pool's state for the bargaining solver.
    /// Today the snapshot is live; once τ_adv staleness lands
    /// (T4.x advertisement protocol) the values may lag, but the
    /// invariant
    /// `advertised free_slots_until_saturation ≤ true free`
    /// must still hold at every instant — see the proptest in
    /// `tests`.
    pub fn advertised_summary(&self, _now: SimTime) -> FluidPoolSummary {
        let active = self.active_streams.len() as u32;
        let free_slots = self.saturation_count.saturating_sub(active);
        FluidPoolSummary {
            capacity: self.capacity,
            active_count: active,
            saturation_count: self.saturation_count,
            current_share: self.current_common_share().unwrap_or(0.0),
            rivalry: self.rivalry(),
            free_slots_until_saturation: free_slots,
        }
    }

    /// Borrow the underlying active-stream map. Provided for
    /// proptest assertions and diagnostics; downstream simulator
    /// code should prefer the live accessors above.
    pub fn active_streams(&self) -> &BTreeMap<StreamId, ActiveStream> {
        &self.active_streams
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
        let p = FluidBandwidthPool::new(1_000_000.0, 8);
        assert_eq!(p.active_count(), 0);
        assert_eq!(p.rivalry(), 0.0);
        assert_eq!(p.current_share(0), 0.0);
        assert_eq!(p.current_common_share(), None);
        let s = p.advertised_summary(0);
        assert_eq!(s.active_count, 0);
        assert_eq!(s.free_slots_until_saturation, 8);
        assert_eq!(s.rivalry, 0.0);
    }

    #[test]
    fn rivalry_increases_with_active_count_and_caps_at_one() {
        let mut p = FluidBandwidthPool::new(1_000_000.0, 4);
        // n = 1 → rivalry = 0
        p.admit(stream(0), 0);
        assert!(approx_eq(p.rivalry(), 0.0));
        // n = 2 → rivalry = 1/4
        p.admit(stream(1), 0);
        assert!(approx_eq(p.rivalry(), 0.25));
        // n = 3 → rivalry = 2/4
        p.admit(stream(2), 0);
        assert!(approx_eq(p.rivalry(), 0.5));
        // n = 5 (= n_cap + 1) → rivalry = 1 (saturated)
        p.admit(stream(3), 0);
        p.admit(stream(4), 0);
        assert!(approx_eq(p.rivalry(), 1.0));
        // n = 10 (well past saturation) → still 1 (capped)
        for i in 5..10 {
            p.admit(stream(i), 0);
        }
        assert!(approx_eq(p.rivalry(), 1.0));
    }

    #[test]
    fn current_share_is_capacity_over_active_count() {
        let mut p = FluidBandwidthPool::new(600.0, 8);
        p.admit(stream(1), 0);
        assert!(approx_eq(p.current_share(1), 600.0));
        p.admit(stream(2), 0);
        assert!(approx_eq(p.current_share(1), 300.0));
        assert!(approx_eq(p.current_share(2), 300.0));
        p.admit(stream(3), 0);
        for id in [1, 2, 3] {
            assert!(approx_eq(p.current_share(id), 200.0));
        }
    }

    #[test]
    fn release_returns_record_and_rebalances_shares() {
        let mut p = FluidBandwidthPool::new(900.0, 8);
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
        assert!(approx_eq(p.current_share(1), 450.0));
        assert!(approx_eq(p.current_share(3), 450.0));
        assert_eq!(p.current_share(2), 0.0);

        // Idempotent release of an inactive id returns None.
        assert_eq!(p.release(99, 30), None);
    }

    #[test]
    fn advertised_summary_matches_live_state_today() {
        let mut p = FluidBandwidthPool::new(1000.0, 4);
        p.admit(stream(0), 0);
        p.admit(stream(1), 0);
        let s = p.advertised_summary(0);
        assert_eq!(s.capacity, 1000.0);
        assert_eq!(s.active_count, 2);
        assert_eq!(s.saturation_count, 4);
        assert!(approx_eq(s.current_share, 500.0));
        assert!(approx_eq(s.rivalry, 0.25));
        // saturation_count - active_count = 4 - 2 = 2
        assert_eq!(s.free_slots_until_saturation, 2);
    }

    #[test]
    #[should_panic(expected = "capacity must be finite and positive")]
    fn zero_capacity_pool_rejects() {
        let _ = FluidBandwidthPool::new(0.0, 4);
    }

    #[test]
    #[should_panic(expected = "saturation_count (n_cap) must be > 0")]
    fn zero_saturation_pool_rejects() {
        let _ = FluidBandwidthPool::new(100.0, 0);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// One step in a randomised pool-driver sequence.
    #[derive(Debug, Clone, Copy)]
    enum Op {
        Admit(StreamId, SimTime, u64),
        Release(StreamId, SimTime),
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0u64..32, 0u64..10_000_000, 0u64..1_000_000)
                .prop_map(|(id, t, demand)| Op::Admit(id, t, demand)),
            (0u64..32, 0u64..10_000_000).prop_map(|(id, t)| Op::Release(id, t)),
        ]
    }

    proptest! {
        /// **Headline T2.3 acceptance gate.** Build a pool with
        /// randomised capacity and saturation count, drive it
        /// through a sequence of admit/release ops, and assert at
        /// every step that
        ///
        ///   advertised free_slots_until_saturation
        ///     ≤ true free_slots_until_saturation
        ///
        /// Today the pool's summary is live, so the two are
        /// equal at every instant — the proptest exercises that
        /// equality across a wide op space, locking the invariant
        /// in as a regression gate for the future τ_adv-stale
        /// summary (T4.x).
        #[test]
        fn advertised_free_capacity_never_exceeds_true(
            capacity in (1.0f64..1.0e9),
            saturation_count in 1u32..32,
            ops in proptest::collection::vec(op_strategy(), 0..200),
        ) {
            let mut pool = FluidBandwidthPool::new(capacity, saturation_count);
            for op in &ops {
                match *op {
                    Op::Admit(id, t, demand) => pool.admit(
                        ActiveStream { id, admitted_at: t, demand_bytes: demand },
                        t,
                    ),
                    Op::Release(id, t) => {
                        let _ = pool.release(id, t);
                    }
                }

                let summary = pool.advertised_summary(0);
                let true_free = saturation_count
                    .saturating_sub(pool.active_count() as u32);
                prop_assert!(
                    summary.free_slots_until_saturation <= true_free,
                    "advertised {} > true {} (n={}, n_cap={})",
                    summary.free_slots_until_saturation,
                    true_free,
                    pool.active_count(),
                    saturation_count,
                );

                // Conservation: when at least one stream is
                // open, every active stream sees an equal share
                // and the total equals capacity (up to f64 round).
                if pool.active_count() > 0 {
                    let share = pool.current_common_share().expect("non-idle");
                    let total = share * pool.active_count() as f64;
                    prop_assert!(
                        (total - capacity).abs() < 1e-6 * capacity,
                        "conservation: {} != {capacity} (n={})",
                        total,
                        pool.active_count()
                    );
                }

                // Rivalry stays in [0, 1].
                let r = pool.rivalry();
                prop_assert!((0.0..=1.0).contains(&r), "rivalry {} out of [0,1]", r);
            }
        }
    }
}
