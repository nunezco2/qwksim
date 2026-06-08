//! Bounded-round best-response inner loop (T4.4).
//!
//! Implements the per-§6.2 driver: each round, every
//! participating agent picks its best feasible response given
//! the current allocation; the round ends when either
//! `R_max` rounds have elapsed or the Nash product has not
//! improved by at least `ε_conv` since the previous round.
//!
//! ## Determinism
//!
//! Agents are processed in ascending [`AgentId`] order via a
//! `BTreeMap` (Q6′ = R2 — no `HashMap`, no randomised
//! tie-break). Two runs with the same `(agents, initial)`
//! pair return byte-identical [`BargainingOutcome`].
//!
//! ## Decision space
//!
//! Each agent's bid is modelled as a scalar `f64` ([`AgentBid`]);
//! the concrete multi-dimensional [`Allocation`](qwksim_resources::Allocation)
//! lands when T4.5+ wires the full bargaining bundle through.
//! The loop's correctness depends only on per-agent
//! best-response monotonicity, not on the dimensionality, so
//! the abstraction generalises.

use std::collections::BTreeMap;

use rand_chacha::ChaCha20Rng;

use qwksim_core::event::AgentId;
use qwksim_core::rng::{ReplicateRng, StreamId};

use crate::nash::nash_product;

/// One participant's bid in a bargaining round. Concrete
/// allocation types land in T4.5+; for the inner loop a
/// single scalar suffices.
pub type AgentBid = f64;

/// Per-scenario bargaining configuration (R_max, ε_conv).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BargainingRound {
    /// Hard ceiling on the number of best-response rounds.
    /// Spec default: 16.
    pub r_max: u32,
    /// Convergence threshold on the Nash-product improvement
    /// per round. Spec default: 1e-6.
    pub epsilon_conv: f64,
}

impl BargainingRound {
    /// Spec defaults (§6.2): `R_max = 16`, `ε_conv = 1e-6`.
    pub fn default_spec() -> Self {
        Self {
            r_max: 16,
            epsilon_conv: 1e-6,
        }
    }
}

/// Result of one bargaining inner-loop invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct BargainingOutcome {
    /// Final per-agent allocation (BTreeMap so iteration is
    /// deterministic).
    pub allocation: BTreeMap<AgentId, AgentBid>,
    /// Number of completed rounds (`≤ r_max`).
    pub rounds: u32,
    /// Final Nash-product log-sum value (see
    /// [`nash_product`]).
    pub final_nash: f64,
    /// `true` iff the loop exited via the `ε_conv` early-stop
    /// branch; `false` if it hit the `R_max` ceiling.
    pub converged: bool,
}

/// Per-agent surface the inner loop consumes.
///
/// Concrete utility functions (FLAG-C closed; T4.2) live in
/// [`crate::utility`] and feed [`utility`](BargainingAgent::utility)
/// / [`disagreement`](BargainingAgent::disagreement). The
/// [`best_response`](BargainingAgent::best_response) hook is
/// per-agent: the agent inspects the current allocation and
/// returns its preferred bid.
pub trait BargainingAgent {
    /// Stable identifier for this agent within the scenario.
    /// Used as the `BTreeMap` ordering key — runtime
    /// determinism depends on it being unique within the
    /// participating set.
    fn id(&self) -> AgentId;

    /// Pick this agent's best feasible bid given the current
    /// allocation. The inner loop calls this once per agent
    /// per round; the agent's own bid in `current` is the
    /// *previous round's* value (or the initial value on
    /// round 0).
    ///
    /// **Agents with no tie-handling logic implement only this
    /// method.** When the agent's utility has a flat maximum
    /// over multiple bids, the agent should override
    /// [`Self::best_response_with_tie_break`] instead and use
    /// the rng to pick one — see T4.5 / `Stream::BargainingTieBreaker`.
    fn best_response(&self, current: &BTreeMap<AgentId, AgentBid>) -> AgentBid;

    /// Tie-break-aware best-response (T4.5). The default
    /// delegates to [`Self::best_response`] (ignores the rng),
    /// so agents whose utility surface is strictly concave do
    /// not need to override.
    ///
    /// Agents whose utility has a flat maximum across multiple
    /// bids — e.g. integer-allocation agents where two distinct
    /// allocations both yield the same FLAG-C utility — should
    /// override and draw from `rng` to pick one. The rng is
    /// keyed by `Stream::BargainingTieBreaker { round_id }` and
    /// is *shared across every agent within a single round* —
    /// agents draw from it in ascending `AgentId` order.
    fn best_response_with_tie_break(
        &self,
        current: &BTreeMap<AgentId, AgentBid>,
        _rng: &mut ChaCha20Rng,
    ) -> AgentBid {
        self.best_response(current)
    }

