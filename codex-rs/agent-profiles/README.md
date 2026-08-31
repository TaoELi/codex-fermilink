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
| `scientific-algorithm` | Hypothesis-first algorithm research prompt; five research subagent roles. |
| `scientific-simulations` | HPC simulation setup, convergence, checkpointing, and validation prompt; five simulation roles; job monitoring. |
| `scientific-measurements` | Calibration, uncertainty, provenance, and experimental-design prompt; five measurement roles; job monitoring. |

Each profile lives under `profiles/<id>/` as `prompt.md`, `multi_agent.md`,
and `agents/*.toml`, all compiled into the binary. Prompts are compact
(≈7 KB vs ≈20 KB shipped), written for entry-graduate-student research code,
and include a minimal progress-update protocol.

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

The simulations and measurements profiles enable three tools backed by the
`codex-job-monitor` crate (semantics ported from the FermiLink harness):

- `jobs.job_attach` — register an already-submitted SLURM job
  (`slurm:<id>`) or detached process (`pid:<pid>`) with its log paths. The
  agent submits through the normal shell, so sandbox and approval machinery
  apply to the submission; a wrapper's PID is not the job.
- `jobs.job_await` — park the turn while deterministic code polls `sacct`
  (falling back to `squeue`) or process liveness with adaptive backoff
  (15 s → 5 min), tailing logs for suspicious lines (NaN, divergence, OOM,
  crashes). Returns on a terminal state, a suspicious log event, repeated
  UNKNOWN scheduler answers, new user input, or a per-call wait budget
  (default 30 min, max 12 h) — the agent burns no turns while waiting and
  simply calls `job_await` again after a budget return.
- `jobs.job_status` — instant snapshot of state history and bounded log
  tails; the reattachment point after a session restart.

Records persist under `$CODEX_HOME/jobs/<thread-id>/`, so a multi-day SLURM
job survives closed laptops and resumed sessions.

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
   just test -p codex-core -p codex-models-manager agent_profile role_ \
       config_loader responses_lite model_switching force_standard
   just test -p codex-tui
   ```

   The upstream tests in that set are load-bearing for the fork:
   `responses_lite` and guardian tests guard the transport-forcing
   boundary; `model_switching` guards custom-provenance survival.
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
