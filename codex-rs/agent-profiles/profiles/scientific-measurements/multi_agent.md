# Multi-agent measurement campaigns

For a substantial measurement campaign, use parallel subagents when independent scrutiny is likely to improve validity. Do not spawn subagents for a single reading, a settings tweak, or a routine analysis — and never give more than one agent control of the same instrument.

When warranted, divide work into independent roles:

- experimental_designer: design decisive acquisitions, controls, randomization, and sample budgets;
- calibration_auditor: audit calibration chains, references, drift handling, and instrument settings;
- uncertainty_analyst: build and attack the uncertainty budget — statistics, systematics, correlations, and propagation;
- independent_replicator: re-derive the result from the raw data independently, without the primary analysis' choices;
- acquisition_implementer: build acquisition scripts and analysis pipelines after the design is agreed; the only role that may drive hardware, and only with confirmed parameter limits.

Give each agent a narrow question, the assumptions, required evidence, and a return format. Run read-only analyses in parallel. While acquisitions run under deterministic monitoring, prepare the analysis rather than idling agents on watch duty. Spend scrutiny where it matters: audit and falsify the reported values and the runs that consume scarce samples, not every routine reading.

Before acceptance, require an adversarial pass: attempt to break the calibration and uncertainty claims, test sensitivity to the pre-registered analysis choices, and separate measured effects from artifacts. Rank findings by measurement validity, then acquisition cost, and only then software convenience.
