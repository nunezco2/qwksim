//! Integration test for **T1.4**: write 100 `WorkflowEvent`s and
//! 100 `IterationEvent`s through `ParquetSink`, re-open the
//! per-record files, and assert (a) the column schema matches the
//! spec from §3.4 and (b) rows come back in the exact order they
//! were written.

use std::fs::File;
use std::sync::Arc;

use arrow::array::{StringArray, UInt32Array, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use qwksim_core::telemetry::{
    IterationEvent, ParquetSink, Provenance, TelemetrySink, WorkflowEvent, WorkflowPhase,
};

const N: usize = 100;

fn provenance() -> Provenance {
    Provenance {
        git_commit: "0000000000000000000000000000000000000000".to_string(),
        lockfile_hash: "0".repeat(64),
        scenario_toml: String::new(),
        seed: "1".to_string(),
        simulator_version: env!("CARGO_PKG_VERSION").to_string(),
        vendor_calibration_sha256: "0".repeat(64),
        host: "test".to_string(),
    }
}

fn expected_workflow_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("at_ns", DataType::UInt64, false),
        Field::new("workflow_id", DataType::UInt64, false),
        Field::new("agent_id", DataType::UInt32, false),
        Field::new("seq", DataType::UInt64, false),
        Field::new("phase", DataType::Utf8, false),
    ]))
}

fn expected_iteration_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("at_ns", DataType::UInt64, false),
        Field::new("workflow_id", DataType::UInt64, false),
        Field::new("agent_id", DataType::UInt32, false),
        Field::new("seq", DataType::UInt64, false),
        Field::new("iteration", DataType::UInt32, false),
        Field::new("classical_ns", DataType::UInt64, false),
        Field::new("qpu_ns", DataType::UInt64, false),
        Field::new("handover_ns", DataType::UInt64, false),
    ]))
}

#[test]
fn writes_100_workflow_events_with_correct_schema_and_order() {
    let dir = tempdir();

    let prov = provenance();
    // Force batch_size = 17 so the 100 records flush across several
    // RecordBatches; the read-back code must concatenate them in
    // order without reordering.
    let mut sink = ParquetSink::create_with_batch_size(&dir, &prov, 17).expect("open sink");

    let phases = [
        WorkflowPhase::Submitted,
        WorkflowPhase::Admitted,
        WorkflowPhase::Started,
        WorkflowPhase::Completed,
        WorkflowPhase::DeadlineMissed,
    ];

    for i in 0..N {
        let rec = WorkflowEvent {
            at_ns: 100 * i as u64,
            workflow_id: i as u64,
            agent_id: (i % 7) as u32,
            seq: i as u64,
            phase: phases[i % phases.len()],
        };
        sink.workflow(rec).expect("emit workflow event");
    }
    Box::new(sink).finish().expect("finish sink");

    let path = ParquetSink::workflow_event_path(&dir);
    let file = File::open(&path).expect("reopen workflow file");
    let reader_builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("parquet builder");

    // Schema check: every field and type matches the spec.
    let on_disk_schema = reader_builder.schema().clone();
    let expected = expected_workflow_schema();
    assert_eq!(on_disk_schema.fields().len(), expected.fields().len());
    for (got, want) in on_disk_schema.fields().iter().zip(expected.fields().iter()) {
        assert_eq!(got.name(), want.name(), "field-name mismatch");
        assert_eq!(got.data_type(), want.data_type(), "field-type mismatch");
        assert_eq!(
            got.is_nullable(),
            want.is_nullable(),
            "nullability mismatch"
        );
    }

    let mut at_ns = Vec::<u64>::with_capacity(N);
    let mut workflow_id = Vec::<u64>::with_capacity(N);
    let mut agent_id = Vec::<u32>::with_capacity(N);
    let mut seq = Vec::<u64>::with_capacity(N);
    let mut phase = Vec::<String>::with_capacity(N);
    let mut reader = reader_builder.build().expect("build parquet reader");
    while let Some(batch) = reader.next().transpose().expect("read batch") {
        let cols = batch.columns();
        let cat = cols[0]
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("at_ns column");
        let wid = cols[1]
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("workflow_id column");
        let aid = cols[2]
            .as_any()
            .downcast_ref::<UInt32Array>()
            .expect("agent_id column");
        let sq = cols[3]
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("seq column");
        let ph = cols[4]
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("phase column");
        for i in 0..batch.num_rows() {
            at_ns.push(cat.value(i));
            workflow_id.push(wid.value(i));
            agent_id.push(aid.value(i));
            seq.push(sq.value(i));
            phase.push(ph.value(i).to_string());
        }
    }

    // The reader may concatenate row groups into a single
    // user-facing `RecordBatch`. What matters is that total row
    // count and row order survived the multi-batch writer-side
    // flush — verified by the seq / workflow_id equality checks
    // below.
    assert_eq!(at_ns.len(), N);
    assert_eq!(seq, (0..N as u64).collect::<Vec<_>>());
    assert_eq!(
        workflow_id,
        (0..N as u64).collect::<Vec<_>>(),
        "row order preserved"
    );
    for (i, p) in phase.iter().enumerate() {
        assert_eq!(p, phases[i % phases.len()].as_str());
    }
}

#[test]
fn writes_100_iteration_events_with_correct_schema_and_order() {
    let dir = tempdir();
    let mut sink = ParquetSink::create_with_batch_size(&dir, &provenance(), 23).expect("open sink");

    for i in 0..N {
        let rec = IterationEvent {
            at_ns: 1000 + i as u64,
            workflow_id: i as u64 / 5,
            agent_id: 0,
            seq: i as u64,
            iteration: (i % 50) as u32,
            classical_ns: 1_000_000 + i as u64,
            qpu_ns: 5_000 * i as u64,
            handover_ns: 200,
        };
        sink.iteration(rec).expect("emit iteration");
    }
    Box::new(sink).finish().expect("finish sink");

    let path = ParquetSink::iteration_event_path(&dir);
    let file = File::open(&path).expect("reopen iteration file");
    let reader_builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("parquet builder");

    let on_disk = reader_builder.schema().clone();
    let expected = expected_iteration_schema();
    assert_eq!(on_disk.fields().len(), expected.fields().len());
    for (got, want) in on_disk.fields().iter().zip(expected.fields().iter()) {
        assert_eq!(got.name(), want.name());
        assert_eq!(got.data_type(), want.data_type());
        assert_eq!(got.is_nullable(), want.is_nullable());
    }

    let mut seq = Vec::<u64>::with_capacity(N);
    let mut iteration = Vec::<u32>::with_capacity(N);
    let mut reader = reader_builder.build().expect("build parquet reader");
    while let Some(batch) = reader.next().transpose().expect("read batch") {
        let sq = batch
            .column(3)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("seq col");
        let it = batch
            .column(4)
            .as_any()
            .downcast_ref::<UInt32Array>()
            .expect("iteration col");
        for i in 0..batch.num_rows() {
            seq.push(sq.value(i));
            iteration.push(it.value(i));
        }
    }
    assert_eq!(seq.len(), N);
    assert_eq!(seq, (0..N as u64).collect::<Vec<_>>());
    for (i, it) in iteration.iter().enumerate() {
        assert_eq!(*it, (i % 50) as u32);
    }
}

fn tempdir() -> std::path::PathBuf {
    let mut dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    dir.push(format!(
        "t1_4_records_{pid}_{nanos}",
        pid = std::process::id(),
        nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("create test tempdir");
    dir
}
