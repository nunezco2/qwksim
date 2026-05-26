//! **b5** — end-to-end single-replicate benchmark.
//!
//! Drives [`qwksim_experiment::smoke::run_fcfs_smoke`] (T1.8) to
//! completion at a 1 000-arrival scale: a tractable proxy for the
//! Q11 headline workload while we wait for the QPU model
//! (Phase 3) and the bargaining solver (Phase 4) to land. The
//! body exercises the same chain the headline replicate will:
//!
//! `Fcfs::submit` → `HpcPartitionAgent::{accept, release}` →
//! `ParquetSink::workflow` (batched, flushed on `finish`) →
//! `ParquetSink::finish` (closes both per-record files with the
//! `Provenance` block embedded).
//!
//! Each criterion iteration creates a fresh `tempdir` under
//! `CARGO_TARGET_TMPDIR` so the Parquet open / close path runs
//! from a clean state; the `iter_with_setup` placement keeps the
//! tempdir construction out of the timed region.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

use qwksim_core::telemetry::Provenance;
use qwksim_experiment::smoke::{run_fcfs_smoke, ArrivalSpec, SmokeRunSpec};

const ARRIVALS: u64 = 1_000;
const PARTITION_CORES: u32 = 1;
/// Inter-arrival gap, simulator nanoseconds. With service =
/// 500_000 ns this lands utilisation comfortably below 1 so the
/// FCFS queue does not accumulate unboundedly during the run.
const ARRIVAL_GAP_NS: u64 = 1_000_000;
const SERVICE_NS: u64 = 500_000;

fn deterministic_provenance() -> Provenance {
    Provenance {
        git_commit: "0".repeat(40),
        lockfile_hash: "0".repeat(64),
        scenario_toml: "[scenario]\nname = \"b5\"\n".to_string(),
        seed: "1".to_string(),
        simulator_version: env!("CARGO_PKG_VERSION").to_string(),
        vendor_calibration_sha256: "0".repeat(64),
        host: "b5-bench-host".to_string(),
    }
}

fn synthetic_arrivals() -> Vec<ArrivalSpec> {
    (0..ARRIVALS)
        .map(|i| ArrivalSpec {
            workflow_id: i,
            arrival_ns: i * ARRIVAL_GAP_NS,
            cores: 1,
            service_ns: SERVICE_NS,
        })
        .collect()
}

/// Per-process counter used to give every criterion iteration a
/// unique sink directory. Avoids cross-iteration filesystem
/// collisions without the cost of a real PRNG or the wall-clock
/// jitter of `SystemTime`.
static ITER_COUNTER: AtomicU64 = AtomicU64::new(0);

fn fresh_sink_dir() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let n = ITER_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.push(format!("b5_end_to_end_{pid}_{n}", pid = std::process::id(),));
    std::fs::create_dir_all(&dir).expect("create b5 sink dir");
    dir
}

fn bench_smoke_1000(c: &mut Criterion) {
    let mut group = c.benchmark_group("b5::end_to_end");
    // Each iteration emits 4 lifecycle events per arrival (Submitted,
    // Admitted, Started, Completed); throughput is reported in
    // workflows per second so the headline metric reads "smoke
    // throughput".
    group.throughput(Throughput::Elements(ARRIVALS));
    group.bench_function("smoke_1000_arrivals", |b| {
        b.iter_with_setup(
            || (fresh_sink_dir(), synthetic_arrivals()),
            |(dir, arrivals)| {
                let result = run_fcfs_smoke(SmokeRunSpec {
                    partition_total_cores: PARTITION_CORES,
                    arrivals,
                    output_dir: dir,
                    provenance: deterministic_provenance(),
                })
                .expect("smoke run");
                // Touch the result so the optimiser cannot prove the
                // call is dead.
                std::hint::black_box(result);
            },
        );
    });
    group.finish();
}

criterion_group!(benches, bench_smoke_1000);
criterion_main!(benches);
