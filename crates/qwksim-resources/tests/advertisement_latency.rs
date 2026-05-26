//! Integration test for **T2.10** — the downstream consumer sees
//! a τ_adv-broadcast summary after the **expected mean latency
//! under CRN**.
//!
//! Two-part gate:
//! 1. **Mean latency** — across 1000 broadcasts on a single
//!    link, the sample mean of `delivered - broadcast` is
//!    within 5 % of the configured `LinkLatencyParams::mean_ns`
//!    (the lognormal sampler is *mean-preserving* so this holds
//!    by construction; the test guards against future
//!    regressions in the sampler).
//! 2. **CRN reproducibility** — running the same schedule twice
//!    with the same `(hierarchy, replicate_index)` produces
//!    byte-identical broadcast lists. This is the foundation
//!    the strategy-proofness replay test (Q2.3) will lean on
//!    in Phase 4.

use qwksim_core::event::SimTime;
use qwksim_core::rng::RngHierarchy;
use qwksim_resources::{
    schedule_broadcasts, AdvertisedSummary, AdvertisementChannel, LinkLatencyParams,
};

const TAU_ADV_NS: SimTime = 1_000_000; // 1 ms
const N_BROADCASTS: usize = 1_000;
const LINK_MEAN_NS: f64 = 50_000.0; // 50 µs
const LINK_LOG_SIGMA: f64 = 0.3;
const HORIZON_NS: SimTime = N_BROADCASTS as SimTime * TAU_ADV_NS;
const REPLICATE_INDEX: u64 = 42;

fn channel() -> AdvertisementChannel {
    AdvertisementChannel::new(
        /* src_site */ 0,
        /* dst_site */ 1,
        TAU_ADV_NS,
        LinkLatencyParams::new(LINK_MEAN_NS, LINK_LOG_SIGMA),
    )
}

fn summary_at(_now: SimTime) -> AdvertisedSummary {
    // Constant summary — the integration test exercises the
    // latency-of-delivery semantics, not the summary content.
    AdvertisedSummary::default()
}

#[test]
fn one_thousand_broadcasts_have_mean_latency_within_five_percent() {
    let hierarchy = RngHierarchy::new(0xdead_beef);
    let bcasts = schedule_broadcasts(
        channel(),
        HORIZON_NS,
        summary_at,
        &hierarchy,
        REPLICATE_INDEX,
    );
    assert_eq!(bcasts.len(), N_BROADCASTS);

    let sum_latency: u64 = bcasts
        .iter()
        .map(|b| b.delivered_at_ns - b.broadcast_at_ns)
        .sum();
    let mean_latency_ns = sum_latency as f64 / N_BROADCASTS as f64;
    let rel_err = (mean_latency_ns - LINK_MEAN_NS).abs() / LINK_MEAN_NS;
    assert!(
        rel_err < 0.05,
        "mean latency {mean_latency_ns} ns differs from anchor {LINK_MEAN_NS} ns by {} %",
        rel_err * 100.0,
    );
}

#[test]
fn schedule_is_bit_identical_under_common_random_numbers() {
    // Two simulations sharing (hierarchy, replicate_index)
    // produce identical broadcast lists — the load-bearing
    // invariant for CRN-paired comparisons across mechanisms
    // (Q5.2 = Z 2x2 factorial) and the strategy-proofness
    // replay test (Q2.3 + Q6′ = R2).
    let hierarchy = RngHierarchy::new(0xc0ffee);
    let a = schedule_broadcasts(
        channel(),
        HORIZON_NS,
        summary_at,
        &hierarchy,
        REPLICATE_INDEX,
    );
    let b = schedule_broadcasts(
        channel(),
        HORIZON_NS,
        summary_at,
        &hierarchy,
        REPLICATE_INDEX,
    );
    assert_eq!(a, b, "CRN-paired runs must produce identical schedules");
}

#[test]
fn broadcasts_fire_at_strict_tau_adv_cadence() {
    let hierarchy = RngHierarchy::new(0x42);
    let bcasts = schedule_broadcasts(
        channel(),
        /* horizon */ 10 * TAU_ADV_NS,
        summary_at,
        &hierarchy,
        REPLICATE_INDEX,
    );
    assert_eq!(bcasts.len(), 10);
    for (i, b) in bcasts.iter().enumerate() {
        assert_eq!(b.broadcast_at_ns, i as SimTime * TAU_ADV_NS);
    }
}
