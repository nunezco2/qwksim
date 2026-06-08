//! Integration tests for **T3.5** — `CompilationCache` with
//! calibration-boundary invalidation.
//!
//! Acceptance gates (from #43):
//!
//! 1. A 50-iteration VQE workflow shows ≥98% hit rate **after
//!    iteration 1** (i.e. iterations 1..50 reuse the iteration-0
//!    compiled artefact).
//! 2. Crossing a calibration boundary sweep-invalidates the
//!    cache — a workflow continuing after a reset incurs a fresh
//!    miss before recovering its hit streak.

use qwksim_core::rng::RngHierarchy;
use qwksim_qpu::{
    CalibrationSchedule, CircuitExec, CompilationCacheKey, FidelityChannel, FidelityClass,
    IntegrationTightness, Modality, OuDriftState, OuParams, QpuAgent, QpuAnchor, OU_STEP_DT_NS,
};

const PERIOD_S: u64 = 100;
const OUTAGE_S: u64 = 10;
const NOMINAL_1Q: f64 = 0.99;
const NOMINAL_2Q: f64 = 0.95;
const NOMINAL_RO: f64 = 0.98;
const COMPILE_COST_NS: u64 = 1_000_000;

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

fn build_agent_with_calibration() -> QpuAgent {
    let anchor = sample_anchor();
    let schedule = CalibrationSchedule::from_anchor(&anchor);
    let replicate = RngHierarchy::new(0xDEAD_BEEF).replicate(0);
    let drift = OuDriftState::new(
        replicate,
        0,
        0,
        0,
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
    QpuAgent::new(7, anchor, IntegrationTightness::OnPrem).with_calibration(drift, schedule)
}

#[test]
fn vqe_workflow_50_iterations_hits_at_least_98_percent_after_iter_1() {
    // Stays well inside the first calibration cycle so no
    // boundary reset interferes with the warm-cache streak.
    let mut agent = build_agent_with_calibration();
    let key = CompilationCacheKey {
        template_id: 42,
        params_hash: 0xCAFE_BABE,
        qpu_id: 7,
    };

    let iter_dt_ns = OU_STEP_DT_NS; // 1 s per iteration → 50 s total.
    let mut hits_after_iter_1: u64 = 0;
    let mut total_after_iter_1: u64 = 0;
    for i in 0..50u64 {
        let now = i * iter_dt_ns;
        agent
            .admit_circuit(CircuitExec::new(i, FidelityClass::Standard), now)
            .expect("operational admit");
        let outcome = agent
            .cache_mut()
            .compile_or_cache(key, COMPILE_COST_NS, now);
        if i >= 1 {
            total_after_iter_1 += 1;
            if outcome.is_hit() {
                hits_after_iter_1 += 1;
            }
        }
    }

    let rate = hits_after_iter_1 as f64 / total_after_iter_1 as f64;
    assert!(
        rate >= 0.98,
        "post-iter-1 hit rate {:.4} < 0.98 (hits {} / total {})",
        rate,
        hits_after_iter_1,
        total_after_iter_1,
    );
    assert_eq!(
        agent.cache().misses(),
        1,
        "cold-start expects exactly one cache miss (iteration 0)",
    );
    assert_eq!(agent.cache().hits(), 49);
    assert_eq!(agent.cache().invalidations(), 0);
}

#[test]
fn calibration_boundary_invalidates_the_cache_via_admit_circuit() {
    // Warm the cache inside cycle 0, then cross the cycle
    // boundary via admit_circuit and assert:
    //   - cache.invalidations bumped exactly once,
    //   - first lookup after the boundary is a fresh miss.
    let mut agent = build_agent_with_calibration();
    let key = CompilationCacheKey {
        template_id: 1,
        params_hash: 1,
        qpu_id: 7,
    };

    // Pre-boundary: 5 iterations inside cycle 0.
    for i in 0..5u64 {
        let now = i * OU_STEP_DT_NS;
        agent
            .admit_circuit(CircuitExec::new(i, FidelityClass::Standard), now)
            .expect("op admit");
        agent
            .cache_mut()
            .compile_or_cache(key, COMPILE_COST_NS, now);
    }
    assert_eq!(agent.cache().misses(), 1, "iter 0 = cold miss");
    assert_eq!(agent.cache().hits(), 4, "iters 1..5 = warm hits");
    assert_eq!(agent.cache().invalidations(), 0);

    // Cross the boundary: admit at t = period (start of cycle 1).
    let t_reset = PERIOD_S * OU_STEP_DT_NS;
    agent
        .admit_circuit(CircuitExec::new(99, FidelityClass::Standard), t_reset)
        .expect("post-reset op admit");
    assert_eq!(
        agent.cache().invalidations(),
        1,
        "boundary crossing must sweep-invalidate the cache exactly once",
    );
    assert!(
        agent.cache().is_empty(),
        "post-boundary cache must hold no entries",
    );

    // First lookup after the boundary is a fresh miss.
    let outcome = agent
        .cache_mut()
        .compile_or_cache(key, COMPILE_COST_NS, t_reset);
    assert!(
        !outcome.is_hit(),
        "first lookup after cache sweep must miss",
    );
    assert_eq!(agent.cache().misses(), 2);
    // History counters are preserved (hits stays at 4 from the
    // pre-boundary streak).
    assert_eq!(agent.cache().hits(), 4);
}

#[test]
fn long_running_workflow_across_multiple_cycles_still_hits_98_percent() {
    // 250 iterations × 1 s each = 2.5 calibration cycles. Each
    // cycle boundary forces exactly one cold miss; the rest of
    // each cycle (≤ 90 in-window iterations) hits. Expected:
    //   misses ≈ 3 (initial + 2 boundary resets)
    //   hits   ≈ remaining operational-window iters
    // Hit rate must still clear the ≥98% bar over the full run.
    //
    // Note: iterations that land inside an outage window are
    // simply skipped (admission is rejected); the cache call is
    // gated on a successful admit.
    let mut agent = build_agent_with_calibration();
    let key = CompilationCacheKey {
        template_id: 99,
        params_hash: 0xABCD,
        qpu_id: 7,
    };

    let mut hits: u64 = 0;
    let mut total: u64 = 0;
    for i in 0..250u64 {
        let now = i * OU_STEP_DT_NS;
        if agent
            .admit_circuit(CircuitExec::new(i, FidelityClass::Standard), now)
            .is_err()
        {
            // In outage — workflow simply waits, no cache call.
            continue;
        }
        let outcome = agent
            .cache_mut()
            .compile_or_cache(key, COMPILE_COST_NS, now);
        total += 1;
        if outcome.is_hit() {
            hits += 1;
        }
    }

    // 2 boundary resets + 1 initial cold = 3 misses total.
    assert_eq!(
        agent.cache().misses(),
        3,
        "expected 3 misses: initial cold + 2 boundary sweeps",
    );
    assert_eq!(agent.cache().invalidations(), 2);
    let rate = hits as f64 / total as f64;
    assert!(
        rate >= 0.98,
        "long-run hit rate {rate:.4} < 0.98 across 2.5 cycles",
    );
}
