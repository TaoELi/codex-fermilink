# codex-agent-profiles (fermilink fork)

Built-in agent profiles. A profile selects the replacement base instructions
used for newly created threads, independently of the selected model and
reasoning effort, and may bundle subagent roles, Ultra-only multi-agent
orchestration guidance, and capabilities such as deterministic long-running
job monitoring.

## Installing the fork's binaries

The fork builds the standard `codex` binary; `codex-fermilink` is installed
as a companion symlink so both names launch the same executable. `codex`
resolves helper executables as siblings of the running binary (symlinks are
canonicalized first), so the install directory must also hold
`codex-code-mode-host`:

```bash
cargo build --release --bin codex
install_dir=~/bin
cp target/release/codex "$install_dir"/codex
ln -sf codex "$install_dir"/codex-fermilink
# codex-code-mode-host embeds V8; the cargo build may fail when no prebuilt
# rusty_v8 archive exists for the pinned version/feature set (HTTP 404 from
# the v8 build script). Either build it via Bazel like the official release,
# or copy the host from an official install of the SAME release line:
cp "$(dirname "$(readlink -f "$(command -v codex)")")/codex-code-mode-host" "$install_dir"/
```

Build this fork from the release tag matching that host (`rust-v0.151.0`):
the code-mode IPC protocol requires the CLI and host to run the same version.
On a machine that also has the official codex on `PATH`, whichever
directory comes first wins for the bare `codex` name; `codex-fermilink`
always launches the fork.

## Profiles

| Profile | Behavior |
| --- | --- |
| `default` | Shipped Codex instructions, byte-for-byte upstream behavior. |
| `scientific-brainstorms` | Research-direction panel prompt: the main agent referees four to eight subagents modeled on real scientists' public work through blind proposal, critique, and ranking rounds, then writes `strategy.md`; four panel roles; no job monitoring. |
| `scientific-algorithm` | Hypothesis-first algorithm research prompt; five research subagent roles; job monitoring for long benchmarks. |
| `scientific-simulations` | HPC simulation setup, convergence, checkpointing, and validation prompt; five simulation roles; job monitoring. |
| `scientific-measurements` | Calibration, uncertainty, provenance, and experimental-design prompt; five measurement roles; job monitoring. |

Each profile lives under `profiles/<id>/` as `prompt.md`, `multi_agent.md`,
and `agents/*.toml`, all compiled into the binary. Prompts are compact
(≈8–10 KB vs ≈20 KB shipped), written for entry-graduate-student research
code, and include a minimal progress-update protocol. The long-jobs sections
carry only policy (never poll, true-PID capture, budget translation,
bookkeeping wake turns); the `jobs.*` parameter semantics live in the tool
descriptions, which the model sees on every request — keep the two from
drifting back into duplication.

All three scientific prompts share one memory convention: long work lives
under a dated `projects/YYYY-MM-DD-short-name/` directory whose `memory.md`
is the canonical state. The file starts with the original request verbatim
and a `last_updated` timestamp, keeps a current-state-and-next-actions
section that is rewritten in place (what a wake-up or resumed session acts
from), and appends a dated step log below it, grouping machine-generated
file families by pattern instead of listing each. The prompts mandate
re-reading it after any resume or history compaction, which is what makes
compact-before-wake (below) safe.

## Selecting a profile

- In the TUI, run `/profile` and pick an entry. The selection is persisted as
  `agent_profile` in the user `config.toml` and a fresh thread is started,
  because base instructions are thread-scoped. (`agent_mode` is accepted as a
  legacy alias of the config key.)
- In config: `agent_profile = "scientific-simulations"`.
- One-off: `codex-fermilink -c agent_profile='"scientific-algorithm"'` (also
  works with `codex-fermilink exec`).

## Semantics

- `default` supplies no custom base instructions, so the models manager
  renders the model's own instruction template exactly as upstream does.
- A non-default profile resolves during config loading into
  `Config::base_instructions` with `BaseInstructionsProvenance::Custom`, the
  same mechanism used by `model_instructions_file`, taking precedence over
  the legacy `model_instructions_file` and inline `instructions` settings;
  programmatic overrides (`ConfigOverrides::base_instructions`) still win.
