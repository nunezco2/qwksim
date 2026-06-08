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

use crate::cache::CompilationCache;
use crate::calibration::{AdmissionRejected, CalibrationSchedule};
use crate::drift::OuDriftState;
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

/// One circuit waiting (or executing) on a [`QpuAgent`].
/// Carries the stable submission id, the required fidelity
/// tier, and the [`mid_circuit_feedback`](Self::mid_circuit_feedback)
/// flag that toggles the per-modality feedback-latency constant
/// from T3.6. Richer per-circuit fields (qubit count, two-qubit
/// gate count, shot count, deadline) land alongside the
/// Phase-4 bargaining wiring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CircuitExec {
    /// Caller-stable circuit identifier (e.g. workflow id × task
    /// index, packed into a `u64`). Opaque to the agent.
    pub circuit_id: u64,
    /// Required fidelity tier.
    pub fidelity_class: FidelityClass,
    /// `true` iff the circuit uses mid-circuit measurement +
    /// classical-feedback conditional gates. When set,
    /// [`QpuAgent::circuit_exec_time_ns`] adds the per-modality
    /// feedback-latency constant from
    /// [`Modality::mid_circuit_feedback_latency_ns`](crate::Modality::mid_circuit_feedback_latency_ns)
    /// to the circuit's base execution time.
    pub mid_circuit_feedback: bool,
}

impl CircuitExec {
    /// Convenience constructor — defaults `mid_circuit_feedback`
    /// to `false`. Use [`Self::with_mid_circuit_feedback`] to
    /// toggle the flag.
    pub fn new(circuit_id: u64, fidelity_class: FidelityClass) -> Self {
        Self {
            circuit_id,
            fidelity_class,
            mid_circuit_feedback: false,
        }
    }

