//! Core simulator-clock and event types.
//!
//! Defines the trio [`SimTime`], [`AgentId`], [`EventSeq`] that
//! parameterise every scheduling decision plus the [`Event`] /
//! [`EventKind`] pair that flows through the DES event queue.
//!
//! Two design choices are load-bearing for downstream determinism:
//!
//! 1. **`SimTime` is `u64` nanoseconds since the start of the
//!    replicate.** Replicate horizons are bounded at one simulated
//!    hour (Q6.3 = a → 3.6 × 10¹² ns), so a `u64` carries five
//!    orders of magnitude of headroom and never wraps in any
//!    headline scenario.
//! 2. **`Event` ordering is the lexicographic tuple `(at, agent,
//!    seq)`.** This is the **only** ordering used by the event
//!    queue and is enforced by hand-written `Ord` / `PartialOrd`
//!    impls so the tie-break is auditable in one place and
//!    immune to field-reordering refactors.
//!
//! Per Q6′ = R2, deterministic-per-replicate single-machine replay
//! requires this tie-break to be total and unique across every
//! event scheduled in a run. The `seq` field is the monotonic
//! counter that ensures uniqueness when two events would otherwise
//! land at the same `(at, agent)`.

use std::cmp::Ordering;

/// Simulator clock — nanoseconds since the start of the replicate.
///
/// Logical time. Never wall-clock time. Real-time-mode hooks (if
/// ever added) would live outside this type.
pub type SimTime = u64;

/// Identifier for a scheduling agent (resource, workflow,
/// portal, …). Population is small per super-site (`n = 3 + k`,
/// with `k` the QPU count); a `u32` is plenty for the
/// multi-super-site federation.
pub type AgentId = u32;

/// Monotonic per-replicate event sequence number. Bumped each time
/// the simulator schedules an event; used as the final tie-break
/// key in the `(at, agent, seq)` triple so the event order across
/// a run is total and deterministic.
pub type EventSeq = u64;

/// Closed enumeration of the event kinds the DES kernel will route.
///
/// Variants are placeholder unit-like today; the data each kind
/// carries lands together with the producing component in the
/// phases that own it (workflow arrivals in Phase 2, advertisement
/// in Phase 2, bargaining-round trigger in Phase 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// A workflow arrives at the federation router.
    Arrival,
    /// A per-resource agent emits its periodic advertised summary
    /// (τ_adv tick from Q4).
    Advertise,
    /// A bargaining episode runs its next best-response round.
    BargainRound,
    /// A workflow or task reaches its terminal state.
    Completion,
}

/// One scheduled event in the DES kernel.
///
/// `Event` is `Copy` because every field is. The event queue moves
/// these by value; ergonomic and avoids pointer indirection on the
/// hot path.
///
/// **Identity & ordering are keyed solely off `(at, agent, seq)`**;
/// `kind` is payload metadata. Every `PartialEq` / `Eq` / `Hash` /
/// `Ord` / `PartialOrd` impl on `Event` uses only the tie-break
/// triple, so the standard `Eq + Ord` contract holds:
/// `a.cmp(&b) == Ordering::Equal ⇔ a == b`. The simulator
/// guarantees unique `seq` per replicate, so two events constructed
/// in a real run with the same `(at, agent, seq)` will also share
/// the same `kind`.
#[derive(Debug, Clone, Copy)]
pub struct Event {
    /// When the event fires, in `SimTime` nanoseconds.
    pub at: SimTime,
    /// Owning agent.
    pub agent: AgentId,
    /// Monotonic tie-break key.
    pub seq: EventSeq,
    /// Discriminator the dispatch table uses to route the event.
    pub kind: EventKind,
}

impl Event {
    /// The full tie-break key used by every ordering / equality /
    /// hashing operation on `Event`. Kept as a method (rather than
    /// inlined in each impl) so the "exactly three fields, in this
    /// exact order" invariant is auditable in a single line.
    #[inline]
    fn tie_break_key(&self) -> (SimTime, AgentId, EventSeq) {
        (self.at, self.agent, self.seq)
    }
}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.tie_break_key() == other.tie_break_key()
    }
}

impl Eq for Event {}

impl std::hash::Hash for Event {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.tie_break_key().hash(state);
    }
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> Ordering {
        self.tie_break_key().cmp(&other.tie_break_key())
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(at: SimTime, agent: AgentId, seq: EventSeq) -> Event {
        Event {
            at,
            agent,
            seq,
            kind: EventKind::Arrival,
        }
    }

    #[test]
    fn tie_break_orders_by_time_then_agent_then_seq() {
        // The expected ascending order. Each line tests one of the
        // three axes: time, then agent within a tied time, then
        // seq within a tied (time, agent).
        let want = vec![
            ev(0, 0, 0),
            ev(0, 0, 1),
            ev(0, 1, 0),
            ev(0, 1, 5),
            ev(10, 0, 0),
            ev(10, 0, 9),
            ev(10, 5, 0),
            ev(20, 0, 0),
        ];

        // Shuffle into a stress order and re-sort. Sort must
        // produce `want` exactly.
        let mut shuffled = vec![
            ev(10, 5, 0),
            ev(0, 0, 0),
            ev(20, 0, 0),
            ev(0, 1, 5),
            ev(10, 0, 9),
            ev(0, 0, 1),
            ev(10, 0, 0),
            ev(0, 1, 0),
        ];
        shuffled.sort();
        assert_eq!(shuffled, want);
    }

    #[test]
    fn event_kind_does_not_affect_identity_or_ordering() {
        // Same (at, agent, seq) with different kinds must compare
        // equal under every relevant trait: Eq + Ord per the
        // standard contract.
        let a = Event {
            at: 5,
            agent: 1,
            seq: 7,
            kind: EventKind::Arrival,
        };
        let b = Event {
            at: 5,
            agent: 1,
            seq: 7,
            kind: EventKind::BargainRound,
        };
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert_eq!(a, b);
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        a.hash(&mut ha);
        b.hash(&mut hb);
        assert_eq!(ha.finish(), hb.finish());
    }

    #[test]
    fn partial_ord_matches_ord() {
        let a = ev(0, 0, 0);
        let b = ev(1, 0, 0);
        assert_eq!(a.partial_cmp(&b), Some(a.cmp(&b)));
        assert_eq!(b.partial_cmp(&a), Some(b.cmp(&a)));
        assert_eq!(a.partial_cmp(&a), Some(Ordering::Equal));
    }

    #[test]
    fn binary_heap_with_reverse_pops_earliest_first() {
        // Demonstrates the intended consumption pattern for the
        // event queue: a `BinaryHeap<Reverse<Event>>` is a min-heap
        // keyed by `(at, agent, seq)`.
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        let mut heap: BinaryHeap<Reverse<Event>> = BinaryHeap::new();
        heap.push(Reverse(ev(10, 0, 0)));
        heap.push(Reverse(ev(0, 1, 0)));
        heap.push(Reverse(ev(0, 0, 5)));
        heap.push(Reverse(ev(0, 0, 0)));

        let mut popped = Vec::new();
        while let Some(Reverse(e)) = heap.pop() {
            popped.push(e);
        }

        assert_eq!(
            popped,
            vec![ev(0, 0, 0), ev(0, 0, 5), ev(0, 1, 0), ev(10, 0, 0)]
        );
    }
}
