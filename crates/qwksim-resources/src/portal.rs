//! `PortalAgent` — the **admission seam** at the front of every
//! super-site. Workflows arrive at the portal, are tie-broken by
//! `(priority, arrival_seq)` per Q11.4 = att3 (priority class
//! distribution {low/normal/high} = 30/60/10), and then handed
//! off to the bargaining solver.
//!
//! Today the portal does only the bookkeeping needed for the
//! admission seam to be testable:
//!
//! - Track an `admit_seq` counter for FCFS-within-priority
//!   tie-breaking.
//! - Hold a pending-queue of admission requests not yet handed
//!   off to the bargaining solver.
//! - Emit an [`AdvertisedSummary`] carrying `portal_queue_depth`
//!   and `portal_high_priority_pending` so the bargaining
//!   utility can pressure-test the portal's load. Once the full
//!   workflow model lands in Phase 2, the portal will also carry
//!   fidelity-class hints; that field belongs in a later PR.
//!
//! `ResourceAgent::accept` / `release` are no-ops — the portal
//! itself holds no allocatable capacity; it owns only the
//! admission decision.

use std::collections::BinaryHeap;

use qwksim_core::event::{AgentId, SimTime};
use qwksim_scheduler::View;

use crate::{AdvertisedSummary, Allocation, ResourceAgent};

/// Workflow priority class (Q11.4 = att3). Ordering: `Low <
/// Normal < High`, so `High` wins admission tie-breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PriorityClass {
    /// 30 % of the headline workload by Q11.4's anchor mix.
    #[default]
    Low,
    /// 60 % of the headline workload.
    Normal,
    /// 10 % of the headline workload.
    High,
}

/// One workflow's arrival at the portal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionRequest {
    /// Identifier for the arriving workflow.
    pub workflow_id: u64,
    /// Simulator-time the workflow arrived at the portal.
    pub arrival_ns: SimTime,
    /// Declared priority class.
    pub priority: PriorityClass,
}

/// Result of the portal's tie-break decision for a single
/// workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionOutcome {
    /// Identifier of the admitted workflow.
    pub workflow_id: u64,
    /// Per-portal monotonic admission sequence number (`0` =
    /// first ever admission through this portal).
    pub admit_seq: u64,
}

/// Internal queue entry — a request plus the seq-counter value
/// the portal assigned when it received it. The tuple
/// `(priority, arrival_ns, receive_seq)` is the total order used
/// to drain the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueueEntry {
    request: AdmissionRequest,
    /// Sequence number at the moment of receipt. Used as the
    /// final tie-break key when priority and arrival_ns are both
    /// equal — guarantees a total order without relying on
    /// HashMap insertion order.
    receive_seq: u64,
}

impl QueueEntry {
    fn sort_key(&self) -> (i8, SimTime, u64) {
        // `i8` so we can negate priority for descending sort.
        // PriorityClass casts cleanly to u8; widen to i8 then
        // negate so `High` (= 2) becomes -2 < -1 (`Normal`) <
        // 0 (`Low`).
        let prio = -(self.request.priority as i8);
        (prio, self.request.arrival_ns, self.receive_seq)
    }
}

// PartialOrd / Ord on QueueEntry compare by *sort_key*. We
// implement them by hand rather than deriving so the
// (priority, arrival_ns, receive_seq) ordering is auditable in a
// single place. BinaryHeap is a max-heap; we want the head of the
// queue to be the *first* to admit, so we want sort_key
// comparisons reversed when stored in the heap — done via
// `std::cmp::Reverse` wrappers at the push site.
impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

