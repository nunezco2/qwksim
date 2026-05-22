//! Telemetry sink trait and Parquet writer.
//!
//! Every observable simulator event flows through [`TelemetrySink`].
//! Three record types map directly to the per-experiment figure
//! inventory in `plan/solution_plan.md` §16:
//!
//! - [`WorkflowEvent`] — one row per workflow lifecycle event
//!   (submitted, admitted, started, completed, deadline-missed).
//!   Always emitted (`tg1`/`tg2` in Q11.6).
//! - [`IterationEvent`] — one row per iteration of an iterative
//!   workflow, with classical, QPU and hand-over timings. Always
//!   emitted (`tg2`).
//! - [`BargainingRoundEvent`] — one row per round of best-response
//!   inside a bargaining episode. Heavy under headline load (a few
//!   GB per run); emission is gated behind the
//!   `per-round-telemetry` Cargo feature (`tg3` in Q11.6).
//!
//! [`ParquetSink`] is the concrete sink. It takes a directory and
//! materialises one Parquet file per record type
//! (`workflow_event.parquet`, `iteration_event.parquet`, and
//! `bargain_round_event.parquet` when the `per-round-telemetry`
//! feature is on). Every file embeds the [`Provenance`] block in
//! its `key_value_metadata` (Q15.2 = pf3). Records are buffered in
//! batches of `DEFAULT_BATCH_SIZE` rows and flushed on either batch
//! fill or [`TelemetrySink::finish`].

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, RecordBatch, StringArray, UInt32Array, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::ArrowWriter;
use parquet::errors::ParquetError;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;

use crate::event::{AgentId, EventSeq, SimTime};

/// Default number of rows buffered in memory before a record batch
/// is flushed to its Parquet writer. Each writer has its own
/// buffer; flushes happen independently.
pub const DEFAULT_BATCH_SIZE: usize = 1024;

/// File name for the workflow-event Parquet file inside the sink
/// directory.
pub const WORKFLOW_EVENT_FILE: &str = "workflow_event.parquet";

/// File name for the iteration-event Parquet file inside the sink
/// directory.
pub const ITERATION_EVENT_FILE: &str = "iteration_event.parquet";

/// File name for the per-bargaining-round event Parquet file
/// (only present when the `per-round-telemetry` feature is on).
pub const BARGAIN_ROUND_EVENT_FILE: &str = "bargain_round_event.parquet";

/// Workflow lifecycle phase. Values map to canonical lowercase
/// strings in the `phase` column of `workflow_event.parquet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkflowPhase {
    /// Workflow has arrived at the federation router.
    Submitted,
    /// Workflow has been admitted to a super-site.
    Admitted,
    /// First task of the workflow has started executing.
    Started,
    /// Workflow has finished all tasks.
    Completed,
    /// Workflow's deadline elapsed before completion.
    DeadlineMissed,
}

impl WorkflowPhase {
    /// Stable lowercase string used in the Parquet `phase` column.
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowPhase::Submitted => "submitted",
            WorkflowPhase::Admitted => "admitted",
            WorkflowPhase::Started => "started",
            WorkflowPhase::Completed => "completed",
            WorkflowPhase::DeadlineMissed => "deadline_missed",
        }
    }
}

/// One row in `workflow_event.parquet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkflowEvent {
    /// Simulator time when this lifecycle event fired.
    pub at_ns: SimTime,
    /// Workflow identifier.
    pub workflow_id: u64,
    /// Agent that emitted the event (router / portal / scheduler).
    pub agent_id: AgentId,
    /// Event-sequence tie-break key (from the simulator's
    /// `EventSeqAllocator`).
    pub seq: EventSeq,
    /// Lifecycle phase.
    pub phase: WorkflowPhase,
}

/// One row in `iteration_event.parquet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IterationEvent {
    /// Simulator time when the iteration completed.
    pub at_ns: SimTime,
    /// Owning workflow.
    pub workflow_id: u64,
    /// Agent that executed the iteration (per-resource scheduler).
    pub agent_id: AgentId,
    /// Event-sequence tie-break key.
    pub seq: EventSeq,
    /// Iteration index within the workflow's iterative loop.
    pub iteration: u32,
    /// Classical work elapsed this iteration (simulator
    /// nanoseconds).
    pub classical_ns: u64,
    /// QPU work elapsed this iteration (simulator nanoseconds).
    pub qpu_ns: u64,
    /// Hand-over latency this iteration (simulator nanoseconds).
    pub handover_ns: u64,
}

