//! Vendor-calibration adapter (T3.8).
//!
//! `build.rs` materialises one JSON calibration sheet per
//! headline modality in `$OUT_DIR`, sourced either from
//! `data/vendor/<name>` (when present and SHA-256-matched against
//! `vendor.toml`) or from the deterministic `synthetic/<name>`
//! placeholder. This module parses those JSON sheets back into
//! [`QpuAnchor`] instances.
//!
//! Per Q17.2 = (vc3) the licence audit is pending, so today the
//! synthetic fallback is always taken; the vendor branch is
//! dead-code until a real SHA is checked in. The schema below is
//! the contract that both paths satisfy.
//!
//! ## JSON schemas
//!
//! The two modalities use *different* natural units in their
//! JSON sheets so each module reads how a vendor sheet would
//! actually arrive on disk. The adapter normalises to the
//! [`QpuAnchor`] canonical `_ns` fields.
//!
//! Superconducting (`calibration_superconducting.json`):
//!
//! ```json
//! {
//!   "modality": "Superconducting",
//!   "qubits": 50,
//!   "t1_us": 80,
//!   "t2_us": 60,
//!   "fidelity_1q": 0.999,
//!   "fidelity_2q": 0.993,
//!   "fidelity_readout": 0.985,
//!   "gate_time_1q_ns": 30,
//!   "gate_time_2q_ns": 200,
//!   "readout_time_us": 1,
//!   "calibration_period_hours": 4,
//!   "calibration_outage_minutes": 15
//! }
//! ```
//!
//! Trapped-ion (`calibration_trapped_ion.json`):
//!
//! ```json
//! {
//!   "modality": "TrappedIon",
//!   "qubits": 30,
//!   "t1_seconds": 10,
//!   "t2_seconds": 1,
//!   "fidelity_1q": 0.9999,
//!   "fidelity_2q": 0.995,
//!   "fidelity_readout": 0.995,
//!   "gate_time_1q_us": 5,
//!   "gate_time_2q_us": 200,
//!   "readout_time_us": 100,
//!   "calibration_period_hours": 8,
//!   "calibration_outage_minutes": 30
//! }
//! ```
//!
//! Extra fields (`source`, `topology`, …) in the JSON are
//! ignored — the adapter only reads the canonical-anchor inputs.

use std::fmt;

use serde::Deserialize;

use crate::{Modality, QpuAnchor};

/// Embedded superconducting calibration JSON (vendor sheet
/// when audit clears; synthetic fallback today).
const SUPERCONDUCTING_JSON: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/calibration_superconducting.json"
));

/// Embedded trapped-ion calibration JSON (vendor sheet when
/// audit clears; synthetic fallback today).
const TRAPPED_ION_JSON: &str =
    include_str!(concat!(env!("OUT_DIR"), "/calibration_trapped_ion.json"));

/// Error type returned by the vendor adapter.
#[derive(Debug)]
pub enum VendorAdapterError {
    /// Underlying JSON deserialisation failed.
    Json(serde_json::Error),
    /// The sheet's `modality` field did not match the modality
    /// the caller asked to parse.
    UnexpectedModality { expected: &'static str, got: String },
}

impl fmt::Display for VendorAdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "vendor sheet JSON parse error: {e}"),
            Self::UnexpectedModality { expected, got } => write!(
                f,
                "vendor sheet modality mismatch: expected {expected}, got {got}",
            ),
        }
    }
}

impl std::error::Error for VendorAdapterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for VendorAdapterError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Parse a superconducting calibration JSON sheet into a
/// [`QpuAnchor`]. Performs the µs → ns / hour → ns / minute → ns
/// unit conversions inline.
pub fn parse_superconducting_sheet(json: &str) -> Result<QpuAnchor, VendorAdapterError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Sheet<'a> {
        modality: &'a str,
        // Optional vendor-side metadata; deserialised so
        // deny_unknown_fields doesn't reject them. None of these
        // map into QpuAnchor.
        #[serde(default)]
        source: Option<&'a str>,
        #[serde(default)]
        topology: Option<&'a str>,
        qubits: u32,
        t1_us: u64,
        t2_us: u64,
        fidelity_1q: f64,
        fidelity_2q: f64,
        fidelity_readout: f64,
        gate_time_1q_ns: u64,
        gate_time_2q_ns: u64,
        readout_time_us: u64,
        calibration_period_hours: u64,
        calibration_outage_minutes: u64,
    }
    let s: Sheet<'_> = serde_json::from_str(json)?;
    if s.modality != "Superconducting" {
        return Err(VendorAdapterError::UnexpectedModality {
            expected: "Superconducting",
            got: s.modality.to_string(),
        });
    }
    Ok(QpuAnchor {
        modality: Modality::Superconducting,
        qubits: s.qubits,
        t1_ns: s.t1_us * 1_000,
        t2_ns: s.t2_us * 1_000,
        fidelity_1q: s.fidelity_1q,
        fidelity_2q: s.fidelity_2q,
        fidelity_readout: s.fidelity_readout,
        gate_time_1q_ns: s.gate_time_1q_ns,
        gate_time_2q_ns: s.gate_time_2q_ns,
        readout_time_ns: s.readout_time_us * 1_000,
        calibration_period_ns: s.calibration_period_hours * 3_600 * 1_000_000_000,
        calibration_outage_ns: s.calibration_outage_minutes * 60 * 1_000_000_000,
    })
}