/// Per-super-site portal agent.
#[derive(Debug)]
pub struct PortalAgent {
    id: AgentId,
    /// Next sequence number assigned on `receive`. Strictly
    /// increasing, so `(priority, arrival_ns, receive_seq)` is a
    /// total order.
    next_seq: u64,
    /// Pending admission queue, ordered by `(priority desc,
    /// arrival_ns asc, receive_seq asc)`.
    pending: BinaryHeap<std::cmp::Reverse<QueueEntry>>,
    /// Monotonic counter for `AdmissionOutcome::admit_seq` —
    /// distinct from `next_seq` (the receive counter): an
    /// `admit_seq` is issued at the moment a workflow is
    /// admitted, not received.
    next_admit_seq: u64,
    /// Cache of how many `High`-priority requests are currently
    /// in `pending`. Updated incrementally so
    /// `advertised_summary` is O(1).
    high_pending_count: u32,
}

impl PortalAgent {
    /// Build a fresh portal with no pending requests.
    pub fn new(id: AgentId) -> Self {
        Self {
            id,
            next_seq: 0,
            pending: BinaryHeap::new(),
            next_admit_seq: 0,
            high_pending_count: 0,
        }
    }

    /// How many requests are currently waiting at the portal.
    pub fn queue_depth(&self) -> u32 {
        self.pending.len() as u32
    }

    /// How many `High`-priority requests are currently waiting.
    pub fn high_priority_pending(&self) -> u32 {
        self.high_pending_count
    }

    /// Enqueue a new admission `request`. The request is *not*
    /// admitted yet — call [`Self::admit_next`] (one at a time)
    /// or [`Self::admit_all_ready`] (drain in priority order) to
    /// actually issue admissions.
    pub fn receive(&mut self, request: AdmissionRequest) {
        let receive_seq = self.next_seq;
        self.next_seq = self.next_seq.checked_add(1).expect(
            "PortalAgent::receive: receive_seq overflowed u64::MAX — implausible per replicate",
        );
        if request.priority == PriorityClass::High {
            self.high_pending_count = self.high_pending_count.saturating_add(1);
        }
        self.pending.push(std::cmp::Reverse(QueueEntry {
            request,
            receive_seq,
        }));
    }

    /// Pop and admit the single highest-priority pending request.
    /// Returns `None` if the queue is empty.
    pub fn admit_next(&mut self) -> Option<AdmissionOutcome> {
        let std::cmp::Reverse(entry) = self.pending.pop()?;
        if entry.request.priority == PriorityClass::High {
            self.high_pending_count = self.high_pending_count.saturating_sub(1);
        }
        let admit_seq = self.next_admit_seq;
        self.next_admit_seq = self
            .next_admit_seq
            .checked_add(1)
            .expect("PortalAgent::admit_next: admit_seq overflowed u64::MAX");
        Some(AdmissionOutcome {
            workflow_id: entry.request.workflow_id,
            admit_seq,
        })
    }

    /// Drain every pending request in priority order, returning
    /// the sequence of `AdmissionOutcome`s in admission order
    /// (the order each was issued by the portal).
    pub fn admit_all_ready(&mut self) -> Vec<AdmissionOutcome> {
        let mut out = Vec::with_capacity(self.pending.len());
        while let Some(o) = self.admit_next() {
            out.push(o);
        }
        out
    }
}

impl ResourceAgent for PortalAgent {
    fn id(&self) -> AgentId {
        self.id
    }

    fn advertised_summary(&self, _now: SimTime) -> AdvertisedSummary {
        AdvertisedSummary {
            portal_queue_depth: self.queue_depth(),
            portal_high_priority_pending: self.high_priority_pending(),
            ..Default::default()
        }
    }

    fn utility(&self, _alloc: &Allocation, _view: &View<'_>) -> f64 {
        // The portal has no allocatable capacity, but it
        // participates in the Nash product via a fixed neutral
        // utility today. FLAG-C will give it a meaningful
        // `α·deadline_slack` term in T4.x.
        1.0
    }

    fn accept(&mut self, _alloc: Allocation, _now: SimTime) {
        // The portal owns no capacity; it forwards admissions to
        // the bargaining solver via `admit_next`. The
        // ResourceAgent::accept/release surface is a no-op here.
    }

    fn release(&mut self, _alloc: &Allocation, _now: SimTime) {
        // See accept().
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: u64, at: SimTime, p: PriorityClass) -> AdmissionRequest {
        AdmissionRequest {
            workflow_id: id,
            arrival_ns: at,
            priority: p,
        }
    }