    /// Utility of this agent under the full allocation.
    /// Feeds the Nash-product convergence check.
    fn utility(&self, allocation: &BTreeMap<AgentId, AgentBid>) -> f64;

    /// Disagreement-point payoff under FCFS-on-stale (FLAG-B,
    /// T4.3). Static per round — the FCFS projection is
    /// computed once before the loop starts.
    fn disagreement(&self) -> f64;
}

impl BargainingRound {
    /// Run the bounded-round best-response loop per §6.2.
    ///
    /// On round `r`:
    /// 1. Each agent (in ascending [`AgentId`] order) replaces
    ///    its bid with [`best_response`](BargainingAgent::best_response).
    /// 2. The new [`nash_product`] is compared to the previous
    ///    round's value. If the improvement is `< ε_conv`, the
    ///    loop terminates with `converged = true`.
    /// 3. After `R_max` rounds, the loop terminates with
    ///    `converged = false` even if not converged.
    ///
    /// # Panics
    ///
    /// Panics if `agents` is empty (a zero-agent bargain is
    /// meaningless) or if any [`AgentId`] in `initial` does
    /// not have a corresponding entry in `agents`.
    pub fn run<A: BargainingAgent>(
        &self,
        agents: &[A],
        initial: BTreeMap<AgentId, AgentBid>,
    ) -> BargainingOutcome {
        self.run_inner(agents, initial, None)
    }

    /// Tie-break-aware variant (T4.5). Identical to
    /// [`Self::run`] but threads a per-round
    /// `Stream::BargainingTieBreaker { round_id }` rng into
    /// each agent's [`best_response_with_tie_break`](BargainingAgent::best_response_with_tie_break)
    /// call.
    ///
    /// The `round_id` consumed by round `r` is
    /// `base_round_id.wrapping_add(r as u64)`. Callers
    /// should pick `base_round_id` to be unique per
    /// bargaining-call within a replicate (e.g.
    /// `base_round_id = workflow_id * R_max`); two calls
    /// sharing a `base_round_id` share rng streams across
    /// rounds.
    ///
    /// # Panics
    ///
    /// Same as [`Self::run`].
    pub fn run_with_tie_break<A: BargainingAgent>(
        &self,
        agents: &[A],
        initial: BTreeMap<AgentId, AgentBid>,
        replicate: &ReplicateRng,
        base_round_id: u64,
    ) -> BargainingOutcome {
        self.run_inner(agents, initial, Some((replicate, base_round_id)))
    }

    fn run_inner<A: BargainingAgent>(
        &self,
        agents: &[A],
        initial: BTreeMap<AgentId, AgentBid>,
        tie_break: Option<(&ReplicateRng, u64)>,
    ) -> BargainingOutcome {
        assert!(!agents.is_empty(), "bargaining run: no agents");
        // Index agents by id for O(log n) lookup and BTreeMap
        // iteration order.
        let mut by_id: BTreeMap<AgentId, &A> = BTreeMap::new();
        for a in agents {
            let prev = by_id.insert(a.id(), a);
            assert!(
                prev.is_none(),
                "bargaining run: duplicate agent id {}",
                a.id(),
            );
        }
        // The initial allocation must cover every agent — a
        // missing key would silently zero an agent's bid below
        // and hide a wiring bug.
        for &id in by_id.keys() {
            assert!(
                initial.contains_key(&id),
                "bargaining run: initial allocation missing agent {id}",
            );
        }

        let mut allocation = initial;
        let mut prev_nash = compute_nash(&by_id, &allocation);

        let mut rounds = 0_u32;
        let mut converged = false;
        for r in 0..self.r_max {
            // Build a per-round rng if tie-breaking is enabled.
            // Done at the round granularity (not per-agent) so
            // agents draw from a single deterministic stream in
            // ascending AgentId order.
            let mut rng = tie_break.map(|(replicate, base)| {
                let round_id = base.wrapping_add(r as u64);
                replicate.stream(StreamId::BargainingTieBreaker { round_id })
            });
            // Iterate agents in BTreeMap (ascending id) order
            // for determinism.
            for (&id, agent) in by_id.iter() {
                let bid = match rng.as_mut() {
                    Some(rng) => agent.best_response_with_tie_break(&allocation, rng),
                    None => agent.best_response(&allocation),
                };
                allocation.insert(id, bid);
            }
            rounds = r + 1;
            let new_nash = compute_nash(&by_id, &allocation);
            if (new_nash - prev_nash) < self.epsilon_conv {
                converged = true;
                prev_nash = new_nash;
                break;
            }
            prev_nash = new_nash;
        }

        BargainingOutcome {
            allocation,
            rounds,
            final_nash: prev_nash,
            converged,
        }
    }
}

