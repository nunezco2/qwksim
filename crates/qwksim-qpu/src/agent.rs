//! `QpuAgent` — per-modality QPU resource agent (T3.2 skeleton).
//!
//! Tracks the `(anchor, tightness)` pair from T3.1 plus a circuit
//! priority queue ordered by [`FidelityClass`]. Implements the
//! [`ResourceAgent`] trait so the bargaining solver in Phase 4 can
//! consume it uniformly with HPC, GPU, and portal agents.
//!
//! The circuit queue is the agent's *workflow-facing* surface:
//! callers `enqueue` a [`CircuitExec`] and the agent's executor
//! pops the next one via [`QpuAgent::pop_next_circuit`]. The
//! `ResourceAgent` trait surface (`accept`, `release`) handles the
//! coarser bargaining-bundle reservation shape, which today
//! carries no QPU-specific fields on [`Allocation`]; the
//! per-circuit detail (qubits, shots, requested fidelity class)
//! flows through the queue.
//!
//! ## Ordering
//!
//! `FidelityClass` is `Low < Standard < High`. The queue pops
//! highest-fidelity first; **within a fidelity class** the queue
//! is FIFO by submission order (assigned monotonically by the
//! agent on `enqueue`). This is a strict (not weak) ordering, so
//! replay is byte-deterministic given a fixed enqueue sequence —
//! satisfying Q6′ = R2 single-machine deterministic replay (no
//! `HashMap` or randomised tie-break).
//!
//! ## What does *not* live here (yet)
//!
//! - OU calibration drift (T3.3).
//! - Calibration boundary and outage scheduling (T3.4).
//! - Compilation cache (T3.5).
//! - Mid-circuit feedback latency (T3.6).
//! - Calibration-aware utility term (T3.7).
//! - Vendor-data adapter (T3.8).
//!
//! Each lands in its own follow-up PR per the §13.3 task graph.

use std::collections::BTreeMap;

use qwksim_core::event::{AgentId, SimTime};
use qwksim_resources::{AdvertisedSummary, Allocation, ResourceAgent};
use qwksim_scheduler::View;

use crate::{IntegrationTightness, QpuAnchor};

/// Required circuit fidelity tier. The queue pops `High` before
/// `Standard` before `Low`; ties within a tier break FIFO by
/// submission order. The variant order is chosen so the derived
/// `Ord` impl reads as "Low < Standard < High" — the queue then
/// pops the *maximum* key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FidelityClass {
    /// Lowest fidelity tier — exploratory / sweep / pre-flight
    /// runs.
    Low,
    /// Default tier for headline workloads.
    Standard,
    /// Highest tier — reserved for runs whose downstream utility
    /// term penalises low fidelity heavily (e.g. headline VQE
    /// final iterations, QC-MC-QPE).
    High,
}

/// One circuit waiting (or executing) on a [`QpuAgent`]. Today
/// the descriptor is minimal: a stable submission id and the
/// requested fidelity class. Richer per-circuit fields (qubit
/// count, two-qubit gate count, shot count, deadline,
/// `mid_circuit_feedback`) land alongside the OU integrator and
/// compilation cache in T3.3+.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CircuitExec {
    /// Caller-stable circuit identifier (e.g. workflow id × task
    /// index, packed into a `u64`). Opaque to the agent.
    pub circuit_id: u64,
    /// Required fidelity tier.
    pub fidelity_class: FidelityClass,
}

impl CircuitExec {
    /// Convenience constructor.
    pub fn new(circuit_id: u64, fidelity_class: FidelityClass) -> Self {
        Self {
            circuit_id,
            fidelity_class,
        }
    }
}

/// Fidelity-class priority queue keyed by `(255 − class_rank,
/// submission_seq)`. `BTreeMap` iterates ascending, so the
/// smallest key is the *highest* fidelity with the *oldest*
/// submission sequence — exactly the next-to-dispatch circuit.
///
/// Kept private to the module; callers interact via
/// [`QpuAgent::enqueue_circuit`] / [`QpuAgent::pop_next_circuit`]
/// / [`QpuAgent::pending_circuits`].
type QueueKey = (u8, u64);