    /// Set the mid-circuit feedback flag. Returns `self` for
    /// builder-style construction.
    pub fn with_mid_circuit_feedback(mut self, on: bool) -> Self {
        self.mid_circuit_feedback = on;
        self
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
    /// OU calibration drift integrator (T3.3). Optional so
    /// T3.2-era constructors continue to work; T3.4 attaches one
    /// via [`QpuAgent::with_calibration`].
    drift: Option<OuDriftState>,
    /// Calibration cycle + outage timing (T3.4). When `Some`,
    /// [`Self::admit_circuit`] rejects requests inside an outage
    /// window and applies the boundary-reset to `drift` lazily
    /// on each admission.
    calibration: Option<CalibrationSchedule>,
    /// Per-QPU compilation cache (T3.5). Always present (an
    /// empty cache is harmless); sweep-invalidated by
    /// [`Self::admit_circuit`] on every calibration boundary
    /// reset.
    cache: CompilationCache,
    /// Simulator time of the most recent calibration-boundary
    /// reset the **cache** has acknowledged. Tracked separately
    /// from `drift.last_reset_at_ns` so a QpuAgent without an
    /// attached drift (e.g. a Phase-2-era stub) still
    /// invalidates the cache exactly once per boundary.
    cache_last_reset_ns: SimTime,
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
            drift: None,
            calibration: None,
            cache: CompilationCache::new(),
            cache_last_reset_ns: 0,
        }
    }

    /// Read-only access to the compilation cache.
    pub fn cache(&self) -> &CompilationCache {
        &self.cache
    }

    /// Mutable access to the compilation cache (e.g. the
    /// per-iteration runtime calls `cache_mut().compile_or_cache`
    /// for each quantum task).
    pub fn cache_mut(&mut self) -> &mut CompilationCache {
        &mut self.cache
    }

    /// Attach an OU drift integrator + calibration schedule.
    /// Once attached, [`Self::admit_circuit`] enforces outage
    /// admission and applies abrupt boundary resets to `drift`
    /// on every cycle (T3.4).
    pub fn with_calibration(mut self, drift: OuDriftState, schedule: CalibrationSchedule) -> Self {
        self.drift = Some(drift);
        self.calibration = Some(schedule);
        self
    }

    /// Read-only access to the OU drift integrator. `None` if
    /// none was attached.
    pub fn drift(&self) -> Option<&OuDriftState> {
        self.drift.as_ref()
    }

    /// Mutable access to the OU drift integrator (e.g. for
    /// stepping it forward in tests / per-iteration code).
    pub fn drift_mut(&mut self) -> Option<&mut OuDriftState> {
        self.drift.as_mut()
    }

    /// The attached calibration schedule, if any.
    pub fn calibration_schedule(&self) -> Option<CalibrationSchedule> {
        self.calibration
    }

    /// Outage-aware admission. If a [`CalibrationSchedule`] is
    /// attached:
    ///
    /// - During an outage window the request is rejected with
    ///   [`AdmissionRejected::InOutage`]. The caller resubmits
    ///   once `t_ns >= until`.
    /// - Otherwise the drift integrator is brought up to date
    ///   with any boundary resets that have elapsed since the
    ///   last admission, and the circuit is enqueued.
    ///
    /// If no schedule is attached the admission always succeeds
    /// (matches the T3.2 baseline).
    pub fn admit_circuit(
        &mut self,
        exec: CircuitExec,
        now: SimTime,
    ) -> Result<u64, AdmissionRejected> {
        if let Some(schedule) = self.calibration {
            if schedule.is_in_outage(now) {
                return Err(AdmissionRejected::InOutage {
                    until: schedule.next_reset_after(now),
                });
            }
            let most_recent = schedule.most_recent_reset_at(now);
            let needs_reset = match self.drift.as_ref() {
                Some(drift) => drift.last_reset_at_ns() < most_recent,
                // No drift attached: still sweep the cache the
                // first time we cross any boundary, but never on
                // calls before the first boundary (`most_recent == 0`
                // would otherwise sweep on every call before the
                // first cycle elapses).
                None => most_recent > 0 && self.cache_last_reset_ns < most_recent,
            };
            if needs_reset {
                if let Some(drift) = self.drift.as_mut() {
                    drift.reset_to_nominal_at(most_recent);
                }
                self.cache.invalidate_all();
                self.cache_last_reset_ns = most_recent;
            }
        }
        Ok(self.enqueue_circuit(exec))
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

    /// Resolve the total execution time of `exec` given its
    /// `base_exec_ns` body duration (the modality-specific
    /// gate-time × circuit-shape product). When the circuit has
    /// `mid_circuit_feedback = true`, the per-modality feedback
    /// latency from
    /// [`Modality::mid_circuit_feedback_latency_ns`](crate::Modality::mid_circuit_feedback_latency_ns)
    /// is added on top (T3.6). When `false`, the base body
    /// duration is returned unchanged.
    ///
    /// Saturating-add guards against overflow when the base
    /// duration is pathologically large; in practice headline
    /// circuit times are nanosecond- to millisecond-scale and
    /// cannot overflow `u64`.
    pub fn circuit_exec_time_ns(&self, exec: &CircuitExec, base_exec_ns: SimTime) -> SimTime {
        if exec.mid_circuit_feedback {
            base_exec_ns.saturating_add(self.anchor.modality.mid_circuit_feedback_latency_ns())
        } else {
            base_exec_ns
        }
    }

    /// Placeholder utility term `∈ [0, 1]` combining the OU
    /// drift state and a calibration-recency decay (T3.7). The
    /// Phase-4 utility function (FLAG-C, T4.2) wires this
    /// directly; today the bargaining solver scaffold consumes
    /// this as the QPU's `β·realised_fidelity` contribution.
    ///
    /// ## Definition
    ///
    /// Returns `drift_term × recency_factor`, clamped to
    /// `[0, 1]`.
    ///
    /// - **drift_term**: product of the per-channel OU-drift
    ///   values relevant to the circuit, each clamped to
    ///   `[0, 1]`. When `circuit.mid_circuit_feedback` is true,
    ///   the readout channel is included; otherwise only the
    ///   1q and 2q channels contribute.
    /// - **recency_factor**: `exp(−λ · Δt_since_cal)` where
    ///   `Δt_since_cal = t_ns − calibration_schedule.most_recent_reset_at(t_ns)`
    ///   and `λ = ln 2 / (period / 2)` — i.e. the recency
    ///   halves each half-period. Returns `1.0` when no
    ///   calibration schedule is attached.
    ///
    /// ## Monotonicity (T3.7 acceptance gate)
    ///
    /// Within a single calibration cycle, with the OU noise
    /// scale `σ = 0` so the drift component is constant, the
    /// term is **strictly decreasing** in `t_ns` because the
    /// recency factor is strictly decreasing. Across cycle
    /// boundaries the recency snaps back to `1.0` and the
    /// monotonicity restarts.
    ///
    /// ## Side effects
    ///
    /// Advances the attached [`OuDriftState`] forward to `t_ns`
    /// (step-on-demand semantics from T3.3). Successive calls
    /// must have non-decreasing `t_ns` or the OU integrator
    /// panics.
    pub fn fidelity_term_at(&mut self, circuit: &CircuitExec, t_ns: SimTime) -> f64 {
        let recency = self.calibration_recency_at(t_ns);
        let f_1q = self
            .read_channel_clamped(crate::drift::FidelityChannel::OneQubit, t_ns)
            .unwrap_or(self.anchor.fidelity_1q.clamp(0.0, 1.0));
        let f_2q = self
            .read_channel_clamped(crate::drift::FidelityChannel::TwoQubit, t_ns)
            .unwrap_or(self.anchor.fidelity_2q.clamp(0.0, 1.0));
        let drift_term = if circuit.mid_circuit_feedback {
            let f_ro = self
                .read_channel_clamped(crate::drift::FidelityChannel::Readout, t_ns)
                .unwrap_or(self.anchor.fidelity_readout.clamp(0.0, 1.0));
            f_1q * f_2q * f_ro
        } else {
            f_1q * f_2q
        };
        (drift_term * recency).clamp(0.0, 1.0)
    }

    /// Read one OU-drift channel value at `t_ns`, clamped to
    /// `[0, 1]`. Returns `None` if no drift is attached.
    fn read_channel_clamped(
        &mut self,
        channel: crate::drift::FidelityChannel,
        t_ns: SimTime,
    ) -> Option<f64> {
        let drift = self.drift.as_mut()?;
        Some(drift.current_state(channel, t_ns).clamp(0.0, 1.0))
    }

    /// Compute `exp(−λ · Δt_since_cal)` where the half-life is
    /// half of the attached calibration period. Returns `1.0`
    /// when no schedule is attached.
    fn calibration_recency_at(&self, t_ns: SimTime) -> f64 {
        let Some(schedule) = self.calibration else {
            return 1.0;
        };
        let last_reset = schedule.most_recent_reset_at(t_ns);
        let dt_since = t_ns.saturating_sub(last_reset);
        let half_life_ns = (schedule.period_ns() / 2).max(1);
        let lambda = std::f64::consts::LN_2 / (half_life_ns as f64);
        (-lambda * dt_since as f64).exp()
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

    fn anchor_with_modality(modality: Modality) -> QpuAnchor {
        QpuAnchor {
            modality,
            ..sample_anchor()
        }
    }

    #[test]
    fn circuit_exec_defaults_mid_circuit_feedback_off() {
        // T3.2 constructor compatibility: CircuitExec::new
        // leaves the new T3.6 flag off by default so existing
        // callers stay untouched.
        let exec = CircuitExec::new(42, FidelityClass::Standard);
        assert!(!exec.mid_circuit_feedback);
    }

    #[test]
    fn with_mid_circuit_feedback_toggles_the_flag() {
        let off = CircuitExec::new(1, FidelityClass::High);
        let on = CircuitExec::new(1, FidelityClass::High).with_mid_circuit_feedback(true);
        let off_again = on.with_mid_circuit_feedback(false);
        assert!(!off.mid_circuit_feedback);
        assert!(on.mid_circuit_feedback);
        assert!(!off_again.mid_circuit_feedback);
    }

    #[test]
    fn circuit_exec_time_ns_returns_base_when_feedback_off() {
        let a = QpuAgent::new(0, sample_anchor(), IntegrationTightness::OnPrem);
        let exec = CircuitExec::new(0, FidelityClass::Standard);
        assert!(!exec.mid_circuit_feedback);
        let base: SimTime = 12_345;
        assert_eq!(
            a.circuit_exec_time_ns(&exec, base),
            base,
            "feedback off must return base unchanged",
        );
    }

    /// T3.6 acceptance gate: when `mid_circuit_feedback = true`,
    /// the QPU adds *exactly* the per-modality constant from
    /// [`Modality::mid_circuit_feedback_latency_ns`] to the
    /// circuit's base execution time.
    #[test]
    fn circuit_exec_time_ns_adds_per_modality_constant_when_feedback_on() {
        let base: SimTime = 50_000;
        for modality in [
            Modality::Superconducting,
            Modality::TrappedIon,
            Modality::Photonic,
        ] {
            let anchor = anchor_with_modality(modality);
            let a = QpuAgent::new(0, anchor, IntegrationTightness::OnPrem);
            let exec = CircuitExec::new(0, FidelityClass::Standard).with_mid_circuit_feedback(true);
            let got = a.circuit_exec_time_ns(&exec, base);
            let want = base + modality.mid_circuit_feedback_latency_ns();
            assert_eq!(
                got,
                want,
                "{modality:?}: total exec time {got} ≠ base {base} + feedback {}",
                modality.mid_circuit_feedback_latency_ns(),
            );
        }
    }

    #[test]
    fn circuit_exec_time_ns_is_independent_of_base_under_feedback_on() {
        // The added constant is the same regardless of base body
        // duration — the per-modality constant is added *on top*.
        let anchor = anchor_with_modality(Modality::TrappedIon);
        let a = QpuAgent::new(0, anchor, IntegrationTightness::OnPrem);
        let exec = CircuitExec::new(0, FidelityClass::Standard).with_mid_circuit_feedback(true);
        let constant = Modality::TrappedIon.mid_circuit_feedback_latency_ns();
        for &base in &[0_u64, 1, 1_000, 1_000_000, 1_000_000_000] {
            assert_eq!(a.circuit_exec_time_ns(&exec, base), base + constant);
        }
    }

    /// Build a QpuAgent wired with a short calibration cycle
    /// (period 100 s, outage 10 s) and a **noiseless** OU drift
    /// (σ = 0 on every channel) so the drift component stays
    /// exactly at the anchor's nominal fidelity — the perfect
    /// fixture for the T3.7 monotonicity gate, which needs the
    /// drift term to be constant so the recency factor's
    /// monotonicity dominates.
    fn agent_with_short_calibration_and_zero_noise() -> QpuAgent {
        use crate::drift::{FidelityChannel, OuDriftState, OuParams, OU_STEP_DT_NS};
        use crate::CalibrationSchedule;
        use qwksim_core::rng::RngHierarchy;

        let mut anchor = sample_anchor();
        // Override the calibration cadence so the test runs
        // inside one cycle.
        anchor.calibration_period_ns = 100 * OU_STEP_DT_NS;
        anchor.calibration_outage_ns = 10 * OU_STEP_DT_NS;

        let schedule = CalibrationSchedule::from_anchor(&anchor);
        let replicate = RngHierarchy::new(0xBEEF_BABE).replicate(0);
        let drift = OuDriftState::new(
            replicate,
            0,
            0,
            0,
            OU_STEP_DT_NS,
            &[
                (
                    FidelityChannel::OneQubit,
                    OuParams::new(0.3, anchor.fidelity_1q, 0.0),
                    anchor.fidelity_1q,
                ),
                (
                    FidelityChannel::TwoQubit,
                    OuParams::new(0.3, anchor.fidelity_2q, 0.0),
                    anchor.fidelity_2q,
                ),
                (
                    FidelityChannel::Readout,
                    OuParams::new(0.3, anchor.fidelity_readout, 0.0),
                    anchor.fidelity_readout,
                ),
            ],
        );
        QpuAgent::new(0, anchor, IntegrationTightness::OnPrem).with_calibration(drift, schedule)
    }

    /// T3.7 acceptance gate: with a noiseless drift, the
    /// fidelity term is **strictly decreasing** in
    /// time-since-calibration within a single cycle (the
    /// recency factor decays monotonically).
    #[test]
    fn fidelity_term_strictly_decreases_in_time_since_calibration() {
        use crate::OU_STEP_DT_NS;
        let mut a = agent_with_short_calibration_and_zero_noise();
        let circuit = CircuitExec::new(0, FidelityClass::Standard);

        // Sample at t = 0, 10, 20, …, 80 s (all inside cycle 0
        // and before the outage at t ≥ 90 s).
        let mut prev = f64::INFINITY;
        for k in 0..=8u64 {
            let t_ns = k * 10 * OU_STEP_DT_NS;
            let term = a.fidelity_term_at(&circuit, t_ns);
            assert!(
                (0.0..=1.0).contains(&term),
                "term {term} out of [0, 1] at t={t_ns}",
            );
            if k > 0 {
                assert!(
                    term < prev,
                    "monotonicity violated at k={k}: term {term} ≥ previous {prev}",
                );
            }
            prev = term;
        }
    }

    #[test]
    fn fidelity_term_jumps_back_up_after_boundary_reset() {
        use crate::OU_STEP_DT_NS;
        let mut a = agent_with_short_calibration_and_zero_noise();
        let circuit = CircuitExec::new(0, FidelityClass::Standard);
        // Just before the next reset boundary: late in cycle 0.
        let t_pre = 89 * OU_STEP_DT_NS;
        let term_pre = a.fidelity_term_at(&circuit, t_pre);
        // Exactly at the boundary (start of cycle 1).
        let t_post = 100 * OU_STEP_DT_NS;
        let term_post = a.fidelity_term_at(&circuit, t_post);
        assert!(
            term_post > term_pre,
            "post-boundary term {term_post} must exceed pre-boundary term {term_pre}",
        );
    }

    #[test]
    fn fidelity_term_with_no_schedule_uses_only_drift_product() {
        // No schedule attached → recency = 1.0; term = product
        // of drift channels (or anchor fallbacks since no drift
        // either).
        let a = QpuAgent::new(0, sample_anchor(), IntegrationTightness::OnPrem);
        // Need mutable agent for the call.
        let mut a = a;
        let circuit = CircuitExec::new(0, FidelityClass::Standard);
        let term = a.fidelity_term_at(&circuit, 0);
        // Sample anchor: 1q=0.999, 2q=0.993.
        let expected = sample_anchor().fidelity_1q * sample_anchor().fidelity_2q;
        assert!((term - expected).abs() < 1e-12);
    }

    #[test]
    fn fidelity_term_with_mid_circuit_feedback_includes_readout() {
        // With feedback=true the readout channel multiplies in;
        // since readout < 1, term must be ≤ no-feedback variant.
        let mut a = agent_with_short_calibration_and_zero_noise();
        let no_fb = CircuitExec::new(0, FidelityClass::Standard);
        let with_fb = CircuitExec::new(1, FidelityClass::Standard).with_mid_circuit_feedback(true);
        let t_ns = 0;
        let term_no = a.fidelity_term_at(&no_fb, t_ns);
        let term_fb = a.fidelity_term_at(&with_fb, t_ns);
        assert!(
            term_fb <= term_no,
            "with-feedback term {term_fb} should be ≤ no-feedback term {term_no} since readout ∈ [0, 1]",
        );
        assert!(term_fb > 0.0);
        assert!(term_no > 0.0);
    }

    #[test]
    fn fidelity_term_clamps_to_zero_one_under_pathological_anchor() {
        // Build an agent with an anchor whose fidelity fields
        // are out of range (e.g. 1.5 — pathological config) and
        // assert the term still lands in [0, 1] because the
        // public function clamps.
        let mut anchor = sample_anchor();
        anchor.fidelity_1q = 1.5;
        anchor.fidelity_2q = -0.5;
        let mut a = QpuAgent::new(0, anchor, IntegrationTightness::OnPrem);
        let circuit = CircuitExec::new(0, FidelityClass::Standard);
        let term = a.fidelity_term_at(&circuit, 0);
        assert!((0.0..=1.0).contains(&term));
    }
}
