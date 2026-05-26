//! HPC resource models for `qwksim`.
//!
//! Hosts the per-resource agents that participate in cooperative
//! bargaining within each super-site: `HpcPartitionAgent`,
//! `GpuPoolAgent`, `PortalAgent`, plus the supporting fluid-bandwidth
//! pools (`FluidBandwidthPool`, `ScratchIoPool` — FLAG-I closed:
//! linear-sharing fluid model) and the shared-constraint rivalry term
//! (`ResourceContentionView` — FLAG-J closed). The crate also
//! defines the [`ResourceAgent`] trait that every per-resource agent
//! implements: advertised summary, utility evaluator, allocation
//! accept/release.
//!
//! See `plan/solution_plan.md` §2.2 for responsibilities and the
//! per-modality envelopes in §2.3 for how this composes with the
//! `qwksim-qpu` crate.

pub mod bandwidth;
pub mod gpu;
pub mod hpc;

pub use bandwidth::{Bandwidth, MemoryBandwidthPool, StreamId};
pub use gpu::GpuPoolAgent;
pub use hpc::HpcPartitionAgent;

use qwksim_core::event::{AgentId, SimTime};
use qwksim_scheduler::View;

/// Per-resource agent's snapshot of its own state at simulator-time
/// `now`. Aggregated across agents to form the
/// `AdvertisedState` consumed by [`qwksim_scheduler::Scheduler`] in
/// the `Local` view variant.
///
/// Each per-resource agent populates only the fields it owns and
/// leaves the rest at zero (the `Default` value). Today:
///
/// - [`HpcPartitionAgent`] populates `free_cores` / `total_cores`.
/// - [`GpuPoolAgent`] populates `free_gpus` / `total_gpus`.
///
/// Memory bandwidth, scratch I/O, fidelity-class queue depth, etc.
/// land alongside their owning resource agents in later Phase-2
/// PRs and Phase-3 (QPU).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdvertisedSummary {
    /// CPU cores currently unallocated.
    pub free_cores: u32,
    /// Configured total CPU cores at this agent (`0` if not an
    /// HPC partition).
    pub total_cores: u32,
    /// GPUs currently unallocated.
    pub free_gpus: u32,
    /// Configured total GPUs at this agent (`0` if not a GPU
    /// pool).
    pub total_gpus: u32,
}

/// One reservation handed to (or held by) a resource agent.
///
/// Aggregate request shape — each per-resource agent reads only
/// the fields it manages, so a workflow that needs *N* cores and
/// *M* GPUs can describe the request once and dispatch the same
/// `Allocation` to both the HPC partition and the GPU pool
/// agents. Zero on a field means "this agent has nothing to do".
///
/// Marker fields today; full bargaining-bundle reservation shape
/// (memory bandwidth, scratch, QPU shots, fidelity class, …)
/// lands in T4.x.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Allocation {
    /// CPU cores requested.
    pub cores: u32,
    /// GPUs requested.
    pub gpus: u32,
}

/// Common interface every per-resource agent implements.
///
/// The bargaining solver (Phase 4) consumes the trait only — never
/// the concrete agent type — so new resource categories (GPU pool,
/// QPU, network, scratch I/O) can be added without touching the
/// solver. See `plan/solution_plan.md` §4.1.
pub trait ResourceAgent: Send {
    /// Stable identifier for this agent within its super-site. Used
    /// as the second coordinate of the `(at, agent, seq)` event
    /// tie-break key.
    fn id(&self) -> AgentId;

    /// Snapshot of the agent's currently-advertised free capacity
    /// at simulator-time `now`. Today the snapshot is live; once
    /// τ_adv staleness lands (T4.x advertisement protocol) the
    /// summary may lag — but per the proptest invariant the
    /// advertised free capacity must **never exceed** the true
    /// free capacity at the same instant.
    fn advertised_summary(&self, now: SimTime) -> AdvertisedSummary;

    /// Return the agent's utility for hypothetically taking on
    /// `alloc` given the bargaining `view`. Stubbed for now;
    /// concrete formula (FLAG-C) lands with the Nash-product
    /// solver in T4.x.
    fn utility(&self, alloc: &Allocation, view: &View<'_>) -> f64;

    /// Commit `alloc` to the agent's books at simulator-time `now`.
    fn accept(&mut self, alloc: Allocation, now: SimTime);

    /// Release `alloc` from the agent's books at simulator-time
    /// `now`. Callers must release only allocations previously
    /// accepted; the trait does not enforce this — concrete impls
    /// may saturate-subtract to be defensive.
    fn release(&mut self, alloc: &Allocation, now: SimTime);
}