fn queue_key(class: FidelityClass, seq: u64) -> QueueKey {
    // Map class to a u8 such that *smaller* key means *higher*
    // priority for BTreeMap ascending iteration:
    //   High     -> 0
    //   Standard -> 1
    //   Low      -> 2
    let rank: u8 = match class {
        FidelityClass::High => 0,
        FidelityClass::Standard => 1,
        FidelityClass::Low => 2,
    };
    (rank, seq)
}

/// Per-modality QPU resource agent (T3.2 skeleton).
///
/// One per super-site in the headline configuration. Wraps the
/// T3.1 anchor + tightness pair with a deterministic
/// fidelity-class circuit queue. The OU drift, compilation
/// cache, and calibration scheduling listed in §2.3 land in the
/// T3.3+ follow-ups.
#[derive(Debug, Clone)]
pub struct QpuAgent {
    id: AgentId,
    anchor: QpuAnchor,
    tightness: IntegrationTightness,
    /// Pending circuits, keyed by `(class_rank, submission_seq)`.
    queue: BTreeMap<QueueKey, CircuitExec>,
    /// Monotonic counter assigned to each enqueued circuit so
    /// the second coordinate of the queue key is a strict
    /// FIFO-within-class tie-break.
    next_seq: u64,
}

impl QpuAgent {
    /// Build a fresh QPU agent with an empty fidelity-class
    /// queue.
    pub fn new(id: AgentId, anchor: QpuAnchor, tightness: IntegrationTightness) -> Self {
        Self {
            id,
            anchor,
            tightness,
            queue: BTreeMap::new(),
            next_seq: 0,
        }
    }

    /// Calibration anchor this agent was constructed with. Today
    /// the anchor is immutable per-agent; T3.4 will introduce
    /// calibration-boundary resets that swap the OU drift state
    /// while keeping the anchor itself stable.
    pub fn anchor(&self) -> &QpuAnchor {
        &self.anchor
    }

    /// Integration tightness this agent was constructed with.
    /// Today the tightness is immutable per-agent; per Q10.1 =
    /// h4 a single anchor can drive either `OnPrem` or `Cloud`
    /// by spinning up two agents with the same anchor and
    /// different tightness.
    pub fn tightness(&self) -> IntegrationTightness {
        self.tightness
    }

    /// Number of circuits currently waiting in the queue.
    pub fn pending_circuits(&self) -> usize {
        self.queue.len()
    }

    /// Enqueue `exec`. Returns the strictly-monotonic submission
    /// sequence number the queue used for FIFO-within-class
    /// tie-break.
    pub fn enqueue_circuit(&mut self, exec: CircuitExec) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .expect("QpuAgent submission sequence overflowed u64");
        let key = queue_key(exec.fidelity_class, seq);
        debug_assert!(
            !self.queue.contains_key(&key),
            "duplicate queue key (rank, seq)={key:?} — sequence allocation must be unique",
        );
        self.queue.insert(key, exec);
        seq
    }

    /// Pop the next circuit to dispatch. Returns `None` if the
    /// queue is empty. Order is strict: highest fidelity class
    /// first, then ascending submission sequence within the
    /// class.
    pub fn pop_next_circuit(&mut self) -> Option<CircuitExec> {
        let key = *self.queue.keys().next()?;
        self.queue.remove(&key)
    }

    /// Peek at the next circuit without removing it. Same
    /// ordering as [`Self::pop_next_circuit`].
    pub fn peek_next_circuit(&self) -> Option<&CircuitExec> {
        self.queue.values().next()
    }
}

impl ResourceAgent for QpuAgent {
    fn id(&self) -> AgentId {
        self.id
    }

    fn advertised_summary(&self, _now: SimTime) -> AdvertisedSummary {
        // QpuAgent does not own CPU cores, GPUs, or portal queue
        // slots. The QPU-specific advertised fields (pending
        // circuits, calibration-state, mean fidelity term) join
        // `AdvertisedSummary` once the bargaining utility wires
        // them in T4.x; today the summary returns the field-wise
        // `Default` so the trait is satisfied without lying to
        // peer agents about capacity that lives elsewhere.
        AdvertisedSummary::default()
    }