- A non-default profile forces the standard Responses transport: config
  loading sets `ModelsManagerConfig::force_standard_responses`, and
  `models-manager/src/model_info.rs::with_config_overrides` then clears
  `use_responses_lite`, so the replacement prompt is sent as the top-level
  `instructions` field instead of being demoted to a developer message on
  the Responses Lite path. The forcing is scoped to profiles on purpose:
  internal flows such as Guardian review intentionally pair their own custom
  instructions with Responses Lite, and the legacy `model_instructions_file`
  keeps its upstream transport behavior.
- Custom provenance already survives model switches and thread forks
  upstream; resumed threads keep the instructions persisted in their rollout.
  When a thread with explicitly custom instructions is resumed under a config
  that supplies none, the inherited text is fed into the per-turn model-info
  overrides (`core/src/session/mod.rs`) so replacement semantics survive the
  resume; if the text matches a built-in profile prompt exactly, the standard
  Responses transport is kept as well.
- `agent_profile` (and its `agent_mode` alias) is on the project-local config
  denylist (`config/src/loader/mod.rs`): a repository's `.codex/config.toml`
  cannot silently switch the agent's root profile.
- The `/status` card shows an `Agent profile` row and the session header
  shows a `profile:` row whenever the configured profile is not `default`.
  These read the active config, so a thread resumed under a different global
  profile keeps its original instructions (per the rollout) even though no
  profile row is shown.

## Bundled subagent roles

A profile can ship subagent roles (`AgentProfile::roles`); they extend the
built-in `default`/`explorer`/`worker` roles offered to the spawn tool, but
only while that profile is selected, so the default profile keeps upstream
behavior exactly. User-defined roles in `$CODEX_HOME/agents/` win on name
collisions.

| Profile | Roles (model / effort) |
| --- | --- |
| scientific-brainstorms | panelist (sol/high), prior_art_scout (luna/xhigh), devils_advocate (sol/xhigh), panel_rapporteur (terra/high) |
| scientific-algorithm | algorithm_theorist (sol/max), scaling_analyst (sol/xhigh), numerical_falsifier (sol/max), independent_replicator (luna/xhigh), gpu_implementer (terra/high) |
| scientific-simulations | model_auditor (sol/xhigh), convergence_analyst (sol/xhigh), result_falsifier (sol/max), independent_replicator (luna/xhigh), simulation_implementer (terra/high) |
| scientific-measurements | experimental_designer (sol/xhigh), calibration_auditor (sol/xhigh), uncertainty_analyst (sol/max), independent_replicator (luna/xhigh), acquisition_implementer (terra/high) |

Replicators deliberately stay on a different model family (luna): their
scientific value is decorrelated failure modes, not raw capability.
Implementers stay on the coding workhorse at high effort because they
translate an already-agreed design; they are the only write-owning roles
(and, for measurements, the only role allowed to drive hardware). The
reasoning-heavy analysis roles run on the top model.

Wiring lives in `core/src/agent/role.rs`: `built_in::profile_configs`
exposes the roles for resolution and for the spawn-tool description, and
`built_in::config_file_contents` resolves each role's virtual `config_path`
to the embedded ConfigToml overlay (`model`, `model_reasoning_effort`,
`developer_instructions`). Note this tree's role files cannot enforce sandbox
levels, so read-only expectations are stated in the roles' developer
instructions rather than enforced; spawned children inherit the profile's
base instructions because custom provenance survives role application.

## Ultra-only orchestration guidance

Each scientific profile's multi-agent orchestration text
(`profiles/<id>/multi_agent.md`, `AgentProfile::multi_agent_guidance`) is
deliberately NOT part of the base instructions. Base instructions are
thread-scoped and persisted while reasoning effort is a per-turn setting, so
an effort-dependent base prompt cannot follow mid-thread effort changes
without rewriting history. Instead the guidance is prepended to the
spawn-agent tool description (`spec_plan.rs::agent_type_description`) only
when the turn's reasoning effort is `ultra`. Lower efforts therefore see the
delegation-free prompt and a plain role list, while Ultra turns see the full
orchestration protocol; the spawn tool's role selector is exposed whenever
the profile bundles roles (`spawn_tool_spec::profile_has_roles`), so explicit
delegation requests still work at any effort.

