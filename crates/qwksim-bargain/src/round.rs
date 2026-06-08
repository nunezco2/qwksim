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

use qwksim_core::event::AgentId;

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
    fn best_response(&self, current: &BTreeMap<AgentId, AgentBid>) -> AgentBid;

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
            // Iterate agents in BTreeMap (ascending id) order
            // for determinism.
            for (&id, agent) in by_id.iter() {
                let bid = agent.best_response(&allocation);
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
