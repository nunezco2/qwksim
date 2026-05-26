//! Fluid memory-bandwidth pool (FLAG-I closed: linear sharing).
//!
//! A pool exposes a fixed *capacity* (bytes per second) shared
//! equally among an open set of concurrent *streams*. At any
//! instant, every active stream sees `capacity / N` bytes per
//! second, where `N` is the number of streams currently open
//! against the pool. Stream identity is an opaque `u64` chosen
//! by the caller (typically `(workflow_id, task_index)` packed in
//! some stable way) so the simulator state path stays
//! deterministic under Q6′ = R2.
//!
//! No τ_adv staleness, no weighted sharing — the simplest fluid
//! model from §2.2 of the engineering plan. Weighted variants
//! and τ_adv decoupling land in later Phase-2 PRs alongside the
//! intra-site network and scratch-I/O pools.
//!
//! Conservation: every share is `capacity / N`, computed in `f64`
//! so the sum of shares equals capacity exactly modulo f64
//! rounding (a 53-bit mantissa absorbs the integer-division
//! tail that the rest of the simulator would otherwise have to
//! account for).

use std::collections::BTreeSet;

/// Identifier for one concurrent stream against the pool. Caller
/// is responsible for keeping ids unique within a pool instance.
pub type StreamId = u64;

/// Bytes per second.
pub type Bandwidth = f64;

/// A fluid bandwidth pool. Capacity is fixed at construction;
/// streams are admitted via [`open`](MemoryBandwidthPool::open)
/// and dismissed via [`close`](MemoryBandwidthPool::close). The
/// share each open stream sees is recomputed every time the
/// active set changes — there is no notion of "ongoing
/// contract": each stream's share rises and falls instantly with
/// the active count.
#[derive(Debug, Clone)]
pub struct MemoryBandwidthPool {
    capacity: Bandwidth,
    // BTreeSet (not HashSet) so iteration order is deterministic
    // under Q6′ = R2. The clippy::disallowed_methods workspace
    // gate would already catch a HashSet swap; the BTreeSet
    // choice is the canonical one for simulator-stateful sets.
    active: BTreeSet<StreamId>,
}

impl MemoryBandwidthPool {
    /// Construct a pool with the given capacity (bytes per
    /// second).
    ///
    /// # Panics
    /// Panics if `capacity` is non-finite or non-positive.
    pub fn new(capacity: Bandwidth) -> Self {
        assert!(
            capacity.is_finite() && capacity > 0.0,
            "MemoryBandwidthPool::new: capacity must be finite and positive, got {capacity}"
        );
        Self {
            capacity,
            active: BTreeSet::new(),
        }
    }

    /// Total configured pool capacity.
    pub fn capacity(&self) -> Bandwidth {
        self.capacity
    }

    /// Number of currently-active streams.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// `true` iff `stream` is open against the pool.
    pub fn is_active(&self, stream: StreamId) -> bool {
        self.active.contains(&stream)
    }

    /// Admit `stream`. Returns `true` if this was the first time
    /// the stream was opened; `false` if it was already active
    /// (the call is then idempotent).
    pub fn open(&mut self, stream: StreamId) -> bool {
        self.active.insert(stream)
    }

    /// Dismiss `stream`. Returns `true` if the stream was active
    /// and is now closed; `false` if it was not active (the call
    /// is then idempotent).
    pub fn close(&mut self, stream: StreamId) -> bool {
        self.active.remove(&stream)
    }

    /// The instantaneous bandwidth share `stream` sees right now:
    /// `capacity / N` if the stream is active, else `0`.
    pub fn share_for(&self, stream: StreamId) -> Bandwidth {
        if self.active.is_empty() || !self.active.contains(&stream) {
            0.0
        } else {
            self.capacity / self.active.len() as f64
        }
    }

    /// The instantaneous per-stream share for any currently-active
    /// stream (they all see the same value under equal-share
    /// fluid). `None` if no streams are active.
    pub fn current_share(&self) -> Option<Bandwidth> {
        if self.active.is_empty() {
            None
        } else {
            Some(self.capacity / self.active.len() as f64)
        }
    }

