//! Per-QPU compilation cache (T3.5).
//!
//! Stores the *transpiled* form of a parametrised circuit so
//! that subsequent iterations of the same variational workflow
//! (VQE, QAOA, QML) skip the transpilation cost when only the
//! parameter *values* change — the cache key captures the
//! template id and the parameter-vector *structure*, not its
//! values, so every iteration of a 50-step VQE workflow hits
//! the same entry.
//!
//! Per Q9.5 = (c3) the cache key is the triple
//! `(template_id, params_hash, qpu_id)`. `params_hash` is the
//! structural hash supplied by the caller (typically a hash of
//! the parameter-vector length and target modality); this lets
//! two workflows sharing the same template share the same
//! compiled output even when their per-iteration parameter
//! draws differ.
//!
//! ## Invalidation
//!
//! Every calibration boundary (T3.4) sweep-invalidates the
//! cache via [`CompilationCache::invalidate_all`]. The QpuAgent
//! integration in `crate::agent` calls this automatically on
//! every cycle reset so the cache surface stays implicit for
//! callers.
//!
//! ## Determinism (Q6′ = R2)
//!
//! Backed by a `BTreeMap`, not a `HashMap` — `compile_or_cache`
//! is byte-deterministic on its input sequence.

use std::collections::BTreeMap;

use qwksim_core::event::{AgentId, SimTime};

/// Cache lookup key.
///
/// `template_id` and `params_hash` are workflow-supplied; the
/// `qpu_id` is the [`crate::QpuAgent`]'s own
/// [`AgentId`](qwksim_core::event::AgentId), so two agents on
/// different super-sites do not share a compiled artefact even
/// when their templates and parameter shapes match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompilationCacheKey {
    /// Template id (matches
    /// [`qwksim_workflow::task::QuantumDescriptor::template_id`]).
    pub template_id: u32,
    /// Structural hash of the parameter vector (length, target
    /// modality, layout — *not* the values).
    pub params_hash: u64,
    /// QPU the cache lives on.
    pub qpu_id: AgentId,
}

/// Marker for the compiled circuit. Today this only records
/// what the transpiler would have cost and when it was
/// produced; the actual lowered representation lands when the
/// Phase-4 bargaining loop wires through the full circuit body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompiledCircuit {
    /// Simulated wall-clock cost the transpiler paid for this
    /// entry (returned as the `0` cost on subsequent hits).
    pub compile_cost_ns: SimTime,
    /// Simulator time the entry was inserted.
    pub compiled_at_ns: SimTime,
}

/// Result of one [`CompilationCache::compile_or_cache`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompileOutcome {
    /// Cache miss: the caller's transpile cost was charged in
    /// full and the entry was inserted.
    Miss {
        /// The transpile cost the caller paid (the same value
        /// passed into [`CompilationCache::compile_or_cache`]).
        compile_cost_ns: SimTime,
    },
    /// Cache hit: zero transpile cost charged.
    Hit,
}

impl CompileOutcome {
    /// `true` iff this outcome was a [`CompileOutcome::Hit`].
    pub fn is_hit(self) -> bool {
        matches!(self, Self::Hit)
    }

    /// The transpile cost charged to the caller (zero on hit,
    /// `compile_cost_ns` on miss).
    pub fn cost_charged(self) -> SimTime {
        match self {
            Self::Hit => 0,
            Self::Miss { compile_cost_ns } => compile_cost_ns,
        }
    }
}

/// Per-QPU compilation cache.
///
/// Owned by [`crate::QpuAgent`]. Always present (an empty cache
/// is harmless overhead) so downstream code never branches on
/// "cache attached or not".
#[derive(Debug, Clone, Default)]
pub struct CompilationCache {
    entries: BTreeMap<CompilationCacheKey, CompiledCircuit>,
    hits: u64,
    misses: u64,
    invalidations: u64,
}

impl CompilationCache {
    /// Build an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lookup `key`. On hit returns [`CompileOutcome::Hit`]; on
    /// miss inserts a new entry costing `compile_cost_ns` and
    /// returns [`CompileOutcome::Miss`]. The hit / miss counters
    /// are bumped accordingly.
    pub fn compile_or_cache(
        &mut self,
        key: CompilationCacheKey,
        compile_cost_ns: SimTime,
        now: SimTime,
    ) -> CompileOutcome {
        use std::collections::btree_map::Entry;
        match self.entries.entry(key) {
            Entry::Occupied(_) => {
                self.hits += 1;
                CompileOutcome::Hit
            }
            Entry::Vacant(slot) => {
                slot.insert(CompiledCircuit {
                    compile_cost_ns,
                    compiled_at_ns: now,
                });
                self.misses += 1;
                CompileOutcome::Miss { compile_cost_ns }
            }
        }
    }

