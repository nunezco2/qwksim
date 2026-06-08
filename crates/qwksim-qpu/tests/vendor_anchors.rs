//! Integration tests for **T3.8** — vendor-calibration adapter.
//!
//! Acceptance gates (from #46):
//!
//! 1. **A scenario boots with vendor-data-derived anchors** —
//!    a `QpuAgent` constructs and admits a circuit using the
//!    anchor that comes off the embedded JSON sheet.
//! 2. **Synthetic fallback path is identical schema** — feeding
//!    a hand-rolled vendor-style JSON through the same adapter
//!    produces a fully-populated `QpuAnchor`, proving the two
//!    paths (vendor and synthetic) share one schema contract.

use qwksim_qpu::{
    headline_superconducting_anchor, headline_trapped_ion_anchor, parse_superconducting_sheet,
    parse_trapped_ion_sheet, CircuitExec, FidelityClass, IntegrationTightness, Modality, QpuAgent,
};

#[test]
fn scenario_boots_with_vendor_data_derived_superconducting_anchor() {
    // The build.rs writes calibration_superconducting.json into
    // $OUT_DIR (synthetic fallback today; vendor sheet once the
    // SHA-256 audit clears). Either way, `headline_superconducting_anchor`
    // returns a fully-populated QpuAnchor that boots a QpuAgent.
    let anchor = headline_superconducting_anchor();
    assert_eq!(anchor.modality, Modality::Superconducting);
    assert!(anchor.qubits > 0);
    assert!(anchor.t1_ns > 0);
    assert!(anchor.t2_ns > 0);
    assert!(anchor.calibration_period_ns > 0);
    assert!(anchor.calibration_outage_ns > 0);
    assert!(anchor.calibration_outage_ns < anchor.calibration_period_ns);

    let mut agent = QpuAgent::new(0, anchor, IntegrationTightness::OnPrem);
    agent
        .admit_circuit(CircuitExec::new(1, FidelityClass::Standard), 0)
        .expect("operational admit on a freshly-booted SC scenario");
    assert_eq!(agent.pending_circuits(), 1);
    assert_eq!(agent.anchor().modality, Modality::Superconducting);
}

#[test]
fn scenario_boots_with_vendor_data_derived_trapped_ion_anchor() {
    let anchor = headline_trapped_ion_anchor();
    assert_eq!(anchor.modality, Modality::TrappedIon);
    assert!(anchor.qubits > 0);
    assert!(anchor.t1_ns > 0);
    assert!(anchor.t2_ns > 0);
    // Trapped-ion T1 is seconds-scale; sanity-check the
    // s → ns conversion landed in the right ballpark (≥ 1 s =
    // 10⁹ ns) instead of micro-seconds or another unit slip.
    assert!(
        anchor.t1_ns >= 1_000_000_000,
        "trapped-ion T1 {} ns < 1 s — unit conversion likely wrong",
        anchor.t1_ns,
    );

    let mut agent = QpuAgent::new(1, anchor, IntegrationTightness::OnPrem);
    agent
        .admit_circuit(CircuitExec::new(2, FidelityClass::High), 0)
        .expect("operational admit on a freshly-booted TI scenario");
    assert_eq!(agent.anchor().modality, Modality::TrappedIon);
}

/// THE second acceptance gate: feed a hand-rolled vendor-style
/// JSON through the SAME public adapter and assert it produces
/// the same QpuAnchor shape as the embedded sheet. This proves
/// the vendor branch and the synthetic branch are interchangeable
/// — when a real vendor file lands tomorrow, the schema
/// contract already passes.
#[test]
fn synthetic_fallback_and_vendor_path_share_identical_schema() {
    let vendor_style_json = r#"{
        "modality": "Superconducting",
        "qubits": 127,
        "t1_us": 120,
        "t2_us": 90,
        "fidelity_1q": 0.99965,
        "fidelity_2q": 0.992,
        "fidelity_readout": 0.987,
        "gate_time_1q_ns": 35,
        "gate_time_2q_ns": 240,
        "readout_time_us": 2,
        "calibration_period_hours": 6,
        "calibration_outage_minutes": 20
    }"#;

    let from_vendor =
        parse_superconducting_sheet(vendor_style_json).expect("vendor-style sheet parses");
    let from_synthetic = headline_superconducting_anchor();

    // Same Modality, same QpuAnchor field shape — both can drop
    // straight into QpuAgent::new without any per-source
    // branching.
    assert_eq!(from_vendor.modality, from_synthetic.modality);

    // Every numeric field is reachable on both anchors. The
    // *values* differ (different vendor / synthetic snapshots),
    // but the schema contract — every QpuAnchor field populated
    // with a non-zero value where a non-zero value makes sense —
    // is identical.
    for anchor in [from_vendor, from_synthetic] {
        assert!(anchor.qubits > 0);
        assert!(anchor.t1_ns > 0);
        assert!(anchor.t2_ns > 0);
        assert!((0.0..=1.0).contains(&anchor.fidelity_1q));
        assert!((0.0..=1.0).contains(&anchor.fidelity_2q));
        assert!((0.0..=1.0).contains(&anchor.fidelity_readout));
        assert!(anchor.gate_time_1q_ns > 0);
        assert!(anchor.gate_time_2q_ns > 0);
        assert!(anchor.readout_time_ns > 0);
        assert!(anchor.calibration_period_ns > 0);
        assert!(anchor.calibration_outage_ns > 0);
        assert!(anchor.calibration_outage_ns < anchor.calibration_period_ns);
        // Both anchors must boot a QpuAgent without panicking.
        let _ = QpuAgent::new(99, anchor, IntegrationTightness::OnPrem);
    }
}

#[test]
fn vendor_style_trapped_ion_json_parses_via_same_adapter() {
    // Mirror of the SC schema gate above for trapped ion.
    let vendor_style_json = r#"{
        "modality": "TrappedIon",
        "qubits": 64,
        "t1_seconds": 30,
        "t2_seconds": 3,
        "fidelity_1q": 0.99995,
        "fidelity_2q": 0.997,
        "fidelity_readout": 0.998,
        "gate_time_1q_us": 4,
        "gate_time_2q_us": 180,
        "readout_time_us": 80,
        "calibration_period_hours": 12,
        "calibration_outage_minutes": 45
    }"#;
    let from_vendor =
        parse_trapped_ion_sheet(vendor_style_json).expect("vendor-style TI sheet parses");
    let from_synthetic = headline_trapped_ion_anchor();
    assert_eq!(from_vendor.modality, from_synthetic.modality);
    // Hand-rolled vendor snapshot pinned values.
    assert_eq!(from_vendor.qubits, 64);
    assert_eq!(from_vendor.t1_ns, 30 * 1_000_000_000);
    assert_eq!(from_vendor.gate_time_1q_ns, 4 * 1_000);
    assert_eq!(
        from_vendor.calibration_period_ns,
        12 * 3_600 * 1_000_000_000
    );
}
