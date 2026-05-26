//! VQE-on-FCFS smoke runner — the first end-to-end path through
//! the Phase-1 + Phase-2 stack.
//!
//! Stitches together:
//!
//! - [`qwksim_workflow::vqe::template`] (T2.9) to build a Q11.3-
//!   anchored VQE [`Workflow`];
//! - [`qwksim_workflow::iter::run_iteration`] (T2.8) for the cv2
//!   noisy-objective convergence loop;
//! - [`qwksim_baselines::Fcfs`] (T1.7) against an
//!   [`qwksim_resources::HpcPartitionAgent`] (T1.6) for per-task
//!   admission and per-iteration resource-release semantics;
//! - [`qwksim_core::telemetry::ParquetSink`] (T0.11 / T1.4) for
//!   the workflow- and iteration-event records.
//!
//! The runner is deliberately minimal — no DES kernel, no τ_adv
//! advertisement (T2.10 ships the scheduler; the simulator
//! integration lands in T4.x), no QPU (Phase 3). Each iteration's
//! `qpu_ns` and `handover_ns` are therefore `0`; `classical_ns`
//! holds the sum of the 8 VQE-template task durations for that
//! iteration. This is enough surface area to assert that:
//!
//! 1. The full Phase-1+2 stack composes (workflow → iter →
//!    fcfs → telemetry).
//! 2. Per-iteration `IterationEvent` records make it to disk.
//! 3. Resource release follows: at the end of every iteration
//!    the `HpcPartitionAgent` reports `used_cores == 0` (every
//!    task that was admitted is now released).
//! 4. Deterministic replay (T1.9) still holds — two runs with
//!    the same `(master_seed, replicate_index, workflow_id)`
//!    produce byte-identical Parquet output.

use std::path::PathBuf;

use qwksim_baselines::Fcfs;
use qwksim_core::event::{AgentId, SimTime};
use qwksim_core::queue::EventSeqAllocator;
use qwksim_core::rng::RngHierarchy;
use qwksim_core::telemetry::{
    IterationEvent, ParquetSink, Provenance, TelemetryError, TelemetrySink, WorkflowEvent,
    WorkflowPhase,
};
use qwksim_resources::HpcPartitionAgent;
use qwksim_workflow::iter::{run_iteration, IterState, IterativeRunSpec};
use qwksim_workflow::vqe::{template, VqeConfig};

/// `AgentId` used for the federation router in the smoke runner.
pub const VQE_SMOKE_AGENT: AgentId = 0;

/// Replicate input.
#[derive(Debug, Clone)]
pub struct VqeSmokeSpec {
    /// Per-replicate-unique workflow identifier.
    pub workflow_id: u64,
    /// Master RNG seed (drives the T0.12 hierarchical-split tree).
    pub master_seed: u64,
    /// Replicate index — same convention as elsewhere in
    /// qwksim-experiment.
    pub replicate_index: u64,
    /// Where to materialise the Parquet sink directory.
    pub output_dir: PathBuf,
    /// Provenance fingerprint embedded into every Parquet file.
    pub provenance: Provenance,
    /// Cores configured on the single HPC partition for this
    /// smoke run. Must be ≥ 1; defaults to 1 in the integration
    /// test.
    pub partition_total_cores: u32,
    /// VQE convergence-objective parameters. Overridable for the
    /// integration test (the headline anchor parameters are 30
    /// iterations long; the smoke run uses a strong negative
    /// drift so the loop converges within ~15 iterations on the
    /// test workstation).
    pub initial_objective: f64,
    pub drift_per_iter: f64,
    pub noise_std: f64,
    pub convergence_threshold: f64,
}

impl VqeSmokeSpec {
    /// Build a smoke spec with strong negative drift so the run
    /// converges quickly (≤ 30 iterations on the test
    /// workstation).
    pub fn fast_convergence(workflow_id: u64, output_dir: PathBuf, provenance: Provenance) -> Self {
        Self {
            workflow_id,
            master_seed: 0,
            replicate_index: 0,
            output_dir,
            provenance,
            partition_total_cores: 1,
            initial_objective: 10.0,
            drift_per_iter: -1.0,
            noise_std: 0.2,
            convergence_threshold: 0.0,
        }
    }
}

/// Replicate output summary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VqeSmokeResult {
    /// Iterations executed (always in `[min_iters, max_iters]`).
    pub iterations_run: u32,
    /// Final EMA-smoothed objective value.
    pub final_smoothed_objective: f64,
    /// Lifecycle events emitted (always 2: Submitted, Completed).
    pub workflow_events_emitted: u64,
    /// Iteration events emitted (= `iterations_run`).
    pub iteration_events_emitted: u64,
    /// Final simulator-time when the workflow Completed.
    pub final_time_ns: SimTime,
    /// Sum of the 8 VQE task durations for one iteration. The
    /// integration test asserts `final_time_ns == iterations_run
    /// × classical_per_iter_ns`, which is exactly what 1-core
    /// sequential FCFS scheduling produces and what *any*
    /// resource-release bug would break.
    pub classical_per_iter_ns: u64,
}

