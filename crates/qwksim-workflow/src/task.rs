//! Task descriptor — the per-DAG-node resource-demand vector
//! (classical) or circuit specification (quantum), per §5.1 of
//! the engineering plan.
//!
//! Each task carries: CPU cores, GPU count, memory capacity and
//! bandwidth, scratch capacity and I/O bandwidth, and either a
//! deterministic classical duration *or* a [`QuantumDescriptor`]
//! (template id, parameter-vector length, target modality, shot
//! count). Per-outgoing-edge data volumes live on the
//! [`DagEdge`](crate::DagEdge) rather than on `Task` itself so
//! the task descriptor stays edge-shape-agnostic.

use qwksim_core::event::SimTime;

/// The two kinds of work a task can describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    /// Pure-classical work — duration is `duration_ns` from the
    /// enclosing [`Task`].
    Classical,
    /// Quantum work — the circuit is described by
    /// [`QuantumDescriptor`]. Per-shot execution time and total
    /// duration land later in Phase 3 once the QPU model
    /// computes them from modality + circuit shape.
    Quantum(QuantumDescriptor),
}

/// QPU modality (Q9.1 anchors). `Photonic` is reserved for the
/// `sw5` sensitivity sweep; the headline uses only
/// `Superconducting` and `TrappedIon`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QpuModality {
    Superconducting,
    TrappedIon,
    Photonic,
}

/// Circuit-level description for a quantum task. Concrete
/// transpilation / shot-by-shot execution lands in
/// `qwksim-qpu` during Phase 3; this is the data carried in the
/// DAG node today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QuantumDescriptor {
    /// Identifier of a parametrised-circuit template registered
    /// with the QPU compilation cache (Q9.5 = c3). Reuse across
    /// iterations of the same variational workflow hits the
    /// cache by `template_id × parameter_structure_hash`.
    pub template_id: u32,
    /// Length of the parameter vector this template consumes.
    /// Combined with the workflow's `Stream::WorkflowAttributes`
    /// draws (Q11.4 + T0.12), this determines the actual
    /// parameter values used per iteration.
    pub parameter_count: u32,
    /// Preferred QPU modality. `None` = "any modality" (the
    /// router uses modality-affinity routing under Q4.1 = iv).
    pub modality: Option<QpuModality>,
    /// Number of shots executed per iteration.
    pub shots: u32,
}

/// Per-task resource-demand vector + duration / circuit
/// descriptor.
///
/// Cheap to copy; immutable once constructed. Downstream
/// simulator code reads fields by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Task {
    /// Discriminator: classical work vs. quantum circuit.
    pub kind: TaskKind,
    /// CPU cores requested.
    pub cores: u32,
    /// GPUs requested.
    pub gpus: u32,
    /// Memory capacity required (bytes).
    pub memory_bytes: u64,
    /// Memory bandwidth required (bytes per second).
    pub memory_bandwidth_bps: u64,
    /// Scratch storage capacity required (bytes).
    pub scratch_capacity_bytes: u64,
    /// Scratch I/O bandwidth required (bytes per second).
    pub scratch_io_bps: u64,
    /// Deterministic classical duration. For
    /// `TaskKind::Quantum`, this is the *classical pre/post
    /// duration* (transpilation, parameter prep, post-processing)
    /// — the QPU body's duration lives in the descriptor and is
    /// computed by `qwksim-qpu`.
    pub duration_ns: SimTime,
}

impl Task {
    /// Build a `Classical` task with no resource demands except
    /// the duration — useful for quick fixtures and the Phase-2
    /// integration tests. Mutate fields directly to add
    /// demands.
    pub fn classical(duration_ns: SimTime) -> Self {
        Self {
            kind: TaskKind::Classical,
            cores: 0,
            gpus: 0,
            memory_bytes: 0,
            memory_bandwidth_bps: 0,
            scratch_capacity_bytes: 0,
            scratch_io_bps: 0,
            duration_ns,
        }
    }

    /// Build a `Quantum` task carrying `descriptor` and the
    /// classical pre/post `duration_ns`.
    pub fn quantum(descriptor: QuantumDescriptor, duration_ns: SimTime) -> Self {
        Self {
            kind: TaskKind::Quantum(descriptor),
            cores: 0,
            gpus: 0,
            memory_bytes: 0,
            memory_bandwidth_bps: 0,
            scratch_capacity_bytes: 0,
            scratch_io_bps: 0,
            duration_ns,
        }
    }

    /// Mutator that returns `self` for builder-style construction.
    /// `cores`, `gpus`, etc. mutators all follow the same shape.
    pub fn with_cores(mut self, cores: u32) -> Self {
        self.cores = cores;
        self
    }

    pub fn with_gpus(mut self, gpus: u32) -> Self {
        self.gpus = gpus;
        self
    }

    pub fn with_memory(mut self, bytes: u64, bandwidth_bps: u64) -> Self {
        self.memory_bytes = bytes;
        self.memory_bandwidth_bps = bandwidth_bps;
        self
    }

    pub fn with_scratch(mut self, capacity_bytes: u64, io_bps: u64) -> Self {
        self.scratch_capacity_bytes = capacity_bytes;
        self.scratch_io_bps = io_bps;
        self
    }

    /// `true` iff the task is a quantum task.
    pub fn is_quantum(&self) -> bool {
        matches!(self.kind, TaskKind::Quantum(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classical_task_builder_sets_defaults_and_duration() {
        let t = Task::classical(1_000);
        assert_eq!(t.kind, TaskKind::Classical);
        assert_eq!(t.cores, 0);
        assert_eq!(t.gpus, 0);
        assert_eq!(t.memory_bytes, 0);
        assert_eq!(t.scratch_io_bps, 0);
        assert_eq!(t.duration_ns, 1_000);
        assert!(!t.is_quantum());
    }

    #[test]
    fn quantum_task_carries_descriptor() {
        let desc = QuantumDescriptor {
            template_id: 42,
            parameter_count: 6,
            modality: Some(QpuModality::TrappedIon),
            shots: 1024,
        };
        let t = Task::quantum(desc, 500);
        assert!(t.is_quantum());
        match t.kind {
            TaskKind::Quantum(d) => {
                assert_eq!(d.template_id, 42);
                assert_eq!(d.shots, 1024);
                assert_eq!(d.modality, Some(QpuModality::TrappedIon));
            }
            _ => panic!("expected Quantum kind"),
        }
        assert_eq!(t.duration_ns, 500);
    }

    #[test]
    fn builders_chain() {
        let t = Task::classical(100)
            .with_cores(8)
            .with_gpus(2)
            .with_memory(1_024 * 1_024 * 1_024, 50_000_000_000)
            .with_scratch(100_000_000_000, 4_000_000_000);
        assert_eq!(t.cores, 8);
        assert_eq!(t.gpus, 2);
        assert_eq!(t.memory_bytes, 1_073_741_824);
        assert_eq!(t.memory_bandwidth_bps, 50_000_000_000);
        assert_eq!(t.scratch_capacity_bytes, 100_000_000_000);
        assert_eq!(t.scratch_io_bps, 4_000_000_000);
    }
}
