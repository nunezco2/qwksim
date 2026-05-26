# FLAG-N — aspirational vs. fallback experiment grid

**Status:** Closed (committed to **aspirational**) on 2026-05-22 at
the M1 / T1.14 milestone.

## Decision

Use the **aspirational 16 000-run Cartesian grid** for the headline
experiment:

```
4 load × 4 τ_adv × 5 mechanisms × 2 info regimes × 100 replicates
= 16 000 runs
```

The 3 × 3 fallback (9 000 runs) is rejected today. It remains
documented in `plan/manifests/headline.toml` as a comment so a
future regression can revive it without re-deriving the trade-off.

## Why now

§14 of `plan/solution_plan.md` opened FLAG-N at questionnaire-close
and committed to closing it at the M1 milestone — i.e. once we had
measured events-per-second on a representative scenario. M1 closes
with T1.13 (b5 end-to-end), so this is the moment.

## Measurements (M1)

All numbers from the author's workstation (M-series macOS, release
build); see `bench/history.csv` for the raw rows.

| bench | scenario | median time | throughput |
| --- | --- | --- | --- |
| `b1::event_queue::push_then_pop/1m` | 1 M synthetic events bulk insert / drain | 65.6 ms | 15.25 Melem/s |
| `b1::event_queue::steady_state/1m_mix` | 10 k prime + 1 M pop/push cycles | 70.6 ms | 14.16 Melem/s |
| `b5::end_to_end/smoke_1000_arrivals` | T1.8 smoke, 1 000 arrivals, 1-core partition, ρ = 0.5 | **0.50 ms** | **2.01 Melem/s** |

## Budget arithmetic

Per Q6.4 = r, the headline target is **≤ 60 s per single-replicate
run**. The 16 000-run grid on 16 cores at 60 s/run = **16.7 CPU
hours**, comfortably within the Q6.5 = (m) + (n) single-workstation
overnight envelope.

Today's b5 measurement gives **0.50 ms / run** for the
representative 1 000-arrival scenario. That is roughly
**120 000× under budget**. Phases 2–4 will add weight to each
replicate — bargaining solver inner loop, QPU calibration drift,
advertisement protocol, mid-circuit feedback, etc. — but the
headroom is large:

- A **1 000×** slowdown from current performance leaves each run
  at ~500 ms; the 16 000-run grid lands at ~8 minutes on 16 cores.
- A **10 000×** worst-case slowdown still lands at ~1.4 hours
  total, comfortably under the 16.7-CPU-hour ceiling.

Committing to the aspirational grid is therefore the low-risk
choice today.

## Re-opening conditions

If, at any point during Phase 2–4, the b5 benchmark regresses
below **~1 second per replicate** for the representative
1 000-arrival scenario, this decision must be revisited:

1. Append a row to `bench/history.csv` with the regression
   numbers and the commit short SHA.
2. Open a follow-up PR titled
   `docs(experiment): reassess FLAG-N at <commit>` that either
   - Records the new headroom and re-confirms the 16 000-run
     grid, or
   - Switches `plan/manifests/headline.toml` to the 3 × 3
     fallback (9 000 runs) and amends this document with the
     rationale.

The escape ladder from §14.6 of `plan/solution_plan.md`
(`esc4 → esc2 → esc3 → esc1`) remains the authoritative fallback
ordering when the trim-the-sweep route alone proves insufficient.

## Cross-references

- Solution plan: §14 (perf budget), §14.6 (escape ladder), the
  FLAG-N entry in the flag-closure ledger.
- `bench/history.csv`: the rows tagged `T1.12 baseline` and
  `T1.13 baseline`.
- Issue history: T1.12 (#24) and T1.13 (#25) introduced the
  measurements that close this flag.