/// Drive `spec` through a VQE workflow against an `Fcfs`
/// scheduler and write the resulting telemetry to the Parquet
/// sink at `spec.output_dir`.
pub fn run_vqe_smoke(spec: VqeSmokeSpec) -> Result<VqeSmokeResult, TelemetryError> {
    assert!(
        spec.partition_total_cores > 0,
        "VqeSmokeSpec::partition_total_cores must be > 0"
    );

    let hierarchy = RngHierarchy::new(spec.master_seed);
    let cfg = VqeConfig::for_workflow(spec.workflow_id);
    let workflow = template(cfg, &hierarchy, spec.replicate_index);

    let partition = HpcPartitionAgent::new(VQE_SMOKE_AGENT, spec.partition_total_cores);
    let mut fcfs = Fcfs::new(partition);

    let mut sink = ParquetSink::create(&spec.output_dir, &spec.provenance)?;
    let mut seq = EventSeqAllocator::new();

    // Workflow lifecycle: Submitted at t = 0.
    sink.workflow(WorkflowEvent {
        at_ns: 0,
        workflow_id: spec.workflow_id,
        agent_id: VQE_SMOKE_AGENT,
        seq: seq.allocate(),
        phase: WorkflowPhase::Submitted,
    })?;

    // Per-iteration classical wallclock = sum of the 8 VQE task
    // durations.
    let classical_per_iter_ns: u64 = workflow.dag.tasks.iter().map(|t| t.duration_ns).sum();

    // Drive the cv2 loop. Each iteration:
    //   1. Submits 8 tasks to Fcfs sequentially; tracks the
    //      iteration's start and completion simulator-times.
    //   2. Calls run_iteration to update the noisy-objective
    //      state and determine the next outcome.
    //   3. Emits one IterationEvent on success.
    let runtime_spec = IterativeRunSpec::new(
        spec.workflow_id,
        workflow.min_iters,
        workflow.max_iters,
        spec.convergence_threshold,
        spec.initial_objective,
        spec.drift_per_iter,
        spec.noise_std,
    );
    let mut iter_state = IterState::new(&runtime_spec);
    // Per-iteration ChaCha20 stream from Stream::ConvergenceObjective
    // is rebuilt inside the workflow's call to run_iteration; here
    // we feed it via the same hierarchy used to derive the
    // workflow's task durations.
    let replicate = hierarchy.replicate(spec.replicate_index);

    let mut sim_time: SimTime = 0;
    let mut iterations_run: u32 = 0;
    let mut iteration_events: u64 = 0;

    loop {
        // Submit each task once, sequentially. The Fcfs scheduler
        // accepts the allocation and reports back the completion
        // time of that single task; we advance sim_time and move
        // on. With a 1-core partition the chain is strictly
        // sequential, so every iteration adds exactly
        // `classical_per_iter_ns` to `sim_time`. The integration
        // test exploits this equality as the resource-release
        // semantic check: any over-commit, late-release, or
        // missing-release bug in Fcfs would inflate the total
        // simulated wallclock.
        for task in &workflow.dag.tasks {
            let completion = fcfs.submit(sim_time, /* cores */ 1, task.duration_ns);
            sim_time = completion;
        }

        // Sample the convergence objective for this iteration.
        let mut conv_rng = replicate.stream(qwksim_core::rng::StreamId::ConvergenceObjective {
            workflow_id: spec.workflow_id,
            iter_idx: iterations_run,
        });
        let outcome = run_iteration(&runtime_spec, &mut iter_state, &mut conv_rng);

        let iter_completion_ns = sim_time;
        iterations_run = iter_state.iter_idx;

        sink.iteration(IterationEvent {
            at_ns: iter_completion_ns,
            workflow_id: spec.workflow_id,
            agent_id: VQE_SMOKE_AGENT,
            seq: seq.allocate(),
            iteration: iterations_run.saturating_sub(1),
            classical_ns: classical_per_iter_ns,
            qpu_ns: 0,
            handover_ns: 0,
        })?;
        iteration_events += 1;

        if outcome.is_terminal() {
            break;
        }
    }

    sink.workflow(WorkflowEvent {
        at_ns: sim_time,
        workflow_id: spec.workflow_id,
        agent_id: VQE_SMOKE_AGENT,
        seq: seq.allocate(),
        phase: WorkflowPhase::Completed,
    })?;

    Box::new(sink).finish()?;

    Ok(VqeSmokeResult {
        iterations_run,
        final_smoothed_objective: iter_state.smoothed_objective,
        workflow_events_emitted: 2,
        iteration_events_emitted: iteration_events,
        final_time_ns: sim_time,
        classical_per_iter_ns,
    })
}
