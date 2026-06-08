//! Per-modality QPU anchor — calibration snapshot a downstream
//! agent reads to model gate execution, OU drift, mid-circuit
//! feedback, etc. Maps directly to §2.3's type sketch and Q9.1
//! anchors.
//!
//! ## TOML deserialisation
//!
//! `QpuAnchor` derives `serde::Deserialize`. The TOML field names
//! use the type's snake_case field names verbatim, with an
//! explicit `_ns` suffix on every time-typed field so the unit
//! is unambiguous in the config file:
//!
//! ```toml
//! modality = "superconducting"
//! qubits = 50
//! t1_ns = 80_000
//! t2_ns = 60_000
//! fidelity_1q = 0.999
//! fidelity_2q = 0.993
//! fidelity_readout = 0.985
//! gate_time_1q_ns = 30
//! gate_time_2q_ns = 200
//! readout_time_ns = 1_000
//! calibration_period_ns = 14_400_000_000_000
//! calibration_outage_ns = 900_000_000_000
//! ```
//!
//! [`IntegrationTightness`] is **not** carried on the anchor.
//! Hand-over latency is projected from `(anchor, tightness)` via
//! [`projected_hand_over_latency_ns`] so the same anchor can
//! drive both an on-prem and a cloud QPU in different sensitivity
//! sweeps without re-deriving the TOML.

use serde::Deserialize;

use qwksim_core::event::SimTime;

/// QPU modality (Q9.1 anchors). `Photonic` is reserved for the
/// `sw5` sensitivity sweep; the headline uses only
/// `Superconducting` and `TrappedIon`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Superconducting,
    TrappedIon,
    Photonic,
}

impl Modality {
    /// Per-modality deterministic constant added to circuit
    /// execution time when the circuit uses **mid-circuit
    /// feedback** (T3.6). Captures the classical-control
    /// feedback loop the modality incurs between a mid-circuit
    /// measurement and the conditional gate that follows.
    ///
    /// | modality          | feedback latency |
    /// |-------------------|------------------|
    /// | Superconducting   |   1 µs           |
    /// | TrappedIon        | 100 µs           |
    /// | Photonic          |  10 µs           |
    ///
    /// Values are pinned per the Q9.6 modality envelope: a fast
    /// classical FPGA control loop on superconducting; the
    /// slower ion measurement-plus-laser-control on trapped ion;
    /// a conservative placeholder for the `sw5` photonic
    /// sensitivity sweep.
    pub fn mid_circuit_feedback_latency_ns(self) -> SimTime {
        match self {
            Modality::Superconducting => 1_000,
            Modality::TrappedIon => 100_000,
            Modality::Photonic => 10_000,
        }
    }
}

/// Tightness of integration between the simulator's HPC partition
/// and the QPU. Determines hand-over latency (Q10.1 = h4 anchor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationTightness {
    #[default]
    OnPrem,
    Cloud,
}

/// Per-modality QPU calibration anchor.
///
/// Every field has a stable unit. Time fields are `SimTime`
/// (`u64` nanoseconds). Fidelities are `f64 ∈ [0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct QpuAnchor {
    /// Modality the anchor was calibrated against.
    pub modality: Modality,
    /// Physical qubit count.
    pub qubits: u32,
    /// T1 relaxation time (ns).
    pub t1_ns: SimTime,
    /// T2 coherence time (ns).
    pub t2_ns: SimTime,
    /// Single-qubit gate fidelity.
    pub fidelity_1q: f64,
    /// Two-qubit gate fidelity.
    pub fidelity_2q: f64,
    /// Readout fidelity.
    pub fidelity_readout: f64,
    /// Single-qubit gate time (ns).
    pub gate_time_1q_ns: SimTime,
    /// Two-qubit gate time (ns).
    pub gate_time_2q_ns: SimTime,
    /// Readout time (ns).
    pub readout_time_ns: SimTime,
    /// Time between calibration outages (ns).
    pub calibration_period_ns: SimTime,
    /// Duration of each calibration outage (ns).
    pub calibration_outage_ns: SimTime,
}

