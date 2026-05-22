//! Single-replicate FCFS smoke runner — the first end-to-end path
//! through the simulator that pulls together `qwksim-baselines`,
//! `qwksim-resources`, and `qwksim-core::telemetry`.
//!
//! Per **T1.8**: drive one replicate from arrival through completion
//! and write a Parquet file. The runner is intentionally minimal —
//! no DES kernel, no DesSleep, no τ_adv, no QPU. It exists as the
//! end-to-end seam so the integration test can assert that:
//!
//! 1. Every workflow produces the four lifecycle events
//!    (`Submitted` → `Admitted` → `Started` → `Completed`).
//! 2. The Parquet sink's provenance metadata is complete.
//!
//! Later phases will graduate this runner into the real
//! `qwksim-experiment` orchestration (rayon outer loop, hierarchical
//! manifests, etc.).

use std::path::PathBuf;

use qwksim_baselines::Fcfs;
use qwksim_core::event::{AgentId, SimTime};
use qwksim_core::queue::EventSeqAllocator;
use qwksim_core::telemetry::{
    ParquetSink, Provenance, TelemetryError, TelemetrySink, WorkflowEvent, WorkflowPhase,
};
use qwksim_resources::HpcPartitionAgent;

/// `AgentId` used for the federation router / single super-site
/// portal in the smoke harness. The full topology gets populated in
/// Phase 2.
pub const SMOKE_ROUTER_AGENT: AgentId = 0;

/// One workflow arrival in the smoke run.
#[derive(Debug, Clone, Copy)]
pub struct ArrivalSpec {
    /// Unique identifier for this workflow within the replicate.
    pub workflow_id: u64,
    /// Simulator time of submission.
    pub arrival_ns: SimTime,
    /// CPU cores requested.
    pub cores: u32,
    /// Deterministic service time (simulator nanoseconds).
    pub service_ns: SimTime,
}

/// Replicate input.
#[derive(Debug, Clone)]
pub struct SmokeRunSpec {
    /// Cores on the single HPC partition this smoke runner backs.
    pub partition_total_cores: u32,
    /// Workflow arrivals to drive through the scheduler.
    pub arrivals: Vec<ArrivalSpec>,
    /// Where to materialise the Parquet sink directory.
    pub output_dir: PathBuf,
    /// Provenance fingerprint embedded into every Parquet file.
    pub provenance: Provenance,
}

/// Replicate output summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmokeRunResult {
    /// Number of `WorkflowEvent` records emitted (four per arrival:
    /// Submitted, Admitted, Started, Completed).
    pub workflow_events: u64,
    /// Final simulator time = max completion across all arrivals.
    pub final_time_ns: SimTime,
}

/// Drive `spec` through an `Fcfs` scheduler and write the resulting
/// telemetry to a Parquet sink at `spec.output_dir`.
///
/// Emits four `WorkflowEvent`s per arrival in this order, with
/// timestamps:
///
/// - `Submitted`  at `arrival_ns`
/// - `Admitted`   at `start_ns` (computed from the FCFS completion)
/// - `Started`    at `start_ns`
/// - `Completed`  at `completion_ns = start_ns + service_ns`
///
/// where `start_ns` is the simulator-time the workflow leaves the
/// queue (which equals `arrival_ns` if there was capacity, or later
/// if it had to wait behind earlier FIFO arrivals).
pub fn run_fcfs_smoke(spec: SmokeRunSpec) -> Result<SmokeRunResult, TelemetryError> {
    let partition = HpcPartitionAgent::new(SMOKE_ROUTER_AGENT, spec.partition_total_cores);
    let mut fcfs = Fcfs::new(partition);
    let mut sink = ParquetSink::create(&spec.output_dir, &spec.provenance)?;
    let mut seq = EventSeqAllocator::new();

    let mut events: u64 = 0;
    let mut final_time: SimTime = 0;

    for arrival in &spec.arrivals {
        sink.workflow(WorkflowEvent {
            at_ns: arrival.arrival_ns,
            workflow_id: arrival.workflow_id,
            agent_id: SMOKE_ROUTER_AGENT,
            seq: seq.allocate(),
            phase: WorkflowPhase::Submitted,
        })?;

        let completion_ns = fcfs.submit(arrival.arrival_ns, arrival.cores, arrival.service_ns);
        let start_ns = completion_ns
            .checked_sub(arrival.service_ns)
            .expect("completion ≥ service");

        sink.workflow(WorkflowEvent {
            at_ns: start_ns,
            workflow_id: arrival.workflow_id,
            agent_id: SMOKE_ROUTER_AGENT,
            seq: seq.allocate(),
            phase: WorkflowPhase::Admitted,
        })?;

        sink.workflow(WorkflowEvent {
            at_ns: start_ns,
            workflow_id: arrival.workflow_id,
            agent_id: SMOKE_ROUTER_AGENT,
            seq: seq.allocate(),
            phase: WorkflowPhase::Started,
        })?;

        sink.workflow(WorkflowEvent {
            at_ns: completion_ns,
            workflow_id: arrival.workflow_id,
            agent_id: SMOKE_ROUTER_AGENT,
            seq: seq.allocate(),
            phase: WorkflowPhase::Completed,
        })?;

        events += 4;
        if completion_ns > final_time {
            final_time = completion_ns;
        }
    }

    Box::new(sink).finish()?;

    Ok(SmokeRunResult {
        workflow_events: events,
        final_time_ns: final_time,
    })
}
