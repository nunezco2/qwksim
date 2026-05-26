//! M/M/1 validation harness (c1 in §12).
//!
//! Drives the `Fcfs` scheduler against a single-core
//! `HpcPartitionAgent` with Poisson arrivals (rate λ) and
//! exponential service (rate μ), giving a textbook M/M/1 queue
//! whose steady-state distributions are closed-form:
//!
//! ```text
//! ρ = λ / μ
//! E[N] = ρ / (1 - ρ)           (mean customers in system; "L")
//! E[W] = 1 / (μ - λ)            (mean sojourn time; "W")
//! ```
//!
//! [`run_mm1_replicate`] returns a per-replicate
//! [`Mm1Statistics`] estimating both quantities from the observed
//! sojourn times via Little's law (`L = λ · W`). Replicate-level
//! aggregation happens in the integration test
//! `tests/mm1_validation.rs`.
//!
//! Unit conversion: every external rate is expressed in
//! reciprocal-seconds. The harness converts internally to
//! [`SimTime`] nanoseconds, runs the simulation in `u64` ns, and
//! converts sojourn times back to seconds for the statistics.

use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};

use qwksim_baselines::Fcfs;
use qwksim_core::event::SimTime;
use qwksim_resources::HpcPartitionAgent;

/// One nanosecond's worth of `SimTime` per second of theoretical
/// time. Picked so that even `ρ → 1` regimes don't overflow `u64`
/// at the harness's typical arrival counts.
const NS_PER_SECOND: f64 = 1.0e9;

/// Mean estimators for one M/M/1 replicate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mm1Statistics {
    /// Time-averaged mean number of customers in the system
    /// (`L`), computed via Little's law `L = λ · W`.
    pub mean_in_system: f64,
    /// Mean sojourn time `W = wait + service`, in seconds.
    pub mean_sojourn_seconds: f64,
}

/// Inputs to one replicate.
#[derive(Debug, Clone, Copy)]
pub struct Mm1RunSpec {
    /// Service-rate `μ` (reciprocal seconds).
    pub service_rate: f64,
    /// Utilisation `ρ = λ / μ`.
    pub utilisation: f64,
    /// Arrivals to *measure* per replicate (post warm-up).
    pub measured_arrivals: u32,
    /// Arrivals to *discard* at the start of each replicate so the
    /// queue reaches steady state. Recommended: 1000 for ρ ≤ 0.6,
    /// 2000 for ρ ≥ 0.9.
    pub warmup_arrivals: u32,
    /// 32-byte ChaCha20 seed for the replicate's RNG.
    pub seed: [u8; 32],
}

/// Theoretical mean number of customers in an M/M/1 system at
/// utilisation `rho`: `ρ / (1 − ρ)`.
pub fn theoretical_mean_in_system(rho: f64) -> f64 {
    rho / (1.0 - rho)
}

/// Theoretical mean sojourn time of an M/M/1 customer:
/// `1 / (μ − λ) = 1 / (μ (1 − ρ))`.
pub fn theoretical_mean_sojourn_seconds(service_rate: f64, rho: f64) -> f64 {
    1.0 / (service_rate * (1.0 - rho))
}

/// Sample U ∼ Uniform(0, 1) from `rng`, with the lowest
/// representable f64 above 0 substituted for any draw that maps
/// exactly to zero — keeps `-ln(U)` finite.
fn uniform_open_01(rng: &mut ChaCha20Rng) -> f64 {
    // 53-bit mantissa precision: divide a 53-bit unsigned by 2^53.
    let bits = rng.next_u64() >> 11; // top 53 bits
    let u = bits as f64 / (1u64 << 53) as f64;
    if u == 0.0 {
        // 2^-64 probability event; never observed in practice but
        // makes the `-ln(u)` path total.
        f64::MIN_POSITIVE
    } else {
        u
    }
}

/// Sample an exponential random variable with rate `lambda`.
fn exponential(rng: &mut ChaCha20Rng, lambda: f64) -> f64 {
    -uniform_open_01(rng).ln() / lambda
}

/// Drive one M/M/1 replicate end-to-end and return its observed
/// mean sojourn / mean-in-system pair.
pub fn run_mm1_replicate(spec: Mm1RunSpec) -> Mm1Statistics {
    assert!(
        spec.service_rate > 0.0,
        "service_rate must be > 0; got {}",
        spec.service_rate
    );
    assert!(
        spec.utilisation > 0.0 && spec.utilisation < 1.0,
        "utilisation must be in (0, 1); got {}",
        spec.utilisation
    );

    let lambda = spec.utilisation * spec.service_rate;
    let mut rng = ChaCha20Rng::from_seed(spec.seed);
    let mut fcfs = Fcfs::new(HpcPartitionAgent::new(0, 1));

    let mut clock_ns: SimTime = 0;
    let mut sojourn_sum_seconds = 0.0f64;
    let mut measured: u64 = 0;
    let total = (spec.warmup_arrivals as u64) + (spec.measured_arrivals as u64);

    for i in 0..total {
        let inter_arrival_s = exponential(&mut rng, lambda);
        clock_ns = clock_ns.saturating_add((inter_arrival_s * NS_PER_SECOND) as SimTime);
        let service_s = exponential(&mut rng, spec.service_rate);
        let service_ns = (service_s * NS_PER_SECOND) as SimTime;

        // Fcfs::submit requires service_ns > 0; clamp tiny draws
        // up to one nanosecond. The probability of an exponential
        // draw mapping to zero ns is vanishingly small for
        // μ ≈ 1/s, but the clamp keeps the assertion total.
        let service_ns = service_ns.max(1);

        let completion = fcfs.submit(clock_ns, 1, service_ns);
        if i >= spec.warmup_arrivals as u64 {
            let sojourn_ns = completion - clock_ns;
            sojourn_sum_seconds += sojourn_ns as f64 / NS_PER_SECOND;
            measured += 1;
        }
    }

    let mean_sojourn_seconds = sojourn_sum_seconds / measured as f64;
    // Little's law: L = λ * W.
    let mean_in_system = lambda * mean_sojourn_seconds;

    Mm1Statistics {
        mean_in_system,
        mean_sojourn_seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theoretical_formulas_match_textbook() {
        // M/M/1 with μ = 1, ρ = 0.5 → L = 1, W = 2.
        let l = theoretical_mean_in_system(0.5);
        let w = theoretical_mean_sojourn_seconds(1.0, 0.5);
        assert!((l - 1.0).abs() < 1e-12, "L = {l}");
        assert!((w - 2.0).abs() < 1e-12, "W = {w}");
    }

    #[test]
    fn replicate_is_deterministic_under_same_seed() {
        let spec = Mm1RunSpec {
            service_rate: 1.0,
            utilisation: 0.5,
            measured_arrivals: 1_000,
            warmup_arrivals: 100,
            seed: [7u8; 32],
        };
        let a = run_mm1_replicate(spec);
        let b = run_mm1_replicate(spec);
        assert_eq!(a, b);
    }

    #[test]
    fn replicate_decorrelates_under_distinct_seeds() {
        let mut spec_a = Mm1RunSpec {
            service_rate: 1.0,
            utilisation: 0.5,
            measured_arrivals: 1_000,
            warmup_arrivals: 100,
            seed: [1u8; 32],
        };
        let a = run_mm1_replicate(spec_a);
        spec_a.seed = [2u8; 32];
        let b = run_mm1_replicate(spec_a);
        assert_ne!(a.mean_sojourn_seconds, b.mean_sojourn_seconds);
    }
}