    #[test]
    fn empty_portal_advertises_zero_queue_depth() {
        let p = PortalAgent::new(0);
        assert_eq!(p.queue_depth(), 0);
        let s = p.advertised_summary(0);
        assert_eq!(s.portal_queue_depth, 0);
        assert_eq!(s.portal_high_priority_pending, 0);
    }

    #[test]
    fn priority_class_ordering_is_low_lt_normal_lt_high() {
        assert!(PriorityClass::Low < PriorityClass::Normal);
        assert!(PriorityClass::Normal < PriorityClass::High);
    }

    #[test]
    fn admit_next_drains_in_priority_then_arrival_order() {
        let mut p = PortalAgent::new(0);
        // All arrive at same instant; receive order doesn't
        // match priority — portal must still admit highest
        // priority first.
        p.receive(req(0, 100, PriorityClass::Normal));
        p.receive(req(1, 100, PriorityClass::Low));
        p.receive(req(2, 100, PriorityClass::High));
        p.receive(req(3, 100, PriorityClass::High));
        p.receive(req(4, 100, PriorityClass::Normal));
        assert_eq!(p.queue_depth(), 5);
        assert_eq!(p.high_priority_pending(), 2);

        let outcomes: Vec<u64> = p.admit_all_ready().iter().map(|o| o.workflow_id).collect();
        // Expected order:
        //  High receive_seq 2 → workflow 2
        //  High receive_seq 3 → workflow 3
        //  Normal receive_seq 0 → workflow 0
        //  Normal receive_seq 4 → workflow 4
        //  Low receive_seq 1 → workflow 1
        assert_eq!(outcomes, vec![2, 3, 0, 4, 1]);

        assert_eq!(p.queue_depth(), 0);
        assert_eq!(p.high_priority_pending(), 0);
    }

    #[test]
    fn admit_next_uses_arrival_ns_before_receive_seq_within_priority() {
        // All Normal priority. arrival_ns is the dominant key
        // *within* a priority class — receive_seq is only the
        // final tie-break for same-instant arrivals.
        let mut p = PortalAgent::new(0);
        p.receive(req(0, 50, PriorityClass::Normal));
        p.receive(req(1, 10, PriorityClass::Normal));
        p.receive(req(2, 30, PriorityClass::Normal));

        let order: Vec<u64> = p.admit_all_ready().iter().map(|o| o.workflow_id).collect();
        assert_eq!(order, vec![1, 2, 0]);
    }

    #[test]
    fn admission_outcomes_carry_monotonic_admit_seq() {
        let mut p = PortalAgent::new(0);
        for i in 0..5 {
            p.receive(req(i, 0, PriorityClass::Normal));
        }
        let outcomes = p.admit_all_ready();
        for (idx, o) in outcomes.iter().enumerate() {
            assert_eq!(o.admit_seq, idx as u64);
        }
    }

    #[test]
    fn high_priority_pending_count_tracks_queue_changes() {
        let mut p = PortalAgent::new(0);
        p.receive(req(0, 0, PriorityClass::High));
        p.receive(req(1, 0, PriorityClass::High));
        p.receive(req(2, 0, PriorityClass::Normal));
        assert_eq!(p.high_priority_pending(), 2);

        // Pop one High → count decrements.
        p.admit_next();
        assert_eq!(p.high_priority_pending(), 1);

        // Pop the other High → 0.
        p.admit_next();
        assert_eq!(p.high_priority_pending(), 0);

        // Pop Normal → still 0.
        p.admit_next();
        assert_eq!(p.high_priority_pending(), 0);
    }

    #[test]
    fn resource_agent_accept_and_release_are_no_ops() {
        let mut p = PortalAgent::new(0);
        let dummy = Allocation::default();
        p.accept(dummy, 0);
        p.release(&dummy, 0);
        assert_eq!(p.queue_depth(), 0);
    }
}
