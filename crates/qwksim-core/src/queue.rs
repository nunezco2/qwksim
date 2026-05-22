//! Deterministic DES event queue.
//!
//! [`EventQueue`] is a thin newtype around
//! `BinaryHeap<Reverse<Event>>` — a min-heap keyed by the
//! `(at, agent, seq)` tie-break triple defined on [`Event`].
//! Popping the queue yields events in ascending `(at, agent, seq)`
//! order, which is the canonical advance order of the DES kernel
//! under Q6′ = R2 single-machine determinism.
//!
//! [`EventSeqAllocator`] is the matching monotonic counter that
//! produces unique [`EventSeq`] values within a replicate. The
//! `Simulator` type (lands in T1.3) will own one of each.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::event::{Event, EventSeq};

/// Min-heap of `Event`s.
///
/// Implementation: `BinaryHeap<Reverse<Event>>`. `Event` itself
/// orders by `(at, agent, seq)` ascending (earliest = least); the
/// outer `Reverse` flips the heap's max-heap default into a
/// min-heap, so `pop()` yields the earliest event.
#[derive(Debug, Default, Clone)]
pub struct EventQueue {
    heap: BinaryHeap<Reverse<Event>>,
}

impl EventQueue {
    /// Empty queue with no preallocated capacity.
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
        }
    }

    /// Empty queue with at least `capacity` slots preallocated.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(capacity),
        }
    }

    /// Insert `event` into the queue.
    pub fn push(&mut self, event: Event) {
        self.heap.push(Reverse(event));
    }

    /// Remove and return the earliest event, or `None` if empty.
    pub fn pop(&mut self) -> Option<Event> {
        self.heap.pop().map(|Reverse(e)| e)
    }

    /// Borrow the earliest event without removing it.
    pub fn peek(&self) -> Option<&Event> {
        self.heap.peek().map(|Reverse(e)| e)
    }

    /// Number of events currently in the queue.
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// `true` iff `len() == 0`.
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

/// Monotonic per-replicate allocator for [`EventSeq`].
///
/// The future `Simulator` owns one of these. Every call to
/// [`EventSeqAllocator::allocate`] returns a strictly increasing
/// value, so the `(at, agent, seq)` tie-break key on each
/// scheduled event is globally unique within a replicate.
#[derive(Debug, Default, Clone, Copy)]
pub struct EventSeqAllocator {
    next: EventSeq,
}

impl EventSeqAllocator {
    /// Allocator starting at `seq = 0`.
    pub fn new() -> Self {
        Self { next: 0 }
    }

    /// Allocate the next sequence number and bump the counter.
    pub fn allocate(&mut self) -> EventSeq {
        let s = self.next;
        self.next = self
            .next
            .checked_add(1)
            .expect("EventSeq overflowed u64::MAX — implausible within a replicate");
        s
    }

    /// Peek the next sequence number without advancing the counter.
    /// Useful for round-number diagnostics; not for scheduling.
    pub fn peek_next(&self) -> EventSeq {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{AgentId, EventKind, SimTime};

    fn ev(at: SimTime, agent: AgentId, seq: EventSeq) -> Event {
        Event {
            at,
            agent,
            seq,
            kind: EventKind::Arrival,
        }
    }

    #[test]
    fn pop_drains_in_at_agent_seq_order() {
        // Input order is deliberately permuted across all three
        // axes of the tie-break key.
        let permuted = [
            ev(10, 5, 0),
            ev(0, 0, 1),
            ev(20, 0, 0),
            ev(0, 1, 5),
            ev(10, 0, 9),
            ev(0, 0, 0),
            ev(10, 0, 0),
            ev(0, 1, 0),
        ];
        let expected = vec![
            ev(0, 0, 0),
            ev(0, 0, 1),
            ev(0, 1, 0),
            ev(0, 1, 5),
            ev(10, 0, 0),
            ev(10, 0, 9),
            ev(10, 5, 0),
            ev(20, 0, 0),
        ];

        let mut q = EventQueue::new();
        for e in &permuted {
            q.push(*e);
        }
        assert_eq!(q.len(), permuted.len());

        let mut popped = Vec::new();
        while let Some(e) = q.pop() {
            popped.push(e);
        }
        assert_eq!(popped, expected);
        assert!(q.is_empty());
    }

    #[test]
    fn peek_matches_subsequent_pop() {
        let mut q = EventQueue::new();
        q.push(ev(5, 0, 0));
        q.push(ev(1, 0, 0));
        q.push(ev(1, 1, 0));

        for _ in 0..3 {
            let peeked = *q.peek().expect("non-empty");
            let popped = q.pop().expect("non-empty");
            assert_eq!(peeked, popped);
        }
        assert_eq!(q.peek(), None);
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn empty_queue_returns_none() {
        let mut q = EventQueue::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert_eq!(q.peek(), None);
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn seq_allocator_is_monotonic_and_unique() {
        let mut alloc = EventSeqAllocator::new();
        let drawn: Vec<EventSeq> = (0..1024).map(|_| alloc.allocate()).collect();

        // Strictly increasing.
        for window in drawn.windows(2) {
            assert!(window[0] < window[1]);
        }

        // Unique.
        let mut sorted = drawn.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), drawn.len());

        // Peek doesn't bump.
        let before = alloc.peek_next();
        let after = alloc.peek_next();
        assert_eq!(before, after);

        // Next picks up where peek left off.
        let issued = alloc.allocate();
        assert_eq!(issued, before);
        assert_eq!(alloc.peek_next(), before + 1);
    }

    #[test]
    fn queue_handles_simulator_style_workload() {
        // Mimics the intended usage pattern: a Simulator allocates
        // unique seq numbers, builds Events, pushes onto the
        // queue, and pops them strictly in (at, agent, seq) order.
        let mut alloc = EventSeqAllocator::new();
        let mut q = EventQueue::with_capacity(16);

        // Schedule a few events at the same simulator time for
        // different agents — exactly the scenario the tie-break
        // key resolves.
        for agent in [3, 1, 2, 1, 0, 2] {
            q.push(Event {
                at: 100,
                agent,
                seq: alloc.allocate(),
                kind: EventKind::Advertise,
            });
        }
        // …plus one earlier event and one later one.
        q.push(Event {
            at: 50,
            agent: 7,
            seq: alloc.allocate(),
            kind: EventKind::Arrival,
        });
        q.push(Event {
            at: 200,
            agent: 0,
            seq: alloc.allocate(),
            kind: EventKind::Completion,
        });

        let mut order = Vec::new();
        while let Some(e) = q.pop() {
            order.push((e.at, e.agent, e.seq));
        }

        // Earliest (at=50) first, then the (at=100) cluster in
        // (agent, seq) order, then (at=200).
        let times: Vec<SimTime> = order.iter().map(|(t, _, _)| *t).collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(order.first().map(|t| t.0), Some(50));
        assert_eq!(order.last().map(|t| t.0), Some(200));

        // Within at=100, agent ordering must be strictly ascending.
        let cluster: Vec<_> = order.iter().filter(|(t, _, _)| *t == 100).collect();
        let agents: Vec<AgentId> = cluster.iter().map(|(_, a, _)| *a).collect();
        let mut sorted_agents = agents.clone();
        sorted_agents.sort();
        assert_eq!(agents, sorted_agents);
    }
}