/// One row in `bargain_round_event.parquet`. Schema lands together
/// with the bargaining solver in Phase 4; today this is a marker
/// type carrying only the tie-break key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BargainingRoundEvent {
    /// Simulator time the round fired.
    pub at_ns: SimTime,
    /// Bargaining episode identifier.
    pub episode_id: u64,
    /// Round number within the episode (0-indexed).
    pub round: u32,
}

/// Universal interface for emitting simulator events to durable
/// storage.
///
/// Methods take `&mut self` because real implementations buffer
/// batches before flushing. Methods return `Result` so I/O failures
/// can propagate.
pub trait TelemetrySink {
    /// Emit one workflow lifecycle record.
    fn workflow(&mut self, rec: WorkflowEvent) -> Result<(), TelemetryError>;

    /// Emit one iteration record.
    fn iteration(&mut self, rec: IterationEvent) -> Result<(), TelemetryError>;

    /// Emit one per-bargaining-round record (only available with
    /// the `per-round-telemetry` feature).
    #[cfg(feature = "per-round-telemetry")]
    fn bargain_round(&mut self, rec: BargainingRoundEvent) -> Result<(), TelemetryError>;

    /// Finalise the sink, flushing any buffered batches and closing
    /// all underlying Parquet files.
    fn finish(self: Box<Self>) -> Result<(), TelemetryError>;
}

/// Provenance fingerprint embedded in every output Parquet file
/// (Q15.2 = pf3).
#[derive(Debug, Clone, Default)]
pub struct Provenance {
    /// Long-form git commit SHA-1, e.g. from `git rev-parse HEAD`.
    pub git_commit: String,
    /// SHA-256 of the workspace `Cargo.lock`, lowercase hex.
    pub lockfile_hash: String,
    /// Resolved post-expansion scenario TOML for this run.
    pub scenario_toml: String,
    /// Replicate seed as a decimal string.
    pub seed: String,
    /// `env!("CARGO_PKG_VERSION")` from the simulator crate.
    pub simulator_version: String,
    /// SHA-256 of the materialised vendor calibration data
    /// (`$OUT_DIR/calibration_*.json` from the `qwksim-qpu`
    /// `build.rs`), lowercase hex.
    pub vendor_calibration_sha256: String,
    /// Host descriptor (e.g. `hostname; arch; cpu-model`).
    pub host: String,
}

impl Provenance {
    /// Materialise the provenance as `KeyValue` entries suitable
    /// for `WriterProperties::set_key_value_metadata`.
    pub fn to_key_values(&self) -> Vec<KeyValue> {
        let entries: [(&str, &str); 7] = [
            ("qwksim.git_commit", &self.git_commit),
            ("qwksim.lockfile_hash", &self.lockfile_hash),
            ("qwksim.scenario_toml", &self.scenario_toml),
            ("qwksim.seed", &self.seed),
            ("qwksim.simulator_version", &self.simulator_version),
            (
                "qwksim.vendor_calibration_sha256",
                &self.vendor_calibration_sha256,
            ),
            ("qwksim.host", &self.host),
        ];
        entries
            .into_iter()
            .map(|(k, v)| KeyValue {
                key: k.to_string(),
                value: Some(v.to_string()),
            })
            .collect()
    }

    /// Reverse of [`Provenance::to_key_values`]: build a
    /// `Provenance` from a slice of `KeyValue` entries. Unknown
    /// keys are ignored; missing keys land as empty strings.
    pub fn from_key_values(kv: &[KeyValue]) -> Self {
        let mut map: HashMap<&str, &str> = HashMap::new();
        for entry in kv {
            if let Some(value) = entry.value.as_deref() {
                map.insert(entry.key.as_str(), value);
            }
        }
        let take = |k: &str| map.get(k).copied().unwrap_or_default().to_string();
        Self {
            git_commit: take("qwksim.git_commit"),
            lockfile_hash: take("qwksim.lockfile_hash"),
            scenario_toml: take("qwksim.scenario_toml"),
            seed: take("qwksim.seed"),
            simulator_version: take("qwksim.simulator_version"),
            vendor_calibration_sha256: take("qwksim.vendor_calibration_sha256"),
            host: take("qwksim.host"),
        }
    }
}

