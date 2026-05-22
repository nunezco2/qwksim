//! Integration test for **T1.8**: drive one replicate through the
//! FCFS smoke runner, then re-open the per-record Parquet file and
//! assert (a) the expected number of `workflow_event` records were
//! emitted (4 lifecycle phases × N arrivals), (b) the provenance
//! metadata is complete on the file, and (c) the lifecycle phases
//! appear in canonical order for each workflow.

use std::fs::File;

use arrow::array::{StringArray, UInt64Array};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use qwksim_core::telemetry::{ParquetSink, Provenance};
use qwksim_experiment::smoke::{run_fcfs_smoke, ArrivalSpec, SmokeRunSpec};

const ARRIVALS: u64 = 5;

fn sample_provenance() -> Provenance {
    Provenance {
        git_commit: "1234567890123456789012345678901234567890".to_string(),
        lockfile_hash: "f".repeat(64),
        scenario_toml: "[scenario]\nname = \"smoke\"\n".to_string(),
        seed: "7".to_string(),
        simulator_version: env!("CARGO_PKG_VERSION").to_string(),
        vendor_calibration_sha256: "a".repeat(64),
        host: "smoke-test; arch-irrelevant".to_string(),
    }
}

fn spec(dir: std::path::PathBuf) -> SmokeRunSpec {
    SmokeRunSpec {
        partition_total_cores: 1,
        // Three arrivals well-spaced (no contention) + two
        // contending arrivals at t = 100 (forces FCFS queueing).
        arrivals: vec![
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
        ],
        output_dir: dir,
        provenance: sample_provenance(),
    }
}

#[test]
fn one_replicate_writes_four_lifecycle_events_per_workflow() {
    let dir = tempdir();
    let result = run_fcfs_smoke(spec(dir.clone())).expect("smoke run");

    // Five workflows × four lifecycle events each.
    assert_eq!(result.workflow_events, ARRIVALS * 4);
    // Last-completing workflow: arrival 4, queued behind 3,
    // 100 + 10 + 10 = 120.
    assert_eq!(result.final_time_ns, 120);

    let path = ParquetSink::workflow_event_path(&dir);
    let reader =
        ParquetRecordBatchReaderBuilder::try_new(File::open(&path).expect("reopen workflow file"))
            .expect("parquet builder");

    let mut workflow_id: Vec<u64> = Vec::new();
    let mut at_ns: Vec<u64> = Vec::new();
    let mut phase: Vec<String> = Vec::new();
    let mut rdr = reader.build().expect("build parquet reader");
    while let Some(batch) = rdr.next().transpose().expect("read batch") {
        let cols = batch.columns();
        let at = cols[0]
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("at_ns");
        let wid = cols[1]
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("workflow_id");
        let ph = cols[4]
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("phase");
        for i in 0..batch.num_rows() {
            at_ns.push(at.value(i));
            workflow_id.push(wid.value(i));
            phase.push(ph.value(i).to_string());
        }
    }

    assert_eq!(at_ns.len() as u64, ARRIVALS * 4);

    // Per-workflow, the four phases must appear in
    // Submitted -> Admitted -> Started -> Completed order.
    let expected_phase_cycle = ["submitted", "admitted", "started", "completed"];
    for w in 0..ARRIVALS {
        let rows: Vec<(usize, &str)> = workflow_id
            .iter()
            .enumerate()
            .filter(|(_, id)| **id == w)
            .map(|(idx, _)| (idx, phase[idx].as_str()))
            .collect();
        assert_eq!(rows.len(), 4, "workflow {w} should have 4 events");
        for (i, (_, p)) in rows.iter().enumerate() {
            assert_eq!(*p, expected_phase_cycle[i], "workflow {w} phase {i}");
        }
    }
}

#[test]
fn replicate_parquet_carries_complete_provenance_metadata() {
    let dir = tempdir();
    let want = sample_provenance();
    run_fcfs_smoke(spec(dir.clone())).expect("smoke run");

    for path in [
        ParquetSink::workflow_event_path(&dir),
        ParquetSink::iteration_event_path(&dir),
    ] {
        let file = File::open(&path).expect("reopen parquet");
        let reader = ParquetRecordBatchReaderBuilder::try_new(file).expect("parquet builder");
        let kv = reader
            .metadata()
            .file_metadata()
            .key_value_metadata()
            .expect("key_value_metadata present")
            .clone();
        let got = Provenance::from_key_values(&kv);

        assert_eq!(got.git_commit, want.git_commit, "{path:?}");
        assert_eq!(got.lockfile_hash, want.lockfile_hash, "{path:?}");
        assert_eq!(got.scenario_toml, want.scenario_toml, "{path:?}");
        assert_eq!(got.seed, want.seed, "{path:?}");
        assert_eq!(got.simulator_version, want.simulator_version, "{path:?}");
        assert_eq!(
            got.vendor_calibration_sha256, want.vendor_calibration_sha256,
            "{path:?}"
        );
        assert_eq!(got.host, want.host, "{path:?}");
    }
}

fn tempdir() -> std::path::PathBuf {
    let mut dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    dir.push(format!(
        "t1_8_smoke_{pid}_{nanos}",
        pid = std::process::id(),
        nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("create test tempdir");
    dir
}