## Deterministic job monitoring (`ProfileCapability::JobMonitor`)

All three scientific profiles (algorithm, simulations, measurements) enable
three tools backed by the `codex-job-monitor` crate (semantics ported from
the FermiLink harness):

- `jobs.job_attach` — register already-submitted work: a SLURM job
  (`slurm:<id>`), a SLURM job array by its parent ID (per-task states are
  aggregated — one failed task fails the array, counts like `7×RUNNING,
  3×PENDING` ride along), a detached process (`pid:<pid>`), or a whole sweep
  of independent jobs in one call (`jobs` list). Optional per-attach
  settings: `log_paths`, `expected_runtime` (`90s`/`45m`/`6h`; a job running
  ≥2× this is reported once as a possible hang), `watch_patterns` (extra
  log regexes — domain anomalies become wake events), and `wake_policy`
  (`each`, or `batch` — the sweep default — where failures wake immediately
  but plain completions wait for the whole batch). The agent submits through
  the normal shell, so sandbox and approval machinery apply to the
  submission; a wrapper's PID is not the job.
- `jobs.job_await` — park the turn while deterministic code polls `sacct`
  (falling back to `squeue --states=all`, which still lists jobs that
  finished within `MinJobAge`) or process liveness with adaptive backoff
  (15 s → 5 min, reset to 15 s after every state change), tailing logs for
  suspicious lines (NaN, divergence, OOM, crashes, plus the job's watch
  patterns). Returns on a terminal state (batch jobs: first failure or the
  whole sweep done), a suspicious log event, an expected-runtime overrun,
  repeated UNKNOWN scheduler answers (controller unreachable or timing out),
  new user input or an incoming agent message, or a per-call wait budget
  (default 30 min, max 12 h) — the agent burns no turns while waiting and
  simply calls `job_await` again after a budget return. Where accounting is
  disabled (`sacct` fails, as on a single-node workstation) a finished job
  vanishes from the scheduler after `MinJobAge`: a vanished job the record
  saw running is reported as terminal `EXITED` (the log must tell success
  from failure, as for a dead PID); one never seen active is `NOT_FOUND`
  (wrong ID, or ended and aged out before attach), so nothing waits on it.
- `jobs.job_status` — instant snapshot of state history, runtime vs.
  expectation, and bounded log tails; the reattachment point after a
  session restart.

Records persist under `$CODEX_HOME/jobs/<thread-id>/`, so a multi-day SLURM
job survives closed laptops and resumed sessions.

While `job_await` is parked, the turn shows a sleep-style waiting item sized
to the wait budget, and job state transitions (for example
PENDING → RUNNING) surface as live notices.

### Session job watcher (`jobs.*` config)

If a turn ends while attached jobs are still active, a session-resident
watcher (enabled by the JobMonitor capability) keeps polling with the same
deterministic engine and wakes the idle agent by injecting a bounded
`[job monitor]` message — which starts a new turn with full context — when a
job reaches a terminal state (batch jobs: first failure, or the whole sweep
terminal), new suspicious or watched log lines appear, or a job overruns its
expected runtime. Each outcome wakes at most once (`notified_at`,
`suspicious_signature`, and `overrun_notified` on the job record; an attach,
status, or await reply that already showed the outcome also counts), interrupted
sessions are never auto-resumed, and `jobs.max_auto_continues` bounds
runaway loops. A session that starts with tracked jobs (a resume) opens with
a one-line `[jobs] N tracked job(s)…` briefing. Because long jobs outlive
the provider prompt cache, a wake on a large history (≥50k tokens in the
last request) may first compact the conversation — the wake turn then starts
from the summary plus the durable `memory.md`, at a fraction of the cost;
every compaction failure mode degrades to waking on the full history.
Whether it does is the profile's choice (`compact_before_wake` on the
profile definition) unless `jobs.compact_before_wake` is set: on for the
simulation and measurement profiles, whose results need fresh analysis after
hours of waiting; off for the algorithm profile, whose benchmark wakes come
every few minutes and need the raw development history — what was tried, why
it failed, the current code — more than the saving.
Configuration:

```toml
[jobs]
auto_continue = true        # default; set false to disable the watcher
check_in_seconds = 21600    # optional periodic "still running" wake-ups
max_auto_continues = 50     # default cap on automatic wake-ups per session
watch_patterns = ["not converged"]  # optional user-level log regexes
compact_before_wake = true  # optional; overrides the profile default (see above)
```

## Adding a profile

Add `profiles/<id>/{prompt.md, multi_agent.md, agents/*.toml}`, the
`include_str!` consts, and an entry in `BUILT_IN_AGENT_PROFILES` in
`src/lib.rs`. The `/profile` picker, config validation, role wiring,
transport forcing, and status surfaces all enumerate the catalog, and the
catalog-driven tests cover new profiles automatically.

## Maintenance across upstream releases

The fork's identity is "one official release tag plus the fermilink
feature". Rules that keep updates cheap:

1. **Always pin to official release tags, never to `main`.** The code-mode
   host speaks a same-version IPC protocol (`deny_unknown_fields` in both
   directions; e.g. `code_mode_host_duration_ns` was added right after
   0.151.0 and broke a mixed pairing), and only tagged releases have a
   distributable matching `codex-code-mode-host`.
2. **Update procedure** (measured cost when hopping ~550 commits: one
   one-line conflict):

   ```bash
   git fetch https://github.com/openai/codex.git tag rust-vNEW --no-tags
   git checkout -b fermilink-NEW rust-vNEW
   git cherry-pick <fermilink feature commits>
   git checkout rust-vNEW -- codex-rs/Cargo.lock   # cargo re-adds fork crates
   just write-config-schema
   ```

   Then run the regression gate, rebuild, reinstall the binaries, and
   re-copy `codex-code-mode-host` from the NEW official release. Keep the
   previous branch until the new one is validated; tag each validated
   rebase (`fermilink-vX.Y.Z`) for reproducibility.
3. **Hook inventory.** Every upstream touchpoint carries a
   `Fermilink fork:` comment; `git grep -n "Fermilink fork"` lists the
   complete integration surface (config resolution, project-config
   denylist, transport forcing, role wiring, spawn-tool gating, session
   resume injection, TUI surfaces). After a rebase, each hook either
   applied cleanly or is the conflict to re-site. Fork-owned files
   (this crate, `job-monitor`, `profile_popups.rs`, the `agent_profile`
   test suite) never conflict.
4. **Regression gate** after every rebase:

   ```bash
   just test -p codex-job-monitor -p codex-agent-profiles -p codex-config
   RUST_MIN_STACK=16777216 just test -p codex-core -p codex-models-manager \
       agent_profile role_ job_watcher \
       config_loader responses_lite model_switching force_standard
   just test -p codex-tui
   ```

   The upstream tests in that set are load-bearing for the fork:
   `responses_lite` and guardian tests guard the transport-forcing
   boundary; `model_switching` guards custom-provenance survival.
   `RUST_MIN_STACK` matters on some machines: the debug-build integration
   futures sit near the default 8 MB thread stack and SIGABRT without it.
5. **Prompt evaluation suite.** Prompts regress under model and upstream
   changes faster than code does. `fermilink-evals/` (repo root) holds
   canned scientific tasks with deterministic graders and an A/B runner;
   run it against the previous and the new binary before shipping prompt
   or job-tool changes (see its README).
5. **Drift to watch per release:** restructuring of `role.rs` or
   `spec_plan.rs` (the densest hooks); upstream changing custom-instruction
   transport semantics (which could shrink or retire
   `force_standard_responses`); new model generations in `models.json`
   (revisit the role model/effort pins); and eventually dropping the
   `agent_mode` config alias. Prompts and role files are fork-owned and
   never need merging.
6. **Cadence:** rebase when a release carries something wanted, not on
   every release; a rebase is roughly an hour including build and gate.
   The long-term exit is upstreaming the profile mechanism itself — the
   design reuses upstream concepts (custom provenance, built-in roles,
   config keys) to keep that door open.
