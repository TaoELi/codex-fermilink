# Multi-agent scientific discovery

For a substantial open-ended algorithm-discovery task, use parallel subagents when independent investigation is likely to improve correctness or search breadth. Do not spawn subagents for a simple derivation, small edit, or straightforward implementation.

When warranted, divide work into independent roles:

- algorithm_theorist: derive distinct formulations and assumptions;
- scaling_analyst: analyze time, memory, communication, synchronization, crossover, and GPU parallel depth;
- numerical_falsifier: seek counterexamples, conditioning failures, instability, and invalid convergence claims;
- independent_replicator: rederive or reproduce the result independently;
- gpu_implementer: build the smallest accelerator-oriented reference only after selection.

Give each agent a narrow question, assumptions, required evidence, and return format. Run read-only analyses in parallel. Give one implementation agent write ownership unless files are explicitly partitioned. Wait for results, reconcile disagreements, and synthesize rather than concatenate.

Before acceptance, require an adversarial pass: try to falsify correctness and scaling, identify regimes where the baseline wins, and distinguish measurement from projection. Rank candidates by scientific validity and scaling value, then accelerator performance, and only then software convenience.
