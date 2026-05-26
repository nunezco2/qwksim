# FLAG-I — fluid vs. packet intra-site network model

**Status:** Closed (committed to **fluid, linear equal-share**) on
2026-05-22 at the T2.3 milestone.

## Decision

The intra-site network model is the **linear equal-share fluid**
sharing rule defined in §4.2 of `plan/solution_plan.md`:

```
share_i(t) = capacity / N(t)   for every active stream i in {1..N(t)}
```

where `N(t)` is the count of streams currently open against the
pool. Conservation `sum_i share_i = capacity` holds at every
instant by construction. A discrete packet-level model
(M/M/1-K queues, NS-3-style discrete-event packet simulation,
etc.) is **rejected**.

## Rationale

1. **Per-run wall-clock budget (Q6.4 = r, ≤ 60 s / run).** Packet
   simulation explodes the event count by 4–6 orders of
   magnitude — packet-level traffic on a 1 Gbps link at 1 KiB
   MTU is ~125 000 send/recv events per second of simulator
   time. The headline workload is bandwidth-light (small
   classical → quantum circuit specs; medium quantum → classical
   shot records) but a 1 000-workflow / 1-simulated-hour run
   would still incur ~10⁸ packet events. Today's per-run budget
   is ~500 µs (T1.13 b5 baseline); a packet model would push
   each replicate well past the (r) ceiling.

2. **Acceptable fidelity for the headline.** The four pillars of
   the headline Pareto front (makespan, p95 quantum-touching
   wait, service utilisation, QPU utilisation under SLA) are
   *coarse-grained* in milliseconds; fluid sharing reproduces
   their first moments to within the bargaining solver's
   measurement noise. Where finer detail is needed (e.g.
   bursty interactions with the τ_adv broadcast), a follow-up
   PR can layer a rate-shaped Markov chain on top of the
   fluid baseline without rewriting the simulator.

3. **Rivalry term composes cleanly.** The cooperative-bargaining
   utility (FLAG-C) wants a scalar
   `rivalry ∈ [0, 1]` to feed into its `γ · (1 − rivalry)`
   slot. The fluid model gives one for free:
   `rivalry = min(1, (n − 1) / n_cap)` where `n_cap` is a
   per-link soft saturation count calibrated separately. A
   packet model would have to derive a comparable scalar by
   integrating queue depth over a window — more work, no
   stronger guarantee.

4. **FLAG-J alignment.** FLAG-J committed to the
   shared-constraint rivalry pattern (no separate "rivalry
   agent"). Fluid pools naturally publish a single scalar
   `rivalry()` consumed by every per-resource agent's utility
   function. Packet simulation would split this responsibility
   across the packet sources and the link, complicating the
   pattern.

## Implementation

- `qwksim-resources::FluidBandwidthPool` (T2.3) owns
  `(capacity, n_cap, BTreeMap<StreamId, ActiveStream>)`.
- `admit(stream, now)` / `release(id, now)` mutate the
  `active_streams` map; iteration order is deterministic
  (`BTreeMap`) under Q6′ = R2 — the workspace
  `clippy::disallowed_methods` gate (T1.11) backstops the
  HashMap exclusion.
- `current_share(id)` returns `capacity / N` if `id` is active,
  `0` otherwise.
- `rivalry()` returns the §4.2 formula.
- `advertised_summary(now)` returns the snapshot the bargaining
  solver consumes in the `Local` view; today's snapshot is
  live, and the proptest in `network.rs::proptests` asserts
  `advertised free ≤ true free` over randomised op sequences.

## Re-opening conditions

Reassess the fluid model if any of the following hold:

- The headline Pareto front's interaction-with-network terms
  (especially p95 quantum-touching wait under bursty arrivals)
  start showing a systematic offset > 10 % against a
  packet-simulation oracle on a small calibration scenario.
- A reviewer requests a precise treatment of TCP slow-start or
  WAN tail latency — both are out of fluid's reach.

In either case, file a follow-up PR titled
`docs(experiment): reassess FLAG-I at <commit>` that either
re-confirms fluid (with the calibration data attached) or
commits to a discrete model. The escape ladder from §14.6 of
`plan/solution_plan.md` does **not** cover this case (the
ladder is about per-run wall-clock, not about model fidelity).

## Cross-references

- Solution plan: §2.2 (resource models), §4.2 (FluidBandwidthPool),
  the FLAG-I entry in the flag-closure ledger.
- T2.2 (#28) — `MemoryBandwidthPool`, the simpler same-shape
  ancestor.
- T2.3 (this PR) — `FluidBandwidthPool` + the proptest gate.
- FLAG-J (`plan/decisions/flag-j.md`, future PR) — shared-
  constraint rivalry pattern that consumes the scalar emitted
  here.
