//! Telemetry sink trait and Parquet writer skeleton.
//!
//! The `TelemetrySink` trait is the single interface through which
//! every observable simulator event reaches durable storage. Three
//! record types map directly to the per-experiment figure inventory
//! in `plan/solution_plan.md` §16:
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
//! [`ParquetSink`] is the concrete sink that materialises records
//! into a Parquet file. This skeleton opens the writer with the
//! [`Provenance`] block embedded as
//! [`parquet::file::metadata::KeyValue`] entries (Q15.2 = pf3) but
//! does **not yet flush record batches**; record schemas land
//! together with their producing crates in later phases.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::errors::ParquetError;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;

/// Per-workflow lifecycle record.
///
/// Schema and field set will be fixed when the workflow runtime
/// lands in Phase 2; today this is a marker type.
#[derive(Debug, Clone, Default)]
pub struct WorkflowEvent {}

/// Per-iteration record for iterative workflows.
///
/// Schema lands together with the iterative-loop runtime in Phase 2.
#[derive(Debug, Clone, Default)]
pub struct IterationEvent {}

/// Per-bargaining-round record (`tg3`).
///
/// Schema lands together with the bargaining solver in Phase 4.
/// Only constructed when the `per-round-telemetry` feature is on.
#[derive(Debug, Clone, Default)]
pub struct BargainingRoundEvent {}

/// Universal interface for emitting simulator events to durable
/// storage.
///
/// All methods take `&mut self` because real implementations will
/// buffer batches before flushing. Methods return `Result` so future
/// I/O failures can propagate; the skeleton bodies are no-ops.
pub trait TelemetrySink {
    /// Emit one workflow lifecycle record.
    fn workflow(&mut self, rec: WorkflowEvent) -> Result<(), TelemetryError>;

    /// Emit one iteration record.
    fn iteration(&mut self, rec: IterationEvent) -> Result<(), TelemetryError>;

    /// Emit one per-bargaining-round record (only available with the
    /// `per-round-telemetry` feature).
    #[cfg(feature = "per-round-telemetry")]
    fn bargain_round(&mut self, rec: BargainingRoundEvent) -> Result<(), TelemetryError>;

    /// Finalise the sink, flushing any buffered state.
    fn finish(self: Box<Self>) -> Result<(), TelemetryError>;
}

/// Provenance fingerprint embedded in every output Parquet file
/// (Q15.2 = pf3).
///
/// Every field is a `String` so the sink stays decoupled from how
/// callers compute the values (build script, runtime, harness, …).
/// Empty strings are tolerated; downstream consumers should treat
/// them as "unknown".
#[derive(Debug, Clone, Default)]
pub struct Provenance {
    /// Long-form git commit SHA-1, e.g. from `git rev-parse HEAD`.
    /// Set by the build script or the experiment harness.
    pub git_commit: String,
    /// SHA-256 of the workspace `Cargo.lock`, lowercase hex.
    pub lockfile_hash: String,
    /// The resolved post-expansion scenario TOML for this run.
    /// May be a placeholder during early phases.
    pub scenario_toml: String,
    /// Replicate seed as a decimal string.
    pub seed: String,
    /// `env!("CARGO_PKG_VERSION")` from the simulator crate.
    pub simulator_version: String,
    /// SHA-256 of the materialised vendor calibration data
    /// (`$OUT_DIR/calibration_*.json` from the `qwksim-qpu`
    /// `build.rs`), lowercase hex; empty if not yet known at sink
    /// construction time.
    pub vendor_calibration_sha256: String,
    /// Host descriptor (e.g. `hostname; arch; cpu-model`). The
    /// experiment harness fills this; it's informational and does
    /// not participate in bit-identical replay (R2 only requires
    /// identity *on the same machine*).
    pub host: String,
}

impl Provenance {
    /// Materialise the provenance as `KeyValue` entries suitable for
    /// `WriterProperties::set_key_value_metadata`.
    ///
    /// The set of keys is closed: the eight `qwksim.*` keys below
    /// are the entire embedded provenance fingerprint.
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
    /// `Provenance` from a slice of `KeyValue` entries. Unknown keys
    /// are ignored; missing keys land as empty strings.
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
    /// I/O failure opening or writing the underlying Parquet file.
    #[error("telemetry I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Arrow / Parquet writer-side failure.
    #[error("telemetry parquet: {0}")]
    Parquet(#[from] ParquetError),
}

/// Parquet-backed implementation of [`TelemetrySink`].
///
/// Opens a single Parquet file with the [`Provenance`] embedded as
/// `key_value_metadata` and a placeholder schema. Today the
/// `workflow`, `iteration`, and (gated) `bargain_round` trait
/// methods are no-ops — record schemas and batch flushing land in
/// later phases with the producing components.
pub struct ParquetSink {
    writer: ArrowWriter<File>,
}

impl ParquetSink {
    /// Open a sink at `path`, embedding `provenance` as
    /// `key_value_metadata`. The placeholder schema has a single
    /// `placeholder: i64` column so that downstream tools that
    /// require a non-empty schema can still open the file; no rows
    /// are written until later phases attach real record batches.
    pub fn create(path: &Path, provenance: &Provenance) -> Result<Self, TelemetryError> {
        let file = File::create(path)?;
        let schema = Arc::new(Schema::new(vec![Field::new(
            "placeholder",
            DataType::Int64,
            true,
        )]));
        let props = WriterProperties::builder()
            .set_key_value_metadata(Some(provenance.to_key_values()))
            .build();
        let writer = ArrowWriter::try_new(file, schema, Some(props))?;
        Ok(Self { writer })
    }
}

impl TelemetrySink for ParquetSink {
    fn workflow(&mut self, _rec: WorkflowEvent) -> Result<(), TelemetryError> {
        // Record schemas + batch flushing land in Phase 2 alongside
        // the workflow runtime.
        Ok(())
    }

    fn iteration(&mut self, _rec: IterationEvent) -> Result<(), TelemetryError> {
        Ok(())
    }

    #[cfg(feature = "per-round-telemetry")]
    fn bargain_round(&mut self, _rec: BargainingRoundEvent) -> Result<(), TelemetryError> {
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<(), TelemetryError> {
        self.writer.close()?;
        Ok(())
    }
}
