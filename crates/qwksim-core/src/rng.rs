//! Hierarchical, deterministic RNG generation for `qwksim`.
//!
//! Implements the two-level derivation chain from
//! `plan/solution_plan.md` §3.3:
//!
//! ```text
//! master_seed ─(stable hash with replicate_index)─► replicate_seed
//! replicate_seed ─(stable hash with stream_id)────► stream_seed
//! ```
//!
//! Every random draw in the simulator is owned by exactly one
//! [`StreamId`] variant. Hashing is **SHA-256 domain-separated by
//! versioned ASCII labels** so the derivation is bit-stable across
//! rebuilds on the same platform (Q6′ = R2 single-machine
//! determinism). The first byte of the label set carries a `:v1`
//! tag so a future scheme bump can land without silently invalidating
//! checked-in `Provenance` blocks.
//!
//! All consumer streams use [`ChaCha20Rng`], which is reseedable from
//! a 32-byte seed and produces identical output for identical seed
//! across architectures (within the precision guarantee of the
//! `rand_chacha` crate's stability promise).

use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use sha2::{Digest, Sha256};

/// Domain-separation label for the master → replicate derivation.
const LABEL_REPLICATE: &[u8] = b"qwksim:replicate:v1";

/// Domain-separation label for the replicate → stream derivation.
const LABEL_STREAM: &[u8] = b"qwksim:stream:v1";

/// Identifier for one of the deterministic random streams used by
/// the simulator.
///
/// The set of variants is closed: every new source of randomness
/// must land here so its seed is reproducible from
/// `(master_seed, replicate_index, StreamId)`. See
/// `plan/solution_plan.md` §3.3 for the registry and the question
/// each variant traces back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamId {
    /// Workflow arrival driver (Q11.1).
    Arrival { scenario_id: u32 },
    /// MMPP modulating chain (Q11.1 = A2).
    MmppState { scenario_id: u32 },
    /// Per-workflow category selection (Q11.2 mix proportions).
    CategoryAssignment { workflow_id: u64 },
    /// Per-workflow attribute draws (`att1`..`att4`, Q11.4).
    WorkflowAttributes { workflow_id: u64, attribute: u8 },
    /// Per-task lognormal duration draws (Q8 / Q11.3).
    TaskDuration { workflow_id: u64, task_index: u32 },
    /// OU calibration-drift integrator state per `(site, qpu,
    /// fidelity-channel)` (Q9.2 = d3).
    CalibrationDrift {
        site_id: u16,
        qpu_id: u16,
        channel: u8,
    },
    /// Noisy-objective draws driving cv2 convergence (Q10.5).
    ConvergenceObjective { workflow_id: u64, iter_idx: u32 },
    /// Cloud-QPU hand-over latency (Q10.1 cloud branch, sw5).
    CloudControlLatency { qpu_id: u32 },
    /// Inter-site lognormal link latency (Q4 / Q3.2).
    LinkLatency { src_site: u16, dst_site: u16 },
    /// Per-channel additive noise on advertised summaries (Q4).
    AdvertisementNoise { site_id: u16, channel: u8 },
    /// Tie-breaker for the bargaining-round best-response solver
    /// (Q2 / Q3).
    BargainingTieBreaker { round_id: u64 },
}

impl StreamId {
    /// Write a canonical byte representation of the variant into
    /// `hasher`. The representation is stable across rebuilds; never
    /// reorder fields, never change variant tags.
    fn write_canonical(&self, hasher: &mut Sha256) {
        match self {
            StreamId::Arrival { scenario_id } => {
                hasher.update(b"Arrival");
                hasher.update(scenario_id.to_le_bytes());
            }
            StreamId::MmppState { scenario_id } => {
                hasher.update(b"MmppState");
                hasher.update(scenario_id.to_le_bytes());
            }
            StreamId::CategoryAssignment { workflow_id } => {
                hasher.update(b"CategoryAssignment");
                hasher.update(workflow_id.to_le_bytes());
            }
            StreamId::WorkflowAttributes {
                workflow_id,
                attribute,
            } => {
                hasher.update(b"WorkflowAttributes");
                hasher.update(workflow_id.to_le_bytes());
                hasher.update(attribute.to_le_bytes());
            }
            StreamId::TaskDuration {
                workflow_id,
                task_index,
            } => {
                hasher.update(b"TaskDuration");
                hasher.update(workflow_id.to_le_bytes());
                hasher.update(task_index.to_le_bytes());
            }
            StreamId::CalibrationDrift {
                site_id,
                qpu_id,
                channel,
            } => {
                hasher.update(b"CalibrationDrift");
                hasher.update(site_id.to_le_bytes());
                hasher.update(qpu_id.to_le_bytes());
                hasher.update(channel.to_le_bytes());
            }
            StreamId::ConvergenceObjective {
                workflow_id,
                iter_idx,
            } => {
                hasher.update(b"ConvergenceObjective");
                hasher.update(workflow_id.to_le_bytes());
                hasher.update(iter_idx.to_le_bytes());
            }
            StreamId::CloudControlLatency { qpu_id } => {
                hasher.update(b"CloudControlLatency");
                hasher.update(qpu_id.to_le_bytes());
            }
            StreamId::LinkLatency { src_site, dst_site } => {
                hasher.update(b"LinkLatency");
                hasher.update(src_site.to_le_bytes());
                hasher.update(dst_site.to_le_bytes());
            }
            StreamId::AdvertisementNoise { site_id, channel } => {
                hasher.update(b"AdvertisementNoise");
                hasher.update(site_id.to_le_bytes());
                hasher.update(channel.to_le_bytes());
            }
            StreamId::BargainingTieBreaker { round_id } => {
                hasher.update(b"BargainingTieBreaker");
                hasher.update(round_id.to_le_bytes());
            }
        }
    }
}

