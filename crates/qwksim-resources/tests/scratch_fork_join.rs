//! Integration test for **T2.4** — drive a small fork-join
//! through a `ScratchIoPool` and assert the resulting makespan
//! matches a hand-checked piecewise-linear computation.
//!
//! ## Hand check
//!
//! Capacity = 1 GB/s. Saturation count = 4. Two streams admitted
//! at `t = 0`:
//!
//! - `stream A`: demand 500 MB
//! - `stream B`: demand 1000 MB
//!
//! Phase 1 — both active, each sees 500 MB/s.
//!   After 1 s: A has transferred 500 MB (done); B has
//!   transferred 500 MB (500 MB remaining). Release A.
//!
//! Phase 2 — only B active, sees 1 GB/s.
//!   After 0.5 s: B has transferred the remaining 500 MB. Release.
//!
//! Total fork-join makespan = 1 s + 0.5 s = **1.5 s**.

use qwksim_resources::{ActiveStream, ScratchIoPool};

/// Drive `pool` until every admitted stream has drained its
/// demand. Returns the makespan in seconds.
///
/// At each iteration:
/// 1. Compute the common per-stream share.
/// 2. Find the smallest `remaining / share` (next stream to
///    finish).
/// 3. Advance simulated time by that amount, decrement every
///    active stream's remaining demand by `share × dt`.
/// 4. Release streams whose remaining reaches zero (with a small
///    tolerance for f64 round-off).
///
/// Bytes-per-stream state is kept by the harness — the pool
/// itself doesn't decrement `ActiveStream::demand_bytes`.
fn drive_until_drained(pool: &mut ScratchIoPool, streams: Vec<ActiveStream>) -> f64 {
    use std::collections::BTreeMap;

    // Per-stream remaining bytes, mirrored from each stream's
    // demand at admit time.
    let mut remaining: BTreeMap<u64, f64> = streams
        .iter()
        .map(|s| (s.id, s.demand_bytes as f64))
        .collect();

    for s in streams {
        pool.admit(s, 0);
    }

    let mut elapsed_seconds = 0.0f64;
    while !remaining.is_empty() {
        let share = pool
            .current_common_share()
            .expect("pool is non-idle while remaining is non-empty");
        // Smallest remaining / share — the next stream to finish.
        let dt = remaining
            .values()
            .map(|r| r / share)
            .fold(f64::INFINITY, f64::min);

        elapsed_seconds += dt;
        let drained = share * dt;
        // Decrement every active stream by `drained` bytes.
        for r in remaining.values_mut() {
            *r -= drained;
        }
        // Release any stream whose remaining hit ~zero. Use a
        // relative tolerance so f64 round-off doesn't leave a
        // stream lingering for an extra iteration.
        let to_release: Vec<u64> = remaining
            .iter()
            .filter(|(_, r)| **r < 1e-6)
            .map(|(id, _)| *id)
            .collect();
        for id in to_release {
            remaining.remove(&id);
            pool.release(id, 0);
        }
    }
    elapsed_seconds
}

#[test]
fn asymmetric_fork_join_makespan_matches_hand_check() {
    let mut pool = ScratchIoPool::new(1_000_000_000.0, 4); // 1 GB/s, n_cap = 4
    let streams = vec![
        ActiveStream {
            id: 0,
            admitted_at: 0,
            demand_bytes: 500_000_000, // 500 MB
        },
        ActiveStream {
            id: 1,
            admitted_at: 0,
            demand_bytes: 1_000_000_000, // 1 GB
        },
    ];

    let makespan = drive_until_drained(&mut pool, streams);

    // Hand-checked: 1.0 s (Phase 1) + 0.5 s (Phase 2) = 1.5 s.
    assert!(
        (makespan - 1.5).abs() < 1e-9,
        "fork-join makespan {makespan} s ≠ 1.5 s"
    );
    assert_eq!(pool.active_count(), 0, "pool drained");
}

#[test]
fn symmetric_fork_join_makespan_equals_demand_over_per_stream_share() {
    // 4 GB/s capacity (n_cap = 8) with 4 streams each wanting
    // 1 GB. Capacity / 4 = 1 GB/s per stream; each finishes in
    // 1 s; fork-join makespan = 1 s.
    let mut pool = ScratchIoPool::new(4_000_000_000.0, 8);
    let streams = (0..4u64)
        .map(|id| ActiveStream {
            id,
            admitted_at: 0,
            demand_bytes: 1_000_000_000,
        })
        .collect();

    let makespan = drive_until_drained(&mut pool, streams);
    assert!(
        (makespan - 1.0).abs() < 1e-9,
        "symmetric fork-join makespan {makespan} s ≠ 1.0 s"
    );
}

#[test]
fn three_streams_finish_in_staged_order() {
    // Capacity = 6 bytes/s. Three streams with demand 1, 2, 6
    // bytes — chosen so the three finish at distinct times that
    // are easy to hand-check.
    //
    // Phase 1 — all 3 active, share = 2 bytes/s each.
    //   Stream 0 (demand 1) finishes first after 0.5 s.
    //   After 0.5 s: stream 0 done, stream 1 has done 1 of 2 (1 remaining),
    //                  stream 2 has done 1 of 6 (5 remaining).
    //   t = 0.5 s.
    //
    // Phase 2 — streams 1 and 2 active, share = 3 bytes/s each.
    //   Stream 1 (1 byte remaining) finishes after 1/3 s.
    //   After 1/3 s: stream 1 done, stream 2 has done 1 more byte
    //                  (4 remaining).
    //   t = 0.5 + 1/3 = 5/6 s.
    //
    // Phase 3 — only stream 2 active, share = 6 bytes/s.
    //   4 bytes remaining → 4/6 = 2/3 s.
    //   t = 5/6 + 2/3 = 9/6 = 1.5 s total.
    let mut pool = ScratchIoPool::new(6.0, 4);
    let streams = vec![
        ActiveStream {
            id: 0,
            admitted_at: 0,
            demand_bytes: 1,
        },
        ActiveStream {
            id: 1,
            admitted_at: 0,
            demand_bytes: 2,
        },
        ActiveStream {
            id: 2,
            admitted_at: 0,
            demand_bytes: 6,
        },
    ];

    let makespan = drive_until_drained(&mut pool, streams);
    assert!(
        (makespan - 1.5).abs() < 1e-9,
        "three-stream staged makespan {makespan} ≠ 1.5"
    );
}