/// Telemetry-sink error type.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// I/O failure opening or writing an underlying Parquet file.
    #[error("telemetry I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Arrow / Parquet writer-side failure.
    #[error("telemetry parquet: {0}")]
    Parquet(#[from] ParquetError),
}

// --- column-vec builders --------------------------------------------------

#[derive(Default)]
struct WorkflowBuffer {
    at_ns: Vec<u64>,
    workflow_id: Vec<u64>,
    agent_id: Vec<u32>,
    seq: Vec<u64>,
    phase: Vec<&'static str>,
}

impl WorkflowBuffer {
    fn push(&mut self, rec: WorkflowEvent) {
        self.at_ns.push(rec.at_ns);
        self.workflow_id.push(rec.workflow_id);
        self.agent_id.push(rec.agent_id);
        self.seq.push(rec.seq);
        self.phase.push(rec.phase.as_str());
    }

    fn len(&self) -> usize {
        self.at_ns.len()
    }

    fn drain_into_batch(&mut self, schema: &SchemaRef) -> RecordBatch {
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(UInt64Array::from(std::mem::take(&mut self.at_ns))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.workflow_id))),
            Arc::new(UInt32Array::from(std::mem::take(&mut self.agent_id))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.seq))),
            Arc::new(StringArray::from(std::mem::take(&mut self.phase))),
        ];
        RecordBatch::try_new(schema.clone(), arrays).expect("workflow schema matches arrays")
    }
}

#[derive(Default)]
struct IterationBuffer {
    at_ns: Vec<u64>,
    workflow_id: Vec<u64>,
    agent_id: Vec<u32>,
    seq: Vec<u64>,
    iteration: Vec<u32>,
    classical_ns: Vec<u64>,
    qpu_ns: Vec<u64>,
    handover_ns: Vec<u64>,
}

impl IterationBuffer {
    fn push(&mut self, rec: IterationEvent) {
        self.at_ns.push(rec.at_ns);
        self.workflow_id.push(rec.workflow_id);
        self.agent_id.push(rec.agent_id);
        self.seq.push(rec.seq);
        self.iteration.push(rec.iteration);
        self.classical_ns.push(rec.classical_ns);
        self.qpu_ns.push(rec.qpu_ns);
        self.handover_ns.push(rec.handover_ns);
    }

    fn len(&self) -> usize {
        self.at_ns.len()
    }

    fn drain_into_batch(&mut self, schema: &SchemaRef) -> RecordBatch {
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(UInt64Array::from(std::mem::take(&mut self.at_ns))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.workflow_id))),
            Arc::new(UInt32Array::from(std::mem::take(&mut self.agent_id))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.seq))),
            Arc::new(UInt32Array::from(std::mem::take(&mut self.iteration))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.classical_ns))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.qpu_ns))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.handover_ns))),
        ];
        RecordBatch::try_new(schema.clone(), arrays).expect("iteration schema matches arrays")
    }
}

// --- schema constructors --------------------------------------------------

fn workflow_event_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("at_ns", DataType::UInt64, false),
        Field::new("workflow_id", DataType::UInt64, false),
        Field::new("agent_id", DataType::UInt32, false),
        Field::new("seq", DataType::UInt64, false),
        Field::new("phase", DataType::Utf8, false),
    ]))
}