/// Top-level RNG hierarchy. Cheap to copy; carries only the master
/// seed.
#[derive(Debug, Clone, Copy)]
pub struct RngHierarchy {
    master_seed: u64,
}

impl RngHierarchy {
    /// Build a hierarchy rooted at `master_seed`. All downstream
    /// streams are deterministically derived from this seed plus a
    /// `(replicate_index, StreamId)` pair.
    pub fn new(master_seed: u64) -> Self {
        Self { master_seed }
    }

    /// Derive the seed for replicate `replicate_index`.
    pub fn replicate(&self, replicate_index: u64) -> ReplicateRng {
        let mut hasher = Sha256::new();
        hasher.update(LABEL_REPLICATE);
        hasher.update(self.master_seed.to_le_bytes());
        hasher.update(replicate_index.to_le_bytes());
        let replicate_seed: [u8; 32] = hasher.finalize().into();
        ReplicateRng { replicate_seed }
    }
}

/// Replicate-scoped RNG factory. Construct one with
/// [`RngHierarchy::replicate`], then call [`ReplicateRng::stream`]
/// once per stream the replicate needs to draw from.
#[derive(Debug, Clone, Copy)]
pub struct ReplicateRng {
    replicate_seed: [u8; 32],
}

impl ReplicateRng {
    /// Return the raw 32-byte replicate seed (useful for
    /// provenance and the `qwksim replay` CLI).
    pub fn replicate_seed(&self) -> [u8; 32] {
        self.replicate_seed
    }

    /// Build the [`ChaCha20Rng`] for stream `id` within this
    /// replicate. Two calls with the same `id` return RNGs in
    /// identical state.
    pub fn stream(&self, id: StreamId) -> ChaCha20Rng {
        let mut hasher = Sha256::new();
        hasher.update(LABEL_STREAM);
        hasher.update(self.replicate_seed);
        id.write_canonical(&mut hasher);
        let stream_seed: [u8; 32] = hasher.finalize().into();
        ChaCha20Rng::from_seed(stream_seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::RngCore;

    /// Pull the first 16 bytes from `rng` as two little-endian u64s.
    fn first_16_bytes(rng: &mut ChaCha20Rng) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..8].copy_from_slice(&rng.next_u64().to_le_bytes());
        out[8..16].copy_from_slice(&rng.next_u64().to_le_bytes());
        out
    }

    #[test]
    fn identical_inputs_yield_identical_first_16_bytes() {
        let h = RngHierarchy::new(0xdead_beef_cafe_f00d);
        let id = StreamId::Arrival { scenario_id: 7 };

        let mut a = h.replicate(42).stream(id);
        let mut b = h.replicate(42).stream(id);

        assert_eq!(first_16_bytes(&mut a), first_16_bytes(&mut b));
    }

    #[test]
    fn distinct_stream_ids_decorrelate_within_a_replicate() {
        let h = RngHierarchy::new(0x1234_5678_9abc_def0);
        let replicate = h.replicate(0);

        let mut a = replicate.stream(StreamId::Arrival { scenario_id: 0 });
        let mut b = replicate.stream(StreamId::Arrival { scenario_id: 1 });
        let mut c = replicate.stream(StreamId::MmppState { scenario_id: 0 });

        let a_first = first_16_bytes(&mut a);
        let b_first = first_16_bytes(&mut b);
        let c_first = first_16_bytes(&mut c);

        assert_ne!(a_first, b_first, "scenario_id change must decorrelate");
        assert_ne!(a_first, c_first, "variant change must decorrelate");
        assert_ne!(b_first, c_first);
    }

    #[test]
    fn distinct_replicates_decorrelate_for_same_stream() {
        let h = RngHierarchy::new(0xfeed_face);
        let id = StreamId::TaskDuration {
            workflow_id: 100,
            task_index: 0,
        };

        let mut a = h.replicate(0).stream(id);
        let mut b = h.replicate(1).stream(id);

        assert_ne!(first_16_bytes(&mut a), first_16_bytes(&mut b));
    }

    #[test]
    fn distinct_master_seeds_decorrelate_everything() {
        let id = StreamId::CalibrationDrift {
            site_id: 1,
            qpu_id: 0,
            channel: 0,
        };

        let mut a = RngHierarchy::new(1).replicate(0).stream(id);
        let mut b = RngHierarchy::new(2).replicate(0).stream(id);

        assert_ne!(first_16_bytes(&mut a), first_16_bytes(&mut b));
    }
}
