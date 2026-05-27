//! Integration test for **T3.4** — calibration boundary reset
//! + outage scheduling.
//!
//! Acceptance gates (from #42):
//!
//! 1. A workflow arriving during an outage window is **deferred**
//!    (admission returns [`AdmissionRejected::InOutage`]).
//! 2. A workflow arriving immediately after a boundary reset
//!    sees the OU drift state back at **nominal fidelity** for
//!    every channel.
//!
//! The scenario:
//!
//! - Period = 100 s, outage = 10 s, anchor `μ` = 0.99 / 0.95 /
//!   0.98 across the 1-qubit / 2-qubit / readout channels.
//! - At `t = 50 s` (operational) → admit a circuit; drift
//!   advances 50 EM steps and lands well away from nominal.
//! - At `t = 95 s` (mid-outage) → admit rejected with `until =
//!   100 s`.
//! - At `t = 100 s` (immediately post-reset) → admit succeeds
//!   and every channel reads exactly `μ`.

use qwksim_core::rng::RngHierarchy;
use qwksim_qpu::{
    AdmissionRejected, CalibrationSchedule, CircuitExec, FidelityChannel, FidelityClass,
    IntegrationTightness, Modality, OuDriftState, OuParams, QpuAgent, QpuAnchor, OU_STEP_DT_NS,
};

const PERIOD_S: u64 = 100;
const OUTAGE_S: u64 = 10;
const NOMINAL_1Q: f64 = 0.99;
const NOMINAL_2Q: f64 = 0.95;
const NOMINAL_RO: f64 = 0.98;

fn sample_anchor() -> QpuAnchor {
    QpuAnchor {
        modality: Modality::Superconducting,
        qubits: 50,
        t1_ns: 80_000,
        t2_ns: 60_000,
        fidelity_1q: NOMINAL_1Q,
        fidelity_2q: NOMINAL_2Q,
        fidelity_readout: NOMINAL_RO,
        gate_time_1q_ns: 30,
        gate_time_2q_ns: 200,
        readout_time_ns: 1_000,
        calibration_period_ns: PERIOD_S * OU_STEP_DT_NS,
        calibration_outage_ns: OUTAGE_S * OU_STEP_DT_NS,
    }
}

fn build_agent() -> QpuAgent {
    let anchor = sample_anchor();
    let schedule = CalibrationSchedule::from_anchor(&anchor);
    let replicate = RngHierarchy::new(0xCAFE_F00D).replicate(0);
    let drift = OuDriftState::new(
        replicate,
        /* site_id */ 0,
        /* qpu_id  */ 0,
        /* t0_ns   */ 0,
        OU_STEP_DT_NS,
        &[
            (
                FidelityChannel::OneQubit,
                OuParams::new(0.3, NOMINAL_1Q, 0.005),
                NOMINAL_1Q,
            ),
            (
                FidelityChannel::TwoQubit,
                OuParams::new(0.3, NOMINAL_2Q, 0.01),
                NOMINAL_2Q,
            ),
            (
                FidelityChannel::Readout,
                OuParams::new(0.3, NOMINAL_RO, 0.005),
                NOMINAL_RO,
            ),
        ],
    );
    QpuAgent::new(0, anchor, IntegrationTightness::OnPrem).with_calibration(drift, schedule)
}

