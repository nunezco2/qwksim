//! Integration test for **T1.10**: c1 analytical-validation gate.
//!
//! Runs `qwksim-experiment::mm1::run_mm1_replicate` over 100
//! replicates per utilisation `ρ ∈ {0.3, 0.6, 0.9}` (each replicate
//! a Poisson-arrival, exponential-service M/M/1 queue with `μ = 1`),
//! then aggregates the per-replicate sojourn-time and Little's-law
//! estimates and asserts both fall **within 1 %** of the
//! closed-form M/M/1 means:
//!
//! ```text
//! E[N] = ρ / (1 − ρ)         (customers in system; "L")
//! E[W] = 1 / (μ (1 − ρ))      (sojourn time; "W")
//! ```
//!
//! Runtime budget on the project workstation is ≤ 90 s; with
//! 18 000 measured arrivals × 100 replicates × 3 ρ ≈ 5.4 M FCFS
//! `submit`s, the test lands well under that on debug builds.

use std::time::Instant;

use qwksim_experiment::mm1::{
    run_mm1_replicate, theoretical_mean_in_system, theoretical_mean_sojourn_seconds, Mm1RunSpec,
    Mm1Statistics,
};

const SERVICE_RATE: f64 = 1.0;
const RHOS: [f64; 3] = [0.3, 0.6, 0.9];
const REPLICATES: u32 = 100;
const TOLERANCE: f64 = 0.01;

fn warmup_for(rho: f64) -> u32 {
    // Heavier-tailed regimes need a longer warm-up before the
    // measurement window converges.
    if rho >= 0.9 {
        2_000
    } else {
        1_000
    }
}

fn measured_for(rho: f64) -> u32 {
    // ρ ≥ 0.9 has higher per-arrival variance; pay for tighter
    // estimates with more measured arrivals.
    if rho >= 0.9 {
        30_000
    } else {
        15_000
    }
}

fn replicate_seed(rho: f64, replicate: u32) -> [u8; 32] {
    // Hand-rolled seed: tag rho × 10 (so 0.3 → 3) and the
    // replicate index in the byte stream. The leading bytes
    // domain-separate the M/M/1 validation from the determinism
    // replay and FCFS smoke runs.
    let mut seed = [0u8; 32];
    seed[0..6].copy_from_slice(b"mm1v01");
    seed[6] = (rho * 10.0) as u8;
    seed[8..12].copy_from_slice(&replicate.to_le_bytes());
    seed
}

fn run_rho(rho: f64) -> (f64, f64) {
    let spec_template = Mm1RunSpec {
        service_rate: SERVICE_RATE,
        utilisation: rho,
        measured_arrivals: measured_for(rho),
        warmup_arrivals: warmup_for(rho),
        seed: [0u8; 32],
    };

    let mut sum_l = 0.0f64;
    let mut sum_w = 0.0f64;
    for r in 0..REPLICATES {
        let mut spec = spec_template;
        spec.seed = replicate_seed(rho, r);
        let Mm1Statistics {
            mean_in_system,
            mean_sojourn_seconds,
        } = run_mm1_replicate(spec);
        sum_l += mean_in_system;
        sum_w += mean_sojourn_seconds;
    }
    (sum_l / REPLICATES as f64, sum_w / REPLICATES as f64)
}

#[test]
fn mm1_means_match_textbook_within_one_percent() {
    let start = Instant::now();

    let mut diagnostics = Vec::new();
    for &rho in &RHOS {
        let (mean_l, mean_w) = run_rho(rho);
        let theory_l = theoretical_mean_in_system(rho);
        let theory_w = theoretical_mean_sojourn_seconds(SERVICE_RATE, rho);

        let err_l = (mean_l - theory_l).abs() / theory_l;
        let err_w = (mean_w - theory_w).abs() / theory_w;

        diagnostics.push(format!(
            "ρ={rho}: L = {mean_l:.5} (theory {theory_l:.5}, err {pct_l:.4}%) ; \
             W = {mean_w:.5} (theory {theory_w:.5}, err {pct_w:.4}%)",
            pct_l = err_l * 100.0,
            pct_w = err_w * 100.0,
        ));

        assert!(
            err_l < TOLERANCE,
            "ρ={rho}: L = {mean_l} vs theory {theory_l} differs by {} (>1%)",
            err_l * 100.0
        );
        assert!(
            err_w < TOLERANCE,
            "ρ={rho}: W = {mean_w} vs theory {theory_w} differs by {} (>1%)",
            err_w * 100.0
        );
    }

    let elapsed = start.elapsed();
    // Emit diagnostics on success too — easier to spot drift in
    // future PRs (e.g. if FCFS performance regressions cause the
    // 90 s budget to start tightening).
    eprintln!("M/M/1 c1 validation passed in {elapsed:?}");
    for line in diagnostics {
        eprintln!("  {line}");
    }

    assert!(
        elapsed.as_secs() <= 90,
        "M/M/1 validation took {elapsed:?}; budget is 90 s"
    );
}
