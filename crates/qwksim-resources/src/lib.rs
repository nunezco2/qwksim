//! HPC resource models for `qwksim`.
//!
//! Hosts the per-resource agents that participate in cooperative
//! bargaining within each super-site: `HpcPartitionAgent`,
//! `GpuPoolAgent`, `PortalAgent`, plus the supporting fluid-bandwidth
//! pools (`FluidBandwidthPool`, `ScratchIoPool` — FLAG-I closed:
//! linear-sharing fluid model) and the shared-constraint rivalry term
//! (`ResourceContentionView` — FLAG-J closed). The crate also
//! defines the `ResourceAgent` trait that every per-resource agent
//! implements: advertised summary, utility evaluator, allocation
//! accept/release.
//!
//! See `plan/solution_plan.md` §2.2 for responsibilities and the
//! per-modality envelopes in §2.3 for how this composes with the
//! `qwksim-qpu` crate.