    fn utility(&self, _alloc: &Allocation, _view: &View<'_>) -> f64 {
        // FLAG-C utility for the QPU side (calibration-aware
        // fidelity term) lands as T3.7 + T4.2; today the stub
        // returns a constant so the trait is callable from
        // downstream tests and the bargaining-solver scaffold.
        1.0
    }

    fn accept(&mut self, _alloc: Allocation, _now: SimTime) {
        // The bargaining-bundle `Allocation` carries cores / gpus
        // only; QPU-specific reservation (shots, fidelity class,
        // qubit count) lands in T4.x. No-op until then.
    }

    fn release(&mut self, _alloc: &Allocation, _now: SimTime) {
        // Mirror of `accept`.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Modality;
    use qwksim_scheduler::{AdvertisedState, GlobalState, LocalState};

    fn sample_anchor() -> QpuAnchor {
        QpuAnchor {
            modality: Modality::Superconducting,
            qubits: 50,
            t1_ns: 80_000,
            t2_ns: 60_000,
            fidelity_1q: 0.999,
            fidelity_2q: 0.993,
            fidelity_readout: 0.985,
            gate_time_1q_ns: 30,
            gate_time_2q_ns: 200,
            readout_time_ns: 1_000,
            calibration_period_ns: 14_400_000_000_000,
            calibration_outage_ns: 900_000_000_000,
        }
    }

    #[test]
    fn fidelity_class_ord_is_low_lt_standard_lt_high() {
        assert!(FidelityClass::Low < FidelityClass::Standard);
        assert!(FidelityClass::Standard < FidelityClass::High);
    }

    #[test]
    fn fresh_agent_reports_empty_queue_and_zero_summary() {
        let a = QpuAgent::new(11, sample_anchor(), IntegrationTightness::OnPrem);
        assert_eq!(a.pending_circuits(), 0);
        assert!(a.peek_next_circuit().is_none());
        let s = a.advertised_summary(0);
        assert_eq!(s.total_cores, 0);
        assert_eq!(s.free_cores, 0);
        assert_eq!(s.total_gpus, 0);
        assert_eq!(s.free_gpus, 0);
        assert_eq!(a.id(), 11);
    }

    #[test]
    fn admits_jobs_in_fidelity_class_priority_order() {
        // T3.2 acceptance gate: enqueue circuits with mixed
        // fidelity classes in a non-trivial interleaving, then
        // pop them and assert the order is:
        //   1. all High (in FIFO submission order)
        //   2. all Standard (in FIFO submission order)
        //   3. all Low (in FIFO submission order)
        //
        // Enqueue order: A(Low), B(High), C(Standard), D(High),
        //                E(Low), F(Standard).
        // Expected pop order:
        //                B(High),    D(High),
        //                C(Standard), F(Standard),
        //                A(Low),     E(Low).
        let mut a = QpuAgent::new(0, sample_anchor(), IntegrationTightness::OnPrem);
        a.enqueue_circuit(CircuitExec::new(0xA, FidelityClass::Low));
        a.enqueue_circuit(CircuitExec::new(0xB, FidelityClass::High));
        a.enqueue_circuit(CircuitExec::new(0xC, FidelityClass::Standard));
        a.enqueue_circuit(CircuitExec::new(0xD, FidelityClass::High));
        a.enqueue_circuit(CircuitExec::new(0xE, FidelityClass::Low));
        a.enqueue_circuit(CircuitExec::new(0xF, FidelityClass::Standard));
        assert_eq!(a.pending_circuits(), 6);

        let mut popped: Vec<u64> = Vec::with_capacity(6);
        while let Some(exec) = a.pop_next_circuit() {
            popped.push(exec.circuit_id);
        }

        assert_eq!(
            popped,
            vec![0xB, 0xD, 0xC, 0xF, 0xA, 0xE],
            "queue must drain by descending fidelity class, FIFO within class",
        );
        assert_eq!(a.pending_circuits(), 0);
    }

    #[test]
    fn within_class_pops_fifo_by_submission_order() {
        let mut a = QpuAgent::new(2, sample_anchor(), IntegrationTightness::OnPrem);
        for id in [100u64, 200, 300, 400] {
            a.enqueue_circuit(CircuitExec::new(id, FidelityClass::Standard));
        }
        let mut popped = Vec::new();
        while let Some(exec) = a.pop_next_circuit() {
            popped.push(exec.circuit_id);
        }
        assert_eq!(
            popped,
            vec![100, 200, 300, 400],
            "single-class queue must drain in submission order",
        );
    }

    #[test]
    fn peek_does_not_consume_the_queue() {
        let mut a = QpuAgent::new(3, sample_anchor(), IntegrationTightness::OnPrem);
        a.enqueue_circuit(CircuitExec::new(1, FidelityClass::Low));
        a.enqueue_circuit(CircuitExec::new(2, FidelityClass::High));
        assert_eq!(a.pending_circuits(), 2);

        let peek = *a.peek_next_circuit().expect("non-empty");
        assert_eq!(peek.circuit_id, 2);
        assert_eq!(peek.fidelity_class, FidelityClass::High);
        // Peek must NOT have consumed anything.
        assert_eq!(a.pending_circuits(), 2);

        let popped = a.pop_next_circuit().expect("non-empty");
        assert_eq!(popped, peek, "pop returns what peek showed");
        assert_eq!(a.pending_circuits(), 1);
    }

    #[test]
    fn anchor_and_tightness_round_trip_through_constructor() {
        let anchor = sample_anchor();
        let a = QpuAgent::new(5, anchor, IntegrationTightness::Cloud);
        assert_eq!(*a.anchor(), anchor);
        assert_eq!(a.tightness(), IntegrationTightness::Cloud);
    }

    #[test]
    fn resource_agent_accept_release_are_no_ops_today() {
        // T3.2 does NOT route QPU reservation through the
        // bargaining-bundle Allocation. The trait surface must
        // be callable (for the bargaining solver scaffold) but
        // must not perturb the circuit queue.
        let mut a = QpuAgent::new(7, sample_anchor(), IntegrationTightness::OnPrem);
        a.enqueue_circuit(CircuitExec::new(1, FidelityClass::High));
        let alloc = Allocation { cores: 4, gpus: 2 };
        a.accept(alloc, 100);
        assert_eq!(
            a.pending_circuits(),
            1,
            "ResourceAgent::accept must not touch the QPU queue",
        );
        a.release(&alloc, 200);
        assert_eq!(
            a.pending_circuits(),
            1,
            "ResourceAgent::release must not touch the QPU queue",
        );
        // Queue still drains the same circuit.
        let exec = a.pop_next_circuit().expect("non-empty");
        assert_eq!(exec.circuit_id, 1);
    }

    #[test]
    fn utility_runs_under_both_view_variants() {
        let a = QpuAgent::new(9, sample_anchor(), IntegrationTightness::OnPrem);
        let g = GlobalState;
        let l = LocalState;
        let ad = AdvertisedState;
        let alloc = Allocation { cores: 0, gpus: 0 };

        let u_oracular = a.utility(&alloc, &View::Oracular(&g));
        let u_local = a.utility(
            &alloc,
            &View::Local {
                local: &l,
                advertised: &ad,
            },
        );
        assert!(u_oracular.is_finite());
        assert!(u_local.is_finite());
    }

    #[test]
    fn enqueue_returns_monotonic_submission_sequences() {
        let mut a = QpuAgent::new(13, sample_anchor(), IntegrationTightness::OnPrem);
        let s0 = a.enqueue_circuit(CircuitExec::new(0, FidelityClass::Low));
        let s1 = a.enqueue_circuit(CircuitExec::new(1, FidelityClass::High));
        let s2 = a.enqueue_circuit(CircuitExec::new(2, FidelityClass::Standard));
        assert_eq!(s0, 0);
        assert_eq!(s1, 1);
        assert_eq!(s2, 2);
    }
}
