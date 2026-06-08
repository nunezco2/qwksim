//! Per-modality QPU models for `qwksim`.
//!
//! Models the quantum side of each super-site: superconducting and
//! trapped-ion QPUs in the headline configuration plus a photonic
//! stub gated behind sensitivity sweep `sw5`. Each `QpuAgent` carries
//! a [`QpuAnchor`] (qubits, T1/T2, single-/two-qubit and readout
//! fidelities, gate and readout times, calibration cadence and
//! outage), an `OuDriftState` integrating Ornstein–Uhlenbeck drift on
//! the fidelity channels, a `CompilationCache` keyed by
//! `(template_id, params_hash, qpu_id)` for variational reuse, and a
//! fidelity-class-weighted priority queue for circuit execution. The
//! [`IntegrationTightness`] flag (`OnPrem` / `Cloud`) selects
//! between hand-over latency models.
//!
//! See `plan/solution_plan.md` §2.3 for the type sketches and §6 for
//! how this composes with bargaining utilities.
//!
//! Calibration data is materialised in `$OUT_DIR` at build time by
//! `build.rs`, sourced either from `data/vendor/` (when present and
//! SHA-256-matched against `vendor.toml`) or from the deterministic
//! synthetic placeholder shipped under `synthetic/`. See
//! `vendor.toml` for the manifest and Q17.2 = (vc3) for the licence
//! status.

pub mod agent;
pub mod anchor;
pub mod cache;
pub mod calibration;
pub mod drift;

pub use agent::{CircuitExec, FidelityClass, QpuAgent};
pub use anchor::{projected_hand_over_latency_ns, IntegrationTightness, Modality, QpuAnchor};
pub use cache::{CompilationCache, CompilationCacheKey, CompileOutcome, CompiledCircuit};
pub use calibration::{AdmissionRejected, CalibrationEvent, CalibrationSchedule};
pub use drift::{FidelityChannel, OuChannelState, OuDriftState, OuParams, OU_STEP_DT_NS};