fn iteration_event_schema() -> SchemaRef {
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

// --- the sink itself ------------------------------------------------------

/// Parquet-backed implementation of [`TelemetrySink`].
///
/// Each run gets its own directory; one Parquet file per record
/// type lives inside. Both files always exist (so downstream tools
/// can rely on a stable layout), but may be empty.
pub struct ParquetSink {
    workflow_writer: ArrowWriter<File>,
    workflow_schema: SchemaRef,
    workflow_buffer: WorkflowBuffer,
    iteration_writer: ArrowWriter<File>,
    iteration_schema: SchemaRef,
    iteration_buffer: IterationBuffer,
    batch_size: usize,
}

impl ParquetSink {
    /// Open a sink that writes into `dir`, creating it (and any
    /// missing parents) if necessary. Embeds `provenance` as
    /// `key_value_metadata` on every Parquet file.
    pub fn create(dir: &Path, provenance: &Provenance) -> Result<Self, TelemetryError> {
        Self::create_with_batch_size(dir, provenance, DEFAULT_BATCH_SIZE)
    }

    /// Same as [`Self::create`] but with an explicit batch-size
    /// threshold for the internal row buffers. `batch_size == 1`
    /// forces every record to flush immediately; mainly useful for
    /// tests that want to verify multi-batch behaviour without
    /// emitting thousands of rows.
    pub fn create_with_batch_size(
        dir: &Path,
        provenance: &Provenance,
        batch_size: usize,
    ) -> Result<Self, TelemetryError> {
        assert!(batch_size > 0, "batch_size must be ≥ 1");

        fs::create_dir_all(dir)?;
        let provenance_kv = provenance.to_key_values();

        let workflow_schema = workflow_event_schema();
        let workflow_writer = open_writer(
            &dir.join(WORKFLOW_EVENT_FILE),
            workflow_schema.clone(),
            provenance_kv.clone(),
        )?;

        let iteration_schema = iteration_event_schema();
        let iteration_writer = open_writer(
            &dir.join(ITERATION_EVENT_FILE),
            iteration_schema.clone(),
            provenance_kv,
        )?;

        Ok(Self {
            workflow_writer,
            workflow_schema,
            workflow_buffer: WorkflowBuffer::default(),
            iteration_writer,
            iteration_schema,
            iteration_buffer: IterationBuffer::default(),
            batch_size,
        })
    }

    /// Resolve the on-disk path of the workflow-event Parquet file
    /// inside a sink directory.
    pub fn workflow_event_path(dir: &Path) -> PathBuf {
        dir.join(WORKFLOW_EVENT_FILE)
    }

    /// Resolve the on-disk path of the iteration-event Parquet
    /// file inside a sink directory.
    pub fn iteration_event_path(dir: &Path) -> PathBuf {
        dir.join(ITERATION_EVENT_FILE)
    }
}

fn open_writer(
    path: &Path,
    schema: SchemaRef,
    provenance_kv: Vec<KeyValue>,
) -> Result<ArrowWriter<File>, TelemetryError> {
    let file = File::create(path)?;
    let props = WriterProperties::builder()
        .set_key_value_metadata(Some(provenance_kv))
        .build();
    Ok(ArrowWriter::try_new(file, schema, Some(props))?)
}

impl TelemetrySink for ParquetSink {
    fn workflow(&mut self, rec: WorkflowEvent) -> Result<(), TelemetryError> {
        self.workflow_buffer.push(rec);
        if self.workflow_buffer.len() >= self.batch_size {
            let batch = self.workflow_buffer.drain_into_batch(&self.workflow_schema);
            self.workflow_writer.write(&batch)?;
        }
        Ok(())
    }

    fn iteration(&mut self, rec: IterationEvent) -> Result<(), TelemetryError> {
        self.iteration_buffer.push(rec);
        if self.iteration_buffer.len() >= self.batch_size {
            let batch = self
                .iteration_buffer
                .drain_into_batch(&self.iteration_schema);
            self.iteration_writer.write(&batch)?;
        }
        Ok(())
    }

    #[cfg(feature = "per-round-telemetry")]
    fn bargain_round(&mut self, _rec: BargainingRoundEvent) -> Result<(), TelemetryError> {
        // Lands with the bargaining solver in Phase 4 — the writer
        // body is gated behind the same feature.
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), TelemetryError> {
        if self.workflow_buffer.len() > 0 {
            let batch = self.workflow_buffer.drain_into_batch(&self.workflow_schema);
            self.workflow_writer.write(&batch)?;
        }
        if self.iteration_buffer.len() > 0 {
            let batch = self
                .iteration_buffer
                .drain_into_batch(&self.iteration_schema);
            self.iteration_writer.write(&batch)?;
        }
        self.workflow_writer.close()?;
        self.iteration_writer.close()?;
        Ok(())
    }
}