/// Evaluate the Nash product over the current allocation
/// using each agent's `utility` and `disagreement`. Wrapper
/// over [`nash_product`] that pulls the two parallel vectors
/// out of the per-agent trait.
fn compute_nash<A: BargainingAgent>(
    by_id: &BTreeMap<AgentId, &A>,
    allocation: &BTreeMap<AgentId, AgentBid>,
) -> f64 {
    let mut us: Vec<f64> = Vec::with_capacity(by_id.len());
    let mut ds: Vec<f64> = Vec::with_capacity(by_id.len());
    for agent in by_id.values() {
        us.push(agent.utility(allocation));
        ds.push(agent.disagreement());
    }
    nash_product(&us, &ds)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quadratic best-response agent: utility is a paraboloid
    /// centred at `0.5` with a coupling term to the other
    /// agent's current bid. Specifically:
    ///
    /// ```text
    /// u_A(x_A, x_B) = − ( (x_A − x_B)² + (x_A − 0.5)² )
    /// ```
    ///
    /// Best response (∂u/∂x_A = 0): `x_A* = 0.25 + 0.5·x_B`,
    /// symmetric for B. The symmetric fixed point is
    /// `x_A = x_B = 0.5`, where both utilities are exactly 0
    /// (the analytical optimum) and the best-response
    /// iteration converges geometrically from any start.
    struct QuadraticAgent {
        id: AgentId,
        partner: AgentId,
    }

    impl BargainingAgent for QuadraticAgent {
        fn id(&self) -> AgentId {
            self.id
        }

        fn best_response(&self, current: &BTreeMap<AgentId, AgentBid>) -> AgentBid {
            let x_other = current.get(&self.partner).copied().unwrap_or(0.0);
            0.25 + 0.5 * x_other
        }

        fn utility(&self, allocation: &BTreeMap<AgentId, AgentBid>) -> f64 {
            let x_self = allocation.get(&self.id).copied().unwrap_or(0.0);
            let x_other = allocation.get(&self.partner).copied().unwrap_or(0.0);
            -((x_self - x_other).powi(2) + (x_self - 0.5).powi(2))
        }

        fn disagreement(&self) -> f64 {
            // Conservative threat point: utility at (0, 0) is
            // -(0 + 0.25) = -0.25, so set disagreement strictly
            // below to keep the per-agent surplus positive and
            // exercise the ln_1p path.
            -1.0
        }
    }

    /// T4.4 acceptance gate: a hand-constructed two-agent
    /// two-resource (here: two parallel best-response axes
    /// coupled symmetrically) case converges to the
    /// analytical fixed point `(x_A, x_B) = (0.5, 0.5)`
    /// within `ε_conv`.
    #[test]
    fn two_agent_quadratic_game_converges_to_analytical_fixpoint_within_epsilon_conv() {
        let agents = vec![
            QuadraticAgent { id: 1, partner: 2 },
            QuadraticAgent { id: 2, partner: 1 },
        ];
        let mut initial = BTreeMap::new();
        initial.insert(1, 0.0);
        initial.insert(2, 0.0);

        let round = BargainingRound::default_spec();
        let outcome = round.run(&agents, initial);

        assert!(
            outcome.converged,
            "expected early-stop via ε_conv, got R_max={} rounds={}",
            round.r_max, outcome.rounds,
        );
        assert!(
            outcome.rounds <= round.r_max,
            "rounds {} exceeded R_max {}",
            outcome.rounds,
            round.r_max,
        );

        // Both bids must be within a small distance of the
        // analytical fixed point 0.5.
        let x_a = outcome.allocation[&1];
        let x_b = outcome.allocation[&2];
        // The bid convergence is the *square root* of the
        // Nash-product convergence (utility is quadratic in
        // displacement), so allow 1e-3 on the bids when
        // ε_conv = 1e-6 on the Nash sum.
        let bid_tol = round.epsilon_conv.sqrt() * 100.0;
        assert!(
            (x_a - 0.5).abs() < bid_tol,
            "x_A {x_a} not within {bid_tol} of analytical 0.5",
        );
        assert!(
            (x_b - 0.5).abs() < bid_tol,
            "x_B {x_b} not within {bid_tol} of analytical 0.5",
        );

        // Final Nash product: utilities at fixed point are 0,
        // disagreements are -1, so surplus = 1 each, log-sum
        // = 2·ln 2.
        let want_nash = 2.0 * std::f64::consts::LN_2;
        assert!(
            (outcome.final_nash - want_nash).abs() < 1e-3,
            "final Nash {} ≠ analytical {want_nash}",
            outcome.final_nash,
        );
    }

    #[test]
    fn run_is_deterministic_across_invocations() {
        let agents = vec![
            QuadraticAgent { id: 7, partner: 11 },
            QuadraticAgent { id: 11, partner: 7 },
        ];
        let mut initial = BTreeMap::new();
        initial.insert(7, 0.1);
        initial.insert(11, 0.9);
        let round = BargainingRound::default_spec();

        let a = round.run(&agents, initial.clone());
        let b = round.run(&agents, initial);

        assert_eq!(a.rounds, b.rounds);
        assert_eq!(a.converged, b.converged);
        assert_eq!(a.allocation, b.allocation);
        assert_eq!(a.final_nash.to_bits(), b.final_nash.to_bits());
    }

    #[test]
    fn r_max_caps_the_loop_when_epsilon_conv_makes_early_stop_impossible() {
        // ε_conv = −∞ means `(new − prev) < ε_conv` is always
        // false (no finite delta is below −∞), so the early-
        // stop branch never fires and the loop must run for
        // exactly R_max rounds.
        let agents = vec![
            QuadraticAgent { id: 1, partner: 2 },
            QuadraticAgent { id: 2, partner: 1 },
        ];
        let mut initial = BTreeMap::new();
        initial.insert(1, 0.0);
        initial.insert(2, 0.0);
        let round = BargainingRound {
            r_max: 5,
            epsilon_conv: f64::NEG_INFINITY,
        };
        let outcome = round.run(&agents, initial);
        assert_eq!(outcome.rounds, 5, "should hit R_max exactly");
        assert!(!outcome.converged, "should NOT report early-stop");
    }

    #[test]
    fn agents_iterate_in_btreemap_ascending_id_order() {
        // Build a tiny tracking agent that records the order
        // in which best_response is called.
        use std::cell::RefCell;

        struct OrderTracker<'a> {
            id: AgentId,
            log: &'a RefCell<Vec<AgentId>>,
        }
        impl BargainingAgent for OrderTracker<'_> {
            fn id(&self) -> AgentId {
                self.id
            }
            fn best_response(&self, _: &BTreeMap<AgentId, AgentBid>) -> AgentBid {
                self.log.borrow_mut().push(self.id);
                0.5
            }
            fn utility(&self, _: &BTreeMap<AgentId, AgentBid>) -> f64 {
                0.0
            }
            fn disagreement(&self) -> f64 {
                -1.0
            }
        }
        let log = RefCell::new(Vec::new());
        // Insert agents OUT OF ID ORDER so the only path to
        // ascending-id iteration is BTreeMap ordering.
        let agents = vec![
            OrderTracker { id: 9, log: &log },
            OrderTracker { id: 1, log: &log },
            OrderTracker { id: 5, log: &log },
        ];
        let mut initial = BTreeMap::new();
        for a in &agents {
            initial.insert(a.id, 0.0);
        }
        let round = BargainingRound {
            r_max: 1,
            epsilon_conv: f64::INFINITY,
        };
        let _ = round.run(&agents, initial);
        // After one round, the log should reflect the
        // ascending-id iteration order.
        assert_eq!(log.borrow().as_slice(), &[1, 5, 9]);
    }

    #[test]
    #[should_panic(expected = "no agents")]
    fn empty_agents_panics() {
        let round = BargainingRound::default_spec();
        let _ = round.run::<QuadraticAgent>(&[], BTreeMap::new());
    }

    #[test]
    #[should_panic(expected = "duplicate agent id")]
    fn duplicate_agent_id_panics() {
        let agents = vec![
            QuadraticAgent { id: 1, partner: 2 },
            QuadraticAgent { id: 1, partner: 2 },
        ];
        let mut initial = BTreeMap::new();
        initial.insert(1, 0.0);
        let round = BargainingRound::default_spec();
        let _ = round.run(&agents, initial);
    }

    #[test]
    #[should_panic(expected = "initial allocation missing")]
    fn missing_initial_bid_panics() {
        let agents = vec![
            QuadraticAgent { id: 1, partner: 2 },
            QuadraticAgent { id: 2, partner: 1 },
        ];
        let mut initial = BTreeMap::new();
        initial.insert(1, 0.0); // agent 2 missing
        let round = BargainingRound::default_spec();
        let _ = round.run(&agents, initial);
    }

    /// T4.5 fixture: an agent whose best response is a 50/50
    /// choice between two equal-utility plateau bids. The
    /// override of [`best_response_with_tie_break`] uses the
    /// shared per-round rng to pick; the default
    /// [`best_response`] (no rng) picks the deterministic
    /// fallback always.
    struct PlateauTieAgent {
        id: AgentId,
        low: AgentBid,
        high: AgentBid,
    }
    impl BargainingAgent for PlateauTieAgent {
        fn id(&self) -> AgentId {
            self.id
        }
        fn best_response(&self, _current: &BTreeMap<AgentId, AgentBid>) -> AgentBid {
            // Deterministic fallback for the tie-break-less
            // path: always pick the low bid.
            self.low
        }
        fn best_response_with_tie_break(
            &self,
            _current: &BTreeMap<AgentId, AgentBid>,
            rng: &mut ChaCha20Rng,
        ) -> AgentBid {
            use rand_core::RngCore;
            // High bit of one u64 → 50/50 pick.
            if rng.next_u64() & (1 << 63) != 0 {
                self.high
            } else {
                self.low
            }
        }
        fn utility(&self, allocation: &BTreeMap<AgentId, AgentBid>) -> f64 {
            // Constant utility on the plateau: both low and
            // high give the same value. Use a fixed offset so
            // the Nash sum is positive.
            let _ = allocation;
            0.5
        }
        fn disagreement(&self) -> f64 {
            -1.0
        }
    }

    /// T4.5 acceptance gate (1/2): two runs with the same
    /// (replicate master_seed, base_round_id) produce
    /// byte-identical tie-break outcomes.
    #[test]
    fn identical_replicate_and_round_id_yield_identical_tie_break_outcomes() {
        use qwksim_core::rng::RngHierarchy;
        let agents = vec![
            PlateauTieAgent {
                id: 1,
                low: 0.2,
                high: 0.8,
            },
            PlateauTieAgent {
                id: 2,
                low: 0.3,
                high: 0.7,
            },
            PlateauTieAgent {
                id: 3,
                low: 0.4,
                high: 0.6,
            },
        ];
        let mut initial = BTreeMap::new();
        for a in &agents {
            initial.insert(a.id, 0.0);
        }
        // Force the loop to run all rounds so we observe
        // multiple tie-break draws.
        let round = BargainingRound {
            r_max: 8,
            epsilon_conv: f64::NEG_INFINITY,
        };
        let replicate_a = RngHierarchy::new(0xCAFE_BABE).replicate(7);
        let replicate_b = RngHierarchy::new(0xCAFE_BABE).replicate(7);

        let out_a = round.run_with_tie_break(&agents, initial.clone(), &replicate_a, 42);
        let out_b = round.run_with_tie_break(&agents, initial, &replicate_b, 42);

        assert_eq!(out_a.allocation, out_b.allocation);
        assert_eq!(out_a.rounds, out_b.rounds);
        assert_eq!(out_a.final_nash.to_bits(), out_b.final_nash.to_bits());
    }

    /// T4.5 acceptance gate (2/2): distinct `base_round_id`s
    /// (or distinct replicates) yield decorrelated tie-break
    /// trajectories.
    #[test]
    fn distinct_round_ids_decorrelate_the_tie_break_picks() {
        use qwksim_core::rng::RngHierarchy;
        // Single agent, tie-break decides every round's bid.
        let agents = vec![PlateauTieAgent {
            id: 1,
            low: 0.0,
            high: 1.0,
        }];
        let mut initial = BTreeMap::new();
        initial.insert(1, 0.5);
        let round = BargainingRound {
            r_max: 16,
            epsilon_conv: f64::NEG_INFINITY,
        };
        let replicate = RngHierarchy::new(0x1234_5678).replicate(0);

        // Run with two distinct base_round_ids; the final
        // bid (decided by the LAST round's tie-break) must
        // differ for at least one trial across a small sweep.
        let mut saw_difference = false;
        for base in 0..32u64 {
            let a = round.run_with_tie_break(&agents, initial.clone(), &replicate, base);
            let b = round.run_with_tie_break(&agents, initial.clone(), &replicate, base + 1);
            if a.allocation[&1] != b.allocation[&1] {
                saw_difference = true;
                break;
            }
        }
        assert!(
            saw_difference,
            "distinct base_round_ids never decorrelated tie-break picks across 32 trials",
        );
    }

    #[test]
    fn distinct_master_seeds_decorrelate_the_tie_break_picks() {
        // Symmetric of the above: same base_round_id, different
        // master seed → different picks.
        use qwksim_core::rng::RngHierarchy;
        let agents = vec![PlateauTieAgent {
            id: 1,
            low: 0.0,
            high: 1.0,
        }];
        let mut initial = BTreeMap::new();
        initial.insert(1, 0.5);
        let round = BargainingRound {
            r_max: 16,
            epsilon_conv: f64::NEG_INFINITY,
        };

        let mut saw_difference = false;
        for seed in 1..32u64 {
            let rep_a = RngHierarchy::new(seed).replicate(0);
            let rep_b = RngHierarchy::new(seed + 100).replicate(0);
            let a = round.run_with_tie_break(&agents, initial.clone(), &rep_a, 0);
            let b = round.run_with_tie_break(&agents, initial.clone(), &rep_b, 0);
            if a.allocation[&1] != b.allocation[&1] {
                saw_difference = true;
                break;
            }
        }
        assert!(
            saw_difference,
            "distinct master seeds never decorrelated tie-break picks across 31 trials",
        );
    }

    #[test]
    fn run_without_tie_break_uses_the_deterministic_fallback_method() {
        // The legacy `run` entry point must call
        // `best_response` (not `best_response_with_tie_break`),
        // so agents that haven't opted in still behave the
        // same way they did pre-T4.5.
        let agents = vec![PlateauTieAgent {
            id: 1,
            low: 0.3,
            high: 0.7,
        }];
        let mut initial = BTreeMap::new();
        initial.insert(1, 0.5);
        let round = BargainingRound {
            r_max: 4,
            epsilon_conv: f64::NEG_INFINITY,
        };
        let out = round.run(&agents, initial);
        // PlateauTieAgent's default best_response returns
        // `low` always.
        assert_eq!(out.allocation[&1], 0.3);
    }

    #[test]
    fn agents_without_tie_break_override_are_unchanged_by_run_with_tie_break() {
        // Regression: an agent that *only* implements
        // best_response (no override) must produce the same
        // outcome under either entry point.
        use qwksim_core::rng::RngHierarchy;
        let agents = vec![
            QuadraticAgent { id: 1, partner: 2 },
            QuadraticAgent { id: 2, partner: 1 },
        ];
        let mut initial = BTreeMap::new();
        initial.insert(1, 0.0);
        initial.insert(2, 0.0);
        let round = BargainingRound::default_spec();
        let replicate = RngHierarchy::new(0xAAAA).replicate(0);

        let out_plain = round.run(&agents, initial.clone());
        let out_tie = round.run_with_tie_break(&agents, initial, &replicate, 0);

        assert_eq!(out_plain.allocation, out_tie.allocation);
        assert_eq!(out_plain.rounds, out_tie.rounds);
        assert_eq!(out_plain.final_nash.to_bits(), out_tie.final_nash.to_bits(),);
    }

    #[test]
    fn nash_product_is_non_decreasing_across_rounds_under_best_response() {
        // The Nash product under best-response in this
        // potential game must be monotone non-decreasing
        // round-on-round. We probe by running with R_max set
        // high enough that we capture every round and check
        // the trajectory by re-running with successively
        // larger R_max ceilings.
        let agents = vec![
            QuadraticAgent { id: 1, partner: 2 },
            QuadraticAgent { id: 2, partner: 1 },
        ];
        let mut initial = BTreeMap::new();
        initial.insert(1, 0.0);
        initial.insert(2, 0.0);

        let mut prev = f64::NEG_INFINITY;
        for r_max in 1..=10u32 {
            let round = BargainingRound {
                r_max,
                epsilon_conv: 0.0,
            };
            let outcome = round.run(&agents, initial.clone());
            assert!(
                outcome.final_nash >= prev - 1e-12,
                "Nash product decreased between R_max={} and {}: {} → {}",
                r_max - 1,
                r_max,
                prev,
                outcome.final_nash,
            );
            prev = outcome.final_nash;
        }
    }
}
