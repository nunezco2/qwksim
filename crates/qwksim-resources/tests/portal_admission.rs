//! Integration test for **T2.5** — portal admission tie-break
//! under two simultaneous arrivals.
//!
//! The portal sits at the front of every super-site and decides
//! the order in which concurrent arrivals are handed off to the
//! bargaining solver. Two workflows arriving at the *same*
//! `arrival_ns` with different priorities must be admitted in
//! priority order; two with the *same* priority must be admitted
//! in `receive_seq` order (FCFS within the priority class).

use qwksim_resources::{AdmissionRequest, PortalAgent, PriorityClass, ResourceAgent};

fn req(id: u64, at: u64, p: PriorityClass) -> AdmissionRequest {
    AdmissionRequest {
        workflow_id: id,
        arrival_ns: at,
        priority: p,
    }
}

#[test]
fn two_simultaneous_arrivals_high_beats_normal() {
    let mut p = PortalAgent::new(0);
    // Both arrive at simulator-time 100. Receive order matches
    // the user's submission order (Normal first, then High) — the
    // portal must still admit High first.
    p.receive(req(/* Normal */ 0, 100, PriorityClass::Normal));
    p.receive(req(/* High */ 1, 100, PriorityClass::High));

    let outcomes = p.admit_all_ready();
    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0].workflow_id, 1, "High admitted first");
    assert_eq!(outcomes[1].workflow_id, 0, "Normal admitted second");
    // Monotonic admit_seq.
    assert_eq!(outcomes[0].admit_seq, 0);
    assert_eq!(outcomes[1].admit_seq, 1);
}

#[test]
fn two_simultaneous_arrivals_same_priority_use_receive_order() {
    let mut p = PortalAgent::new(0);
    p.receive(req(/* normal-first */ 0, 100, PriorityClass::Normal));
    p.receive(req(/* normal-second */ 1, 100, PriorityClass::Normal));

    let outcomes = p.admit_all_ready();
    assert_eq!(
        outcomes.iter().map(|o| o.workflow_id).collect::<Vec<_>>(),
        vec![0, 1],
        "FCFS within priority class"
    );
}

#[test]
fn two_simultaneous_arrivals_advertised_summary_carries_pending_counts() {
    let mut p = PortalAgent::new(0);
    p.receive(req(0, 100, PriorityClass::Normal));
    p.receive(req(1, 100, PriorityClass::High));

    let s = p.advertised_summary(100);
    assert_eq!(s.portal_queue_depth, 2);
    assert_eq!(s.portal_high_priority_pending, 1);
    // Resource-typed fields on the same summary stay at Default
    // (the portal owns no cores / GPUs / scratch / network).
    assert_eq!(s.free_cores, 0);
    assert_eq!(s.total_cores, 0);
    assert_eq!(s.free_gpus, 0);
    assert_eq!(s.total_gpus, 0);
}
