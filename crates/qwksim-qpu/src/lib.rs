//! Per-modality QPU models for `qwksim`.
//!
//! Models the quantum side of each super-site: superconducting and
//! trapped-ion QPUs in the headline configuration plus a photonic
//! stub gated behind sensitivity sweep `sw5`. Each `QpuAgent` carries
//! a `QpuAnchor` (qubits, T1/T2, single-/two-qubit and readout
//! fidelities, gate and readout times, calibration cadence and
//! outage), an `OuDriftState` integrating Ornstein–Uhlenbeck drift on
//! the fidelity channels, a `CompilationCache` keyed by
//! `(template_id, params_hash, qpu_id)` for variational reuse, and a
//! fidelity-class-weighted priority queue for circuit execution. The
//! `IntegrationTightness` flag (`OnPrem` / `Cloud`) selects between
//! hand-over latency models.
//!
//! See `plan/solution_plan.md` §2.3 for the type sketches and §6 for
//! how this composes with bargaining utilities.
