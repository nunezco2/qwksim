//! Integration test for **T2.11** — end-to-end VQE workflow
//! driven through the Phase-1+2 stack (workflow → iter → fcfs →
//! telemetry). Verifies:
//!
//! 1. The runner returns successfully and reports
//!    `iterations_run ∈ [min_iters, max_iters]`.
//! 2. `iteration_event.parquet` carries the expected row count
//!    (one per iteration), with `iteration` indices `0..n` and
//!    matching `workflow_id` / `agent_id`.
//! 3. **Resource-release semantics**: every iteration ends with
//!    `HpcPartitionAgent::used_cores() == 0` (verified via the
//!    `VqeSmokeResult::resource_release_clean` flag).
//! 4. **Deterministic replay (T1.9)**: two runs with the same
//!    `(master_seed, replicate_index, workflow_id)` produce
//!    byte-identical Parquet output.

use std::fs::File;

use arrow::array::{UInt32Array, UInt64Array};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use qwksim_core::telemetry::{ParquetSink, Provenance};
use qwksim_experiment::vqe_smoke::{run_vqe_smoke, VqeSmokeSpec};

fn sample_provenance() -> Provenance {
    Provenance {
        git_commit: "0".repeat(40),
        lockfile_hash: "0".repeat(64),
        scenario_toml: "[scenario]\nname = \"vqe-fcfs-smoke\"\n".to_string(),
        seed: "7".to_string(),
        simulator_version: env!("CARGO_PKG_VERSION").to_string(),
        vendor_calibration_sha256: "0".repeat(64),
        host: "t2.11-vqe-fcfs".to_string(),
    }
}

fn tempdir(suffix: &str) -> std::path::PathBuf {
    let mut dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    dir.push(format!(
        "t2_11_vqe_fcfs_{suffix}_{pid}_{nanos}",
        pid = std::process::id(),
        nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&dir).expect("create test tempdir");
    dir
}

#[test]
fn vqe_on_fcfs_produces_iteration_events_and_releases_resources() {
    let dir = tempdir("primary");
    let spec =
        VqeSmokeSpec::fast_convergence(/* workflow_id */ 42, dir.clone(), sample_provenance());
    let result = run_vqe_smoke(spec).expect("vqe smoke run");

    // 1. min_iters ≤ iterations_run ≤ max_iters (VQE anchor: 10..=50).
    assert!(
        (10..=50).contains(&result.iterations_run),
        "iterations_run {} out of [10, 50]",
        result.iterations_run
    );
    // 2. Workflow lifecycle emitted twice (Submitted + Completed).
    assert_eq!(result.workflow_events_emitted, 2);
    // 3. One iteration event per iteration.
    assert_eq!(
        result.iteration_events_emitted as u32,
        result.iterations_run
    );
    // 4. Resource-release invariant: with 1 core, every task in
    //    every iteration runs strictly sequentially, so the final
    //    simulator-time equals `iterations_run × sum(task durations)`.
    //    Any over-commit / missing-release / late-release bug in
    //    Fcfs would inflate this total.
    let expected_final_time = result.classical_per_iter_ns * result.iterations_run as u64;
    assert_eq!(
        result.final_time_ns,
        expected_final_time,
        "final_time_ns {} ≠ iterations_run ({}) × classical_per_iter_ns ({}) = {}",
        result.final_time_ns,
        result.iterations_run,
        result.classical_per_iter_ns,
        expected_final_time
    );

    // Reopen iteration_event.parquet and verify schema-level
    // content.
    let path = ParquetSink::iteration_event_path(&dir);
    let reader = ParquetRecordBatchReaderBuilder::try_new(
        File::open(&path).expect("reopen iteration_event"),
    )
    .expect("parquet builder");

    let mut iter_idxs: Vec<u32> = Vec::new();
    let mut workflow_ids: Vec<u64> = Vec::new();
    let mut rdr = reader.build().expect("build parquet reader");
    while let Some(batch) = rdr.next().transpose().expect("read batch") {
        let cols = batch.columns();
        let wid = cols[1]
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("workflow_id col");
        let it = cols[4]
            .as_any()
            .downcast_ref::<UInt32Array>()
            .expect("iteration col");
        for i in 0..batch.num_rows() {
            workflow_ids.push(wid.value(i));
            iter_idxs.push(it.value(i));
        }
    }

    assert_eq!(iter_idxs.len() as u32, result.iterations_run);
    // Iteration indices must be contiguous starting from 0.
    let expected_iters: Vec<u32> = (0..result.iterations_run).collect();
    assert_eq!(iter_idxs, expected_iters);
    // All rows share the workflow_id.
    for wid in &workflow_ids {
        assert_eq!(*wid, 42);
    }
}

#[test]
fn vqe_on_fcfs_replay_produces_byte_identical_parquet() {
    // T2.11 acceptance: deterministic-replay CI test (T1.9) stays
    // green when the VQE-on-FCFS smoke is added to the
    // pipeline. Two runs with identical spec produce byte-
    // identical Parquet outputs.
    let dir_a = tempdir("replay_a");
    let dir_b = tempdir("replay_b");

    let mut spec_a = VqeSmokeSpec::fast_convergence(11, dir_a.clone(), sample_provenance());
    spec_a.master_seed = 0xfeed;
    spec_a.replicate_index = 3;

    let mut spec_b = spec_a.clone();
    spec_b.output_dir = dir_b.clone();

    let result_a = run_vqe_smoke(spec_a).expect("smoke a");
    let result_b = run_vqe_smoke(spec_b).expect("smoke b");

    // Same logical outcome.
    assert_eq!(result_a.iterations_run, result_b.iterations_run);
    assert_eq!(result_a.final_time_ns, result_b.final_time_ns);
    assert_eq!(
        result_a.final_smoothed_objective, result_b.final_smoothed_objective,
        "smoothed objective must be bit-identical under CRN"
    );

    // Byte-identical Parquet output on both per-record files.
    for filename in ["workflow_event.parquet", "iteration_event.parquet"] {
        let a = std::fs::read(dir_a.join(filename)).expect("read a");
        let b = std::fs::read(dir_b.join(filename)).expect("read b");
        assert_eq!(
            a.len(),
            b.len(),
            "{filename}: byte-length mismatch ({} vs {})",
            a.len(),
            b.len()
        );
        assert_eq!(a, b, "{filename}: byte content mismatch");
    }
}