/// Parse a trapped-ion calibration JSON sheet into a
/// [`QpuAnchor`]. Performs the s → ns / µs → ns / hour → ns /
/// minute → ns unit conversions inline.
pub fn parse_trapped_ion_sheet(json: &str) -> Result<QpuAnchor, VendorAdapterError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Sheet<'a> {
        modality: &'a str,
        #[serde(default)]
        source: Option<&'a str>,
        #[serde(default)]
        topology: Option<&'a str>,
        qubits: u32,
        t1_seconds: u64,
        t2_seconds: u64,
        fidelity_1q: f64,
        fidelity_2q: f64,
        fidelity_readout: f64,
        gate_time_1q_us: u64,
        gate_time_2q_us: u64,
        readout_time_us: u64,
        calibration_period_hours: u64,
        calibration_outage_minutes: u64,
    }
    let s: Sheet<'_> = serde_json::from_str(json)?;
    if s.modality != "TrappedIon" {
        return Err(VendorAdapterError::UnexpectedModality {
            expected: "TrappedIon",
            got: s.modality.to_string(),
        });
    }
    Ok(QpuAnchor {
        modality: Modality::TrappedIon,
        qubits: s.qubits,
        t1_ns: s.t1_seconds * 1_000_000_000,
        t2_ns: s.t2_seconds * 1_000_000_000,
        fidelity_1q: s.fidelity_1q,
        fidelity_2q: s.fidelity_2q,
        fidelity_readout: s.fidelity_readout,
        gate_time_1q_ns: s.gate_time_1q_us * 1_000,
        gate_time_2q_ns: s.gate_time_2q_us * 1_000,
        readout_time_ns: s.readout_time_us * 1_000,
        calibration_period_ns: s.calibration_period_hours * 3_600 * 1_000_000_000,
        calibration_outage_ns: s.calibration_outage_minutes * 60 * 1_000_000_000,
    })
}

/// Headline superconducting [`QpuAnchor`] sourced from the
/// embedded JSON sheet (vendor when audit clears; synthetic
/// fallback today).
///
/// Panics only if the JSON sheet is malformed — that would be a
/// build-system bug since the bytes are checked in.
pub fn headline_superconducting_anchor() -> QpuAnchor {
    parse_superconducting_sheet(SUPERCONDUCTING_JSON)
        .expect("embedded superconducting calibration JSON must parse")
}

/// Headline trapped-ion [`QpuAnchor`] sourced from the embedded
/// JSON sheet.
pub fn headline_trapped_ion_anchor() -> QpuAnchor {
    parse_trapped_ion_sheet(TRAPPED_ION_JSON)
        .expect("embedded trapped-ion calibration JSON must parse")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_superconducting_anchor_matches_synthetic_q9_1_values() {
        // The synthetic SC sheet pins the Q9.1 headline anchor;
        // the adapter must reproduce it verbatim modulo unit
        // conversion.
        let a = headline_superconducting_anchor();
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
        assert_eq!(a.calibration_period_ns, 4 * 3_600 * 1_000_000_000);
        assert_eq!(a.calibration_outage_ns, 15 * 60 * 1_000_000_000);
    }

    #[test]
    fn embedded_trapped_ion_anchor_matches_synthetic_q9_1_values() {
        let a = headline_trapped_ion_anchor();
        assert_eq!(a.modality, Modality::TrappedIon);
        assert_eq!(a.qubits, 30);
        assert_eq!(a.t1_ns, 10_000_000_000);
        assert_eq!(a.t2_ns, 1_000_000_000);
        assert!((a.fidelity_1q - 0.9999).abs() < 1e-12);
        assert!((a.fidelity_2q - 0.995).abs() < 1e-12);
        assert!((a.fidelity_readout - 0.995).abs() < 1e-12);
        assert_eq!(a.gate_time_1q_ns, 5_000);
        assert_eq!(a.gate_time_2q_ns, 200_000);
        assert_eq!(a.readout_time_ns, 100_000);
        assert_eq!(a.calibration_period_ns, 8 * 3_600 * 1_000_000_000);
        assert_eq!(a.calibration_outage_ns, 30 * 60 * 1_000_000_000);
    }

    #[test]
    fn modality_mismatch_produces_typed_error() {
        // Feed a trapped-ion JSON into the superconducting
        // adapter — it must refuse with UnexpectedModality, not
        // silently misinterpret unit fields.
        let bad = r#"{
            "modality": "TrappedIon",
            "qubits": 30,
            "t1_us": 80,
            "t2_us": 60,
            "fidelity_1q": 0.999,
            "fidelity_2q": 0.993,
            "fidelity_readout": 0.985,
            "gate_time_1q_ns": 30,
            "gate_time_2q_ns": 200,
            "readout_time_us": 1,
            "calibration_period_hours": 4,
            "calibration_outage_minutes": 15
        }"#;
        match parse_superconducting_sheet(bad) {
            Err(VendorAdapterError::UnexpectedModality { expected, got }) => {
                assert_eq!(expected, "Superconducting");
                assert_eq!(got, "TrappedIon");
            }
            other => panic!("expected UnexpectedModality, got {other:?}"),
        }
    }

    #[test]
    fn unknown_field_in_sheet_is_rejected() {
        // The two sheet schemas are closed (deny_unknown_fields)
        // so a typo in a real vendor sheet fails fast instead of
        // silently dropping data.
        let bad = r#"{
            "modality": "Superconducting",
            "qubits": 50,
            "t1_us": 80,
            "t2_us": 60,
            "fidelity_1q": 0.999,
            "fidelity_2q": 0.993,
            "fidelity_readout": 0.985,
            "gate_time_1q_ns": 30,
            "gate_time_2q_ns": 200,
            "readout_time_us": 1,
            "calibration_period_hours": 4,
            "calibration_outage_minutes": 15,
            "completely_made_up_field": "oops"
        }"#;
        assert!(matches!(
            parse_superconducting_sheet(bad),
            Err(VendorAdapterError::Json(_)),
        ));
    }
}
