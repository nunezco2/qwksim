//! Calibration boundary scheduling + outage admission (T3.4).
//!
//! Each QPU calibrates on a fixed cadence: every
//! [`QpuAnchor::calibration_period_ns`](crate::QpuAnchor) the
//! QPU enters a [`CalibrationEvent::OutageBegin`] window of
//! [`QpuAnchor::calibration_outage_ns`](crate::QpuAnchor); at
//! the end of the window a [`CalibrationEvent::BoundaryReset`]
//! abruptly returns the OU drift state to the anchor's nominal
//! fidelity values.
//!
//! ## Cycle layout
//!
//! Cycle `k` spans `[k · period_ns, (k + 1) · period_ns)`. Within
//! a cycle:
//!
//! - Operational window: `[k · period, k · period + (period −
//!   outage))`.
//! - Outage window:     `[k · period + (period − outage),
//!   (k + 1) · period)` — the QPU rejects new admission requests
//!   here.
//! - Boundary reset at `(k + 1) · period` — the drift state is
//!   reset back to the anchor's nominal values.
//!
//! `t = 0` is the *initial* calibration (no preceding outage);
//! cycle 0's outage spans `[period − outage, period)` and the
//! first boundary reset lands at `period`.
//!
//! ## What does *not* live here
//!
//! - The DES wiring that actually pumps the
//!   [`CalibrationEvent`] sequence into the simulator event
//!   queue lands in the Phase-6 harness. Today the events are
//!   queryable from the schedule via [`CalibrationSchedule::next_event_after`]
//!   so the future DES integration is a thin glue layer.

use qwksim_core::event::SimTime;

use crate::QpuAnchor;

/// Discrete event the calibration schedule emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CalibrationEvent {
    /// QPU enters a calibration outage (`outage_ns` long); new
    /// admissions are rejected until the matching `BoundaryReset`.
    OutageBegin,
    /// End of the outage; OU drift returns abruptly to the
    /// anchor's nominal fidelity values.
    BoundaryReset,
}

/// Decision returned by [`crate::QpuAgent::admit_circuit`] when
/// the QPU refuses an admission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdmissionRejected {
    /// The QPU is currently inside a calibration outage. The
    /// caller can resubmit once `t_ns >= until`.
    InOutage {
        /// Time of the next [`CalibrationEvent::BoundaryReset`]
        /// (start of the next operational window).
        until: SimTime,
    },
}

/// Per-QPU calibration schedule. Cheap to copy.
///
/// `outage_ns == 0` and `period_ns == 0` are both rejected — the
/// "no calibration ever" case is modelled by simply not
/// attaching a schedule to a [`crate::QpuAgent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CalibrationSchedule {
    period_ns: SimTime,
    outage_ns: SimTime,
}

impl CalibrationSchedule {
    /// Build a schedule with an explicit `(period, outage)`
    /// pair.
    ///
    /// # Panics
    /// Panics if `period_ns == 0`, `outage_ns == 0`, or
    /// `outage_ns >= period_ns` — those configurations are
    /// pathological (an outage longer than the cycle would
    /// leave no operational window).
    pub fn new(period_ns: SimTime, outage_ns: SimTime) -> Self {
        assert!(period_ns > 0, "calibration period_ns must be > 0");
        assert!(outage_ns > 0, "calibration outage_ns must be > 0");
        assert!(
            outage_ns < period_ns,
            "outage_ns ({outage_ns}) must be < period_ns ({period_ns})",
        );
        Self {
            period_ns,
            outage_ns,
        }
    }

    /// Derive the schedule from a [`QpuAnchor`]'s
    /// `calibration_period_ns` / `calibration_outage_ns` fields.
    pub fn from_anchor(anchor: &QpuAnchor) -> Self {
        Self::new(anchor.calibration_period_ns, anchor.calibration_outage_ns)
    }

    /// Calibration cycle length.
    pub fn period_ns(&self) -> SimTime {
        self.period_ns
    }

    /// Length of each outage window.
    pub fn outage_ns(&self) -> SimTime {
        self.outage_ns
    }

    /// Whether simulator time `t_ns` falls inside an outage
    /// window.
    pub fn is_in_outage(&self, t_ns: SimTime) -> bool {
        let in_cycle = t_ns % self.period_ns;
        in_cycle >= self.period_ns - self.outage_ns
    }

    /// Time of the most recent boundary reset at or before
    /// `t_ns`. At `t_ns < period_ns` this returns `0` — the
    /// initial calibration that brought the QPU online.
    pub fn most_recent_reset_at(&self, t_ns: SimTime) -> SimTime {
        (t_ns / self.period_ns) * self.period_ns
    }

    /// Time of the next boundary reset strictly after `t_ns`.
    pub fn next_reset_after(&self, t_ns: SimTime) -> SimTime {
        (t_ns / self.period_ns + 1) * self.period_ns
    }

