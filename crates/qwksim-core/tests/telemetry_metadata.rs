//! Roundtrip test: write an empty Parquet file with the full
//! `Provenance` block embedded as `key_value_metadata`, re-open it,
//! and verify every field is preserved (Q15.2 = pf3).
//!
//! This is the headline acceptance gate for T0.11.

use std::fs::File;

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use qwksim_core::telemetry::{ParquetSink, Provenance, TelemetrySink};

fn sample_provenance() -> Provenance {
    Provenance {
        git_commit: "deadbeefcafef00d1234567890abcdef12345678".to_string(),
        lockfile_hash: "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
            .to_string(),
        scenario_toml: r#"[scenario]
name = "headline"
load = 1.0
tau_adv_ms = 10
"#
        .to_string(),
        seed: "42".to_string(),
        simulator_version: env!("CARGO_PKG_VERSION").to_string(),
        vendor_calibration_sha256:
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        host: "test-host; aarch64-darwin; m-series".to_string(),
    }
}

#[test]
fn roundtrip_preserves_every_provenance_field() {
    let dir = tempdir();
    let path = dir.join("empty.parquet");

    let prov = sample_provenance();

    let sink = ParquetSink::create(&path, &prov).expect("open sink");
    Box::new(sink).finish().expect("finish sink");

    let file = File::open(&path).expect("reopen parquet");
    let reader = ParquetRecordBatchReaderBuilder::try_new(file).expect("parquet builder");
    let kv = reader
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .expect("key_value_metadata present")
        .clone();

    let read_back = Provenance::from_key_values(&kv);

    assert_eq!(read_back.git_commit, prov.git_commit);
    assert_eq!(read_back.lockfile_hash, prov.lockfile_hash);
    assert_eq!(read_back.scenario_toml, prov.scenario_toml);
    assert_eq!(read_back.seed, prov.seed);
    assert_eq!(read_back.simulator_version, prov.simulator_version);
    assert_eq!(
        read_back.vendor_calibration_sha256,
        prov.vendor_calibration_sha256
    );
    assert_eq!(read_back.host, prov.host);
}

#[test]
fn metadata_block_contains_all_qwksim_keys() {
    let dir = tempdir();
    let path = dir.join("empty_keys.parquet");
    let prov = sample_provenance();

    let sink = ParquetSink::create(&path, &prov).expect("open sink");
    Box::new(sink).finish().expect("finish sink");

    let file = File::open(&path).expect("reopen parquet");
    let reader = ParquetRecordBatchReaderBuilder::try_new(file).expect("parquet builder");
    let keys: Vec<String> = reader
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .expect("metadata present")
        .iter()
        .map(|kv| kv.key.clone())
        .collect();

    for required in [
        "qwksim.git_commit",
        "qwksim.lockfile_hash",
        "qwksim.scenario_toml",
        "qwksim.seed",
        "qwksim.simulator_version",
        "qwksim.vendor_calibration_sha256",
        "qwksim.host",
    ] {
        assert!(
            keys.iter().any(|k| k == required),
            "expected key {required:?} in metadata, got {keys:?}"
        );
    }
}

/// Build a per-test-run scratch directory under `target/`. Using
/// `target/` rather than `std::env::temp_dir()` keeps the artefact
/// inside the cargo build tree (cleaned by `cargo clean`) and avoids
/// pulling in an extra `tempfile` dependency for the skeleton.
fn tempdir() -> std::path::PathBuf {
    let mut dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    dir.push(format!(
        "t0_11_telemetry_{pid}_{nanos}",
        pid = std::process::id(),
        nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("create test tempdir");
    dir
}