    /// Peek at an entry without bumping counters. `None` if not
    /// cached.
    pub fn peek(&self, key: &CompilationCacheKey) -> Option<&CompiledCircuit> {
        self.entries.get(key)
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` iff the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of successful lookups since construction (counter
    /// is **not** reset by [`Self::invalidate_all`] — it tracks
    /// the entire history).
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Number of misses since construction.
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Number of [`Self::invalidate_all`] sweeps since
    /// construction.
    pub fn invalidations(&self) -> u64 {
        self.invalidations
    }

    /// Hit rate over the full history. Returns `0.0` when no
    /// lookups have happened yet.
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    /// Sweep-invalidate every entry. Called by the [`crate::QpuAgent`]
    /// on every calibration-boundary reset (T3.4) so a freshly
    /// recalibrated QPU does not reuse stale compiled artefacts.
    /// The hit / miss counters are preserved.
    pub fn invalidate_all(&mut self) {
        self.entries.clear();
        self.invalidations += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(template_id: u32, params_hash: u64, qpu_id: AgentId) -> CompilationCacheKey {
        CompilationCacheKey {
            template_id,
            params_hash,
            qpu_id,
        }
    }

    #[test]
    fn fresh_cache_is_empty_and_has_zero_hit_rate() {
        let c = CompilationCache::new();
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
        assert_eq!(c.hits(), 0);
        assert_eq!(c.misses(), 0);
        assert_eq!(c.invalidations(), 0);
        assert_eq!(c.hit_rate(), 0.0);
    }

    #[test]
    fn first_lookup_is_a_miss_and_second_is_a_hit() {
        let mut c = CompilationCache::new();
        let k = key(7, 0xdead_beef, 11);

        let r1 = c.compile_or_cache(k, 1_000_000, 100);
        assert_eq!(
            r1,
            CompileOutcome::Miss {
                compile_cost_ns: 1_000_000
            }
        );
        assert!(!r1.is_hit());
        assert_eq!(r1.cost_charged(), 1_000_000);
        assert_eq!(c.len(), 1);
        assert_eq!(c.hits(), 0);
        assert_eq!(c.misses(), 1);

        let r2 = c.compile_or_cache(k, 1_000_000, 200);
        assert_eq!(r2, CompileOutcome::Hit);
        assert!(r2.is_hit());
        assert_eq!(r2.cost_charged(), 0);
        assert_eq!(c.len(), 1, "hit must not duplicate the entry");
        assert_eq!(c.hits(), 1);
        assert_eq!(c.misses(), 1);
        assert_eq!(c.hit_rate(), 0.5);
    }

    #[test]
    fn different_keys_do_not_collide() {
        let mut c = CompilationCache::new();
        let kt = key(1, 0, 0); // template_id varies
        let kp = key(0, 1, 0); // params_hash varies
        let kq = key(0, 0, 1); // qpu_id varies
        for k in [kt, kp, kq] {
            assert!(!c.compile_or_cache(k, 1, 0).is_hit(), "first call → miss");
        }
        assert_eq!(c.len(), 3);
        assert_eq!(c.misses(), 3);
        // Repeat each: all hits.
        for k in [kt, kp, kq] {
            assert!(c.compile_or_cache(k, 1, 0).is_hit(), "second call → hit");
        }
        assert_eq!(c.hits(), 3);
        assert_eq!(c.misses(), 3);
        assert_eq!(c.hit_rate(), 0.5);
    }

    #[test]
    fn invalidation_clears_entries_but_preserves_counters() {
        let mut c = CompilationCache::new();
        let k = key(1, 1, 1);
        // Warm.
        c.compile_or_cache(k, 1, 0);
        c.compile_or_cache(k, 1, 0);
        c.compile_or_cache(k, 1, 0);
        assert_eq!(c.len(), 1);
        assert_eq!(c.hits(), 2);
        assert_eq!(c.misses(), 1);

        c.invalidate_all();
        assert!(c.is_empty());
        assert_eq!(c.invalidations(), 1);
        assert_eq!(c.hits(), 2, "history-wide counters preserved");
        assert_eq!(c.misses(), 1);

        // Next lookup misses because entries were cleared.
        let r = c.compile_or_cache(k, 1, 0);
        assert!(!r.is_hit());
        assert_eq!(c.misses(), 2);
        assert_eq!(c.invalidations(), 1);
    }

    #[test]
    fn peek_does_not_bump_counters_or_insert() {
        let mut c = CompilationCache::new();
        let k = key(1, 1, 1);

        assert!(c.peek(&k).is_none());
        assert_eq!(c.hits(), 0);
        assert_eq!(c.misses(), 0);

        c.compile_or_cache(k, 42, 10);
        let entry = c.peek(&k).expect("entry present");
        assert_eq!(entry.compile_cost_ns, 42);
        assert_eq!(entry.compiled_at_ns, 10);
        // peek didn't bump hits.
        assert_eq!(c.hits(), 0);
        assert_eq!(c.misses(), 1);
    }

    #[test]
    fn hit_rate_after_warmup_matches_unit_arithmetic() {
        // 1 miss + 9 hits = 90% hit rate.
        let mut c = CompilationCache::new();
        let k = key(1, 1, 1);
        for _ in 0..10 {
            c.compile_or_cache(k, 1, 0);
        }
        assert_eq!(c.misses(), 1);
        assert_eq!(c.hits(), 9);
        assert!((c.hit_rate() - 0.9).abs() < 1e-12);
    }
}