impl QpuAnchor {
    /// Parse an anchor from a TOML string.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }
}

/// Project the hand-over latency for `anchor` running under
/// `tightness`. The function is total — every (modality,
/// tightness) pair maps to a deterministic nanosecond value.
///
/// Per Q10.1 (Reading A: headline on-prem; cloud values reserved
/// for the `sw5` sensitivity sweep):
///
/// | modality          | OnPrem   | Cloud      |
/// |-------------------|----------|------------|
/// | Superconducting   | 5 ms     | 50 ms mean |
/// | TrappedIon        | 10 ms    | 100 ms mean|
/// | Photonic          | 1 ms     | 200 ms mean|
///
/// The cloud row is the **arithmetic mean** of the lognormal
/// hand-over latency that lands later in T3.5 (the projection
/// here is the closed-form mean for use by the bargaining
/// utility without sampling).
pub fn projected_hand_over_latency_ns(
    anchor: &QpuAnchor,
    tightness: IntegrationTightness,
) -> SimTime {
    match (anchor.modality, tightness) {
        (Modality::Superconducting, IntegrationTightness::OnPrem) => 5_000_000,
        (Modality::TrappedIon, IntegrationTightness::OnPrem) => 10_000_000,
        (Modality::Photonic, IntegrationTightness::OnPrem) => 1_000_000,
        (Modality::Superconducting, IntegrationTightness::Cloud) => 50_000_000,
        (Modality::TrappedIon, IntegrationTightness::Cloud) => 100_000_000,
        (Modality::Photonic, IntegrationTightness::Cloud) => 200_000_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SC_TOML: &str = r#"
        modality = "superconducting"
        qubits = 50
        t1_ns = 80_000
        t2_ns = 60_000
        fidelity_1q = 0.999
        fidelity_2q = 0.993
        fidelity_readout = 0.985
        gate_time_1q_ns = 30
        gate_time_2q_ns = 200
        readout_time_ns = 1_000
        calibration_period_ns = 14_400_000_000_000
        calibration_outage_ns = 900_000_000_000
    "#;

    const TI_TOML: &str = r#"
        modality = "trapped_ion"
        qubits = 30
        t1_ns = 10_000_000_000
        t2_ns = 1_000_000_000
        fidelity_1q = 0.9999
        fidelity_2q = 0.995
        fidelity_readout = 0.995
        gate_time_1q_ns = 5_000
        gate_time_2q_ns = 200_000
        readout_time_ns = 100_000
        calibration_period_ns = 28_800_000_000_000
        calibration_outage_ns = 1_800_000_000_000
    "#;

    #[test]
    fn deserialises_superconducting_q9_1_anchor() {
        let a = QpuAnchor::from_toml(SC_TOML).expect("parse SC anchor");
        assert_eq!(a.modality, Modality::Superconducting);
        assert_eq!(a.qubits, 50);
        assert_eq!(a.t1_ns, 80_000);
        assert_eq!(a.t2_ns, 60_000);
        assert!((a.fidelity_1q - 0.999).abs() < 1e-12);
        assert!((a.fidelity_2q - 0.993).abs() < 1e-12);
        assert!((a.fidelity_readout - 0.985).abs() < 1e-12);
        assert_eq!(a.gate_time_1q_ns, 30);
        assert_eq!(a.gate_time_2q_ns, 200);
        assert_eq!(a.readout_time_ns, 1_000);
        assert_eq!(a.calibration_period_ns, 14_400_000_000_000);
        assert_eq!(a.calibration_outage_ns, 900_000_000_000);
    }

    #[test]
    fn deserialises_trapped_ion_q9_1_anchor() {
        let a = QpuAnchor::from_toml(TI_TOML).expect("parse TI anchor");
        assert_eq!(a.modality, Modality::TrappedIon);
        assert_eq!(a.qubits, 30);
        assert_eq!(a.gate_time_1q_ns, 5_000);
    }

    #[test]
    fn flipping_integration_tightness_changes_latency_not_anchor() {
        // T3.1 acceptance gate: parse the anchor once, project
        // the hand-over latency under both tightness values, and
        // assert the anchor itself is byte-identical between the
        // two projections.
        let anchor = QpuAnchor::from_toml(SC_TOML).expect("parse SC anchor");
        let before = anchor;

        let on_prem = projected_hand_over_latency_ns(&anchor, IntegrationTightness::OnPrem);
        let cloud = projected_hand_over_latency_ns(&anchor, IntegrationTightness::Cloud);

        assert_eq!(on_prem, 5_000_000, "SC on-prem hand-over = 5 ms");
        assert_eq!(cloud, 50_000_000, "SC cloud hand-over = 50 ms");
        assert_ne!(on_prem, cloud, "tightness must shift latency");

        assert_eq!(
            anchor, before,
            "anchor must remain byte-identical across projections"
        );
    }

    #[test]
    fn projected_latency_table_matches_q10_1_anchors() {
        // One row per (modality, tightness) combination.
        let cases: &[(Modality, IntegrationTightness, SimTime)] = &[
            (
                Modality::Superconducting,
                IntegrationTightness::OnPrem,
                5_000_000,
            ),
            (
                Modality::TrappedIon,
                IntegrationTightness::OnPrem,
                10_000_000,
            ),
            (Modality::Photonic, IntegrationTightness::OnPrem, 1_000_000),
            (
                Modality::Superconducting,
                IntegrationTightness::Cloud,
                50_000_000,
            ),
            (
                Modality::TrappedIon,
                IntegrationTightness::Cloud,
                100_000_000,
            ),
            (Modality::Photonic, IntegrationTightness::Cloud, 200_000_000),
        ];
        for (modality, tightness, expected) in cases {
            let anchor = QpuAnchor {
                modality: *modality,
                qubits: 1,
                t1_ns: 1,
                t2_ns: 1,
                fidelity_1q: 1.0,
                fidelity_2q: 1.0,
                fidelity_readout: 1.0,
                gate_time_1q_ns: 1,
                gate_time_2q_ns: 1,
                readout_time_ns: 1,
                calibration_period_ns: 1,
                calibration_outage_ns: 1,
            };
            assert_eq!(
                projected_hand_over_latency_ns(&anchor, *tightness),
                *expected,
                "({modality:?}, {tightness:?}) expected {expected} ns",
            );
        }
    }

    #[test]
    fn integration_tightness_defaults_to_on_prem() {
        let t: IntegrationTightness = Default::default();
        assert_eq!(t, IntegrationTightness::OnPrem);
    }

    #[test]
    fn unknown_modality_string_fails_deserialisation() {
        let bad = r#"
            modality = "ising_machine"
            qubits = 1
            t1_ns = 1
            t2_ns = 1
            fidelity_1q = 1.0
            fidelity_2q = 1.0
            fidelity_readout = 1.0
            gate_time_1q_ns = 1
            gate_time_2q_ns = 1
            readout_time_ns = 1
            calibration_period_ns = 1
            calibration_outage_ns = 1
        "#;
        assert!(QpuAnchor::from_toml(bad).is_err());
    }

    #[test]
    fn mid_circuit_feedback_latency_constants_are_pinned_per_modality() {
        // T3.6: the per-modality classical-control feedback
        // constants are part of the simulator's headline
        // physics envelope (Q9.6). Pin them so a future refactor
        // cannot silently re-key headline scenario results.
        assert_eq!(
            Modality::Superconducting.mid_circuit_feedback_latency_ns(),
            1_000
        );
        assert_eq!(
            Modality::TrappedIon.mid_circuit_feedback_latency_ns(),
            100_000
        );
        assert_eq!(Modality::Photonic.mid_circuit_feedback_latency_ns(), 10_000);
    }
}