#[test]
fn workflow_during_outage_is_deferred_and_post_reset_workflow_sees_nominal() {
    let mut agent = build_agent();
    let schedule = agent
        .calibration_schedule()
        .expect("schedule attached in build_agent");

    let t_op = 50 * OU_STEP_DT_NS; // mid-cycle 0, operational.
    let t_outage = 95 * OU_STEP_DT_NS; // inside cycle 0's outage [90, 100).
    let t_reset = PERIOD_S * OU_STEP_DT_NS; // boundary at exactly 100 s.

    // Sanity-check the schedule layout before exercising admission.
    assert!(!schedule.is_in_outage(t_op));
    assert!(schedule.is_in_outage(t_outage));
    assert!(
        !schedule.is_in_outage(t_reset),
        "boundary reset reopens the operational window",
    );

    // 1. Operational admit: drift advances 50 EM steps.
    let r_op = agent.admit_circuit(CircuitExec::new(1, FidelityClass::Standard), t_op);
    assert!(r_op.is_ok(), "operational admit must succeed");

    // Drive the drift up to t_outage so the channels are far
    // from nominal before the boundary reset lands. Use the
    // drift's step-on-demand interface directly.
    let drift_mut = agent.drift_mut().expect("drift attached");
    let pre_reset_1q = drift_mut.current_state(FidelityChannel::OneQubit, t_outage);
    let pre_reset_2q = drift_mut.current_state(FidelityChannel::TwoQubit, t_outage);
    let pre_reset_ro = drift_mut.current_state(FidelityChannel::Readout, t_outage);
    // At least one channel will have drifted off nominal under
    // a 100-step OU trace at σ=0.005-0.01 — assert at least one
    // channel diverged so the post-reset comparison is
    // meaningful.
    assert!(
        pre_reset_1q != NOMINAL_1Q || pre_reset_2q != NOMINAL_2Q || pre_reset_ro != NOMINAL_RO,
        "all three channels stayed exactly at nominal — \
         seed is too quiet to demonstrate the post-reset gate",
    );

    // 2. Outage admit: rejected, with `until` pointing at the
    //    next boundary reset.
    let r_outage = agent.admit_circuit(CircuitExec::new(2, FidelityClass::Standard), t_outage);
    match r_outage {
        Err(AdmissionRejected::InOutage { until }) => {
            assert_eq!(
                until, t_reset,
                "InOutage::until must point at the next boundary reset"
            );
        }
        other => panic!("expected InOutage rejection, got {other:?}"),
    }
    // Queue depth must not have grown: outage admit was rejected.
    assert_eq!(agent.pending_circuits(), 1);

    // 3. Post-reset admit: succeeds, AND every channel reads
    //    exactly nominal.
    let r_reset = agent.admit_circuit(CircuitExec::new(3, FidelityClass::Standard), t_reset);
    assert!(r_reset.is_ok(), "post-reset admit must succeed");
    assert_eq!(agent.pending_circuits(), 2);

    let drift_mut = agent.drift_mut().expect("drift attached");
    // `current_state` at the reset boundary is the post-reset
    // value (no further EM steps consumed since `last_t_ns ==
    // t_reset` after the reset).
    let post_1q = drift_mut.current_state(FidelityChannel::OneQubit, t_reset);
    let post_2q = drift_mut.current_state(FidelityChannel::TwoQubit, t_reset);
    let post_ro = drift_mut.current_state(FidelityChannel::Readout, t_reset);
    assert_eq!(
        post_1q.to_bits(),
        NOMINAL_1Q.to_bits(),
        "1q channel must read nominal immediately after boundary reset",
    );
    assert_eq!(
        post_2q.to_bits(),
        NOMINAL_2Q.to_bits(),
        "2q channel must read nominal immediately after boundary reset",
    );
    assert_eq!(
        post_ro.to_bits(),
        NOMINAL_RO.to_bits(),
        "readout channel must read nominal immediately after boundary reset",
    );

    // Drift's recorded last_reset must match the boundary.
    assert_eq!(
        agent.drift().expect("drift attached").last_reset_at_ns(),
        t_reset,
    );
}

#[test]
fn admit_circuit_with_no_schedule_attached_is_always_operational() {
    // Regression guard: the T3.2 baseline (no calibration
    // attached) still admits everything.
    let anchor = sample_anchor();
    let mut agent = QpuAgent::new(0, anchor, IntegrationTightness::OnPrem);
    for t_ns in [0, 50_000, 95 * OU_STEP_DT_NS, 1_000_000_000_000] {
        let r = agent.admit_circuit(CircuitExec::new(t_ns, FidelityClass::Standard), t_ns);
        assert!(r.is_ok(), "admit at {t_ns} ns rejected without a schedule");
    }
}
