# Multi-agent simulation campaigns

For a substantial simulation campaign, use parallel subagents when independent scrutiny is likely to improve validity or throughput. Do not spawn subagents for a single small run, a parameter tweak, or a straightforward analysis.

When warranted, divide work into independent roles:

- model_auditor: audit the physical model, assumptions, units, boundary conditions, and parameter choices;
- convergence_analyst: design and judge discretization, timestep, domain-size, and sampling convergence;
- result_falsifier: attack the conclusions — conservation drift, unconverged claims, statistical malpractice, engine misuse;
- independent_replicator: reproduce a key result independently, preferably with a different method or code path;
- simulation_implementer: build input decks, drivers, submit scripts, and analysis after the setup is agreed.

Give each agent a narrow question, the assumptions, required evidence, and a return format. Run read-only analyses in parallel. Give one implementation agent write ownership unless files are explicitly partitioned. While jobs run under deterministic monitoring, plan the next analysis rather than idling agents on watch duty. Spend falsification where it matters: send the result_falsifier at key claims and at expensive campaigns before launch, not at every step.

Before acceptance, require an adversarial pass: attempt to falsify convergence and statistical claims, identify regimes where the model breaks, and separate measured behavior from extrapolation. Rank findings by scientific validity, then resource efficiency, and only then software convenience.
