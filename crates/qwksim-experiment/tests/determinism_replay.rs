//! Integration test for **T1.9** — Q6′ = R2 deterministic-replay
//! regression gate.
//!
//! Runs the T1.8 FCFS smoke replicate three times with the same
//! `(scenario, seed, provenance)` tuple and asserts that the
//! resulting `workflow_event.parquet` (and `iteration_event.parquet`)
//! files are **byte-identical** across the three runs.
//!
//! Failure surfaces here means a regression has slipped into the
//! deterministic-replay guarantee — either a writer-side
//! non-determinism (HashMap iteration order leaking into Parquet
//! metadata, uninitialised padding, …) or a simulator-side one
//! (incorrect RNG stream split, missing tie-break, etc.). Running
//! as part of `cargo test --workspace` makes this PR-blocking on
//! every push.

use std::fs;

use qwksim_core::telemetry::{ParquetSink, Provenance};
use qwksim_experiment::smoke::{run_fcfs_smoke, ArrivalSpec, SmokeRunSpec};

fn deterministic_provenance() -> Provenance {
    // Every field is a fixed literal — no env var, no hostname
    // probe, no clock. Anything time-varying would break the
    // byte-equality assertion before the simulator even ran.
    Provenance {
        git_commit: "0".repeat(40),
        lockfile_hash: "0".repeat(64),
        scenario_toml: "[scenario]\nname = \"determinism\"\n".to_string(),
        seed: "42".to_string(),
        simulator_version: "0.1.0-determinism-replay".to_string(),
        vendor_calibration_sha256: "0".repeat(64),
        host: "determinism-replay-test".to_string(),
    }
}

fn deterministic_arrivals() -> Vec<ArrivalSpec> {
    // Mix of well-spaced arrivals (no contention) and a contended
    // burst (force FCFS queueing). Same set the T1.8 test uses, so
    // both gates exercise the same code path.
    vec![
        ArrivalSpec {
            workflow_id: 0,
            arrival_ns: 0,
            cores: 1,
            service_ns: 5,
        },
        ArrivalSpec {
            workflow_id: 1,
            arrival_ns: 10,
            cores: 1,
            service_ns: 5,
        },
        ArrivalSpec {
            workflow_id: 2,
            arrival_ns: 20,
            cores: 1,
            service_ns: 5,
        },
        ArrivalSpec {
            workflow_id: 3,
            arrival_ns: 100,
            cores: 1,
            service_ns: 10,
        },
        ArrivalSpec {
            workflow_id: 4,
            arrival_ns: 100,
            cores: 1,
            service_ns: 10,
        },
    ]
}

fn run_into(dir: std::path::PathBuf) {
    run_fcfs_smoke(SmokeRunSpec {
        partition_total_cores: 1,
        arrivals: deterministic_arrivals(),
        output_dir: dir,
        provenance: deterministic_provenance(),
    })
    .expect("smoke run");
}

fn first_diff(a: &[u8], b: &[u8]) -> Option<(usize, u8, u8)> {
    if a.len() != b.len() {
        // Mark the difference at the divergence boundary so the
        // diagnostic shows where one buffer ran out.
        let i = a.len().min(b.len());
        return Some((
            i,
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        ));
    }
    a.iter()
        .zip(b.iter())
        .enumerate()
        .find_map(|(i, (x, y))| (x != y).then_some((i, *x, *y)))
}

#[test]
fn three_consecutive_smoke_runs_produce_byte_identical_parquet() {
    let base = scratch_root();
    let dirs = [base.join("a"), base.join("b"), base.join("c")];
    for d in &dirs {
        run_into(d.clone());
    }

    for name in ["workflow_event.parquet", "iteration_event.parquet"] {
        let bytes_a = fs::read(dirs[0].join(name)).expect("read a");
        let bytes_b = fs::read(dirs[1].join(name)).expect("read b");
        let bytes_c = fs::read(dirs[2].join(name)).expect("read c");

        assert_eq!(
            bytes_a.len(),
            bytes_b.len(),
            "{name}: runs a and b have different sizes ({} vs {})",
            bytes_a.len(),
            bytes_b.len()
        );
        assert_eq!(
            bytes_a.len(),
            bytes_c.len(),
            "{name}: runs a and c have different sizes ({} vs {})",
            bytes_a.len(),
            bytes_c.len()
        );

        if let Some((i, x, y)) = first_diff(&bytes_a, &bytes_b) {
            panic!(
                "{name}: byte differs between run a and run b at offset {i}: {x:#04x} vs {y:#04x}"
            );
        }
        if let Some((i, x, y)) = first_diff(&bytes_a, &bytes_c) {
            panic!(
                "{name}: byte differs between run a and run c at offset {i}: {x:#04x} vs {y:#04x}"
            );
        }
    }
}

#[test]
fn smoke_run_paths_resolve_inside_the_supplied_dir() {
    // Sanity: ParquetSink::workflow_event_path and friends do not
    // leak names that differ across invocations.
    let base = scratch_root().join("paths");
    fs::create_dir_all(&base).unwrap();
    let w1 = ParquetSink::workflow_event_path(&base);
    let w2 = ParquetSink::workflow_event_path(&base);
    assert_eq!(w1, w2);
    assert!(w1.starts_with(&base));
}

fn scratch_root() -> std::path::PathBuf {
    let mut dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    dir.push(format!(
        "t1_9_determinism_{pid}_{nanos}",
        pid = std::process::id(),
        nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).expect("create test scratch root");
    dir
}
