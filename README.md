# qwksim

Agent-based discrete-event simulator of HPC–QPU integration for distributed
game-theoretic scheduling research.

This repository contains the in-progress Rust implementation. The research
specification and the engineering plan live under `plan/`.

- [What is this](#what-is-this)
- [Quickstart](#quickstart)
- [Layout](#layout)
- [Responsible disclosure](#responsible-disclosure)
- [License](#license)

## What is this

`qwksim` simulates a federation of HPC sites with attached QPUs (superconducting,
trapped-ion, etc.) and the workflows that consume their combined resources.
The goal is to evaluate whether distributed, game-theoretic schedulers —
specifically, per-resource cooperative-bargaining agents under bounded-round
best-response dynamics — can match or outperform centralised schedulers on a
four-objective Pareto frontier (makespan, p95 quantum-touching wait time,
service utilisation, QPU utilisation under deadline-miss SLA).

The research question, system model, baselines, and experimental design are
documented in [`plan/problem_definition.md`](plan/problem_definition.md).
The engineering plan — workspace layout, milestones, PR-sized task list —
is in [`plan/solution_plan.md`](plan/solution_plan.md).

## Quickstart

Requires a recent stable Rust toolchain (install via
[rustup](https://rustup.rs/)).

```sh
git clone https://github.com/nunezco2/qwksim.git
cd qwksim
cargo check --workspace
```

The workspace is currently a stub; member crates are added in subsequent
phases per the plan. Once present, the standard verbs apply:

```sh
cargo build --workspace
cargo test --workspace
cargo bench --no-run
```

## Layout

```
.
├── Cargo.toml      # workspace manifest (resolver = "2")
├── LICENSE         # Apache-2.0
├── README.md       # this file
└── plan/           # research specification and engineering plan
```

Member crates (`qwksim-core`, `qwksim-resources`, `qwksim-qpu`,
`qwksim-workflow`, `qwksim-bargain`, `qwksim-scheduler`,
`qwksim-baselines`, `qwksim-experiment`, `qwksim-analysis`, `qwksim-cli`)
will appear under the workspace root as their owning phases land.

## Responsible disclosure

> *"If you discover scheduling-attack patterns or capacity-inference
> techniques enabled by this simulator that could be misused against
> real federated HPC–QPU operators, please contact `<MAINTAINER_EMAIL>`
> privately before public disclosure. We commit to coordinated disclosure
> with affected operators within 90 days."*

`<MAINTAINER_EMAIL>` to be set by the maintainer before the first public release.

## License

Licensed under the Apache License, Version 2.0. See [`LICENSE`](LICENSE).