    /// Time of the next outage-begin event strictly after
    /// `t_ns`. The outage in cycle `k` begins at `k · period +
    /// (period − outage)`.
    pub fn next_outage_begin_after(&self, t_ns: SimTime) -> SimTime {
        let cycle = t_ns / self.period_ns;
        let begin_in_cycle = cycle * self.period_ns + (self.period_ns - self.outage_ns);
        if begin_in_cycle > t_ns {
            begin_in_cycle
        } else {
            begin_in_cycle + self.period_ns
        }
    }

    /// Next calibration event strictly after `t_ns` (whichever
    /// of `OutageBegin` / `BoundaryReset` comes first). Driver
    /// hook for the Phase-6 DES integration.
    pub fn next_event_after(&self, t_ns: SimTime) -> (SimTime, CalibrationEvent) {
        let next_begin = self.next_outage_begin_after(t_ns);
        let next_reset = self.next_reset_after(t_ns);
        if next_begin <= next_reset {
            (next_begin, CalibrationEvent::OutageBegin)
        } else {
            (next_reset, CalibrationEvent::BoundaryReset)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Modality;

    fn anchor(period_ns: SimTime, outage_ns: SimTime) -> QpuAnchor {
        QpuAnchor {
            modality: Modality::Superconducting,
            qubits: 50,
            t1_ns: 80_000,
            t2_ns: 60_000,
            fidelity_1q: 0.999,
            fidelity_2q: 0.993,
            fidelity_readout: 0.985,
            gate_time_1q_ns: 30,
            gate_time_2q_ns: 200,
            readout_time_ns: 1_000,
            calibration_period_ns: period_ns,
            calibration_outage_ns: outage_ns,
        }
    }

    #[test]
    fn from_anchor_inherits_period_and_outage() {
        let a = anchor(100, 10);
        let s = CalibrationSchedule::from_anchor(&a);
        assert_eq!(s.period_ns(), 100);
        assert_eq!(s.outage_ns(), 10);
    }

    #[test]
    #[should_panic(expected = "outage_ns")]
    fn outage_longer_than_period_panics() {
        CalibrationSchedule::new(50, 100);
    }

    #[test]
    #[should_panic(expected = "period_ns must be > 0")]
    fn zero_period_panics() {
        CalibrationSchedule::new(0, 10);
    }

    #[test]
    fn is_in_outage_matches_cycle_arithmetic() {
        // Period 100, outage 10. Cycle 0: ops [0, 90), outage
        // [90, 100). Cycle 1: ops [100, 190), outage [190, 200).
        let s = CalibrationSchedule::new(100, 10);
        assert!(!s.is_in_outage(0));
        assert!(!s.is_in_outage(50));
        assert!(!s.is_in_outage(89));
        assert!(s.is_in_outage(90));
        assert!(s.is_in_outage(95));
        assert!(s.is_in_outage(99));
        assert!(!s.is_in_outage(100), "boundary reset reopens the window");
        assert!(!s.is_in_outage(150));
        assert!(s.is_in_outage(190));
    }

    #[test]
    fn most_recent_reset_floors_to_period_multiple() {
        let s = CalibrationSchedule::new(100, 10);
        assert_eq!(s.most_recent_reset_at(0), 0);
        assert_eq!(s.most_recent_reset_at(50), 0);
        assert_eq!(s.most_recent_reset_at(99), 0);
        assert_eq!(s.most_recent_reset_at(100), 100);
        assert_eq!(s.most_recent_reset_at(150), 100);
        assert_eq!(s.most_recent_reset_at(199), 100);
        assert_eq!(s.most_recent_reset_at(200), 200);
    }

    #[test]
    fn next_reset_after_strictly_jumps_past_t() {
        let s = CalibrationSchedule::new(100, 10);
        assert_eq!(s.next_reset_after(0), 100);
        assert_eq!(s.next_reset_after(99), 100);
        assert_eq!(s.next_reset_after(100), 200);
        assert_eq!(s.next_reset_after(150), 200);
    }

    #[test]
    fn next_outage_begin_matches_cycle_layout() {
        let s = CalibrationSchedule::new(100, 10);
        assert_eq!(s.next_outage_begin_after(0), 90);
        assert_eq!(s.next_outage_begin_after(50), 90);
        assert_eq!(s.next_outage_begin_after(89), 90);
        assert_eq!(s.next_outage_begin_after(90), 190);
        assert_eq!(s.next_outage_begin_after(100), 190);
    }

    #[test]
    fn next_event_after_interleaves_outage_begin_and_reset() {
        let s = CalibrationSchedule::new(100, 10);
        // From t=0: next is 90 (OutageBegin).
        assert_eq!(s.next_event_after(0), (90, CalibrationEvent::OutageBegin));
        // From t=90: next is 100 (BoundaryReset).
        assert_eq!(
            s.next_event_after(90),
            (100, CalibrationEvent::BoundaryReset),
        );
        // From t=100: next is 190 (OutageBegin).
        assert_eq!(
            s.next_event_after(100),
            (190, CalibrationEvent::OutageBegin),
        );
        // From t=190: next is 200 (BoundaryReset).
        assert_eq!(
            s.next_event_after(190),
            (200, CalibrationEvent::BoundaryReset),
        );
    }
}