    /// Sum of every active stream's instantaneous share. Equals
    /// `capacity` when at least one stream is open (the
    /// conservation invariant), `0` when the pool is idle.
    pub fn total_active_share(&self) -> Bandwidth {
        match self.current_share() {
            None => 0.0,
            Some(s) => s * self.active.len() as f64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOL: f64 = 1e-9;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < TOL
    }

    #[test]
    fn idle_pool_reports_zero_active_and_zero_share() {
        let pool = MemoryBandwidthPool::new(1_000_000.0);
        assert_eq!(pool.capacity(), 1_000_000.0);
        assert_eq!(pool.active_count(), 0);
        assert!(!pool.is_active(0));
        assert_eq!(pool.share_for(0), 0.0);
        assert_eq!(pool.current_share(), None);
        assert_eq!(pool.total_active_share(), 0.0);
    }

    #[test]
    fn single_stream_consumes_full_capacity() {
        let mut pool = MemoryBandwidthPool::new(1_000_000.0);
        assert!(pool.open(7));
        assert_eq!(pool.share_for(7), 1_000_000.0);
        assert_eq!(pool.current_share(), Some(1_000_000.0));
        assert!(approx_eq(pool.total_active_share(), 1_000_000.0));
    }

    #[test]
    fn n_streams_each_receive_capacity_over_n_and_bandwidth_is_conserved() {
        // T2.2 headline acceptance gate: open n streams, every
        // share is capacity / n, and the sum of shares equals
        // capacity.
        let capacity = 1_000_000.0;
        for &n in &[1u64, 2, 3, 4, 5, 8, 16, 32, 127] {
            let mut pool = MemoryBandwidthPool::new(capacity);
            for i in 0..n {
                pool.open(i);
            }
            assert_eq!(pool.active_count(), n as usize);
            let expected_share = capacity / n as f64;
            for i in 0..n {
                assert!(
                    approx_eq(pool.share_for(i), expected_share),
                    "n={n} share_for({i}) = {} ≠ {expected_share}",
                    pool.share_for(i)
                );
            }
            assert!(
                approx_eq(pool.total_active_share(), capacity),
                "n={n}: total active share {} ≠ capacity {capacity}",
                pool.total_active_share(),
            );
        }
    }

    #[test]
    fn shares_rebalance_when_streams_open_and_close() {
        // Open three streams, share = capacity / 3.
        // Close one, share = capacity / 2 for the remaining two.
        // Close another, share = capacity for the last one.
        let capacity = 90.0;
        let mut pool = MemoryBandwidthPool::new(capacity);
        pool.open(1);
        pool.open(2);
        pool.open(3);
        for s in [1, 2, 3] {
            assert!(approx_eq(pool.share_for(s), 30.0));
        }

        assert!(pool.close(2));
        for s in [1, 3] {
            assert!(approx_eq(pool.share_for(s), 45.0));
        }
        assert_eq!(pool.share_for(2), 0.0, "closed stream sees 0");

        assert!(pool.close(1));
        assert!(approx_eq(pool.share_for(3), 90.0));
    }

    #[test]
    fn open_and_close_are_idempotent() {
        let mut pool = MemoryBandwidthPool::new(100.0);
        assert!(pool.open(1));
        assert!(!pool.open(1), "second open of same id is a no-op");
        assert_eq!(pool.active_count(), 1);

        assert!(pool.close(1));
        assert!(!pool.close(1), "second close of same id is a no-op");
        assert_eq!(pool.active_count(), 0);
    }

    #[test]
    fn share_for_unknown_stream_is_zero_even_when_pool_is_busy() {
        let mut pool = MemoryBandwidthPool::new(100.0);
        pool.open(1);
        pool.open(2);
        assert_eq!(pool.share_for(99), 0.0);
    }

    #[test]
    #[should_panic(expected = "capacity must be finite and positive")]
    fn zero_capacity_pool_rejects() {
        MemoryBandwidthPool::new(0.0);
    }

    #[test]
    #[should_panic(expected = "capacity must be finite and positive")]
    fn nan_capacity_pool_rejects() {
        MemoryBandwidthPool::new(f64::NAN);
    }

    #[test]
    #[should_panic(expected = "capacity must be finite and positive")]
    fn negative_capacity_pool_rejects() {
        MemoryBandwidthPool::new(-1.0);
    }
}
