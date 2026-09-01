//! Built-in agent profiles.
//!
//! An agent profile selects the replacement base instructions used for newly
//! created threads. The `default` profile supplies no replacement so Codex
//! keeps the shipped model instructions; every other profile carries a
//! complete replacement prompt tailored to a specific scientific workflow,
//! and may bundle subagent roles, Ultra-only multi-agent guidance, and
//! capabilities such as deterministic long-running job monitoring. Profiles
//! are independent from the selected model and reasoning effort.

/// Identifier of the built-in profile that keeps the shipped Codex behavior.
pub const DEFAULT_AGENT_PROFILE_ID: &str = "default";

/// Identifier of the scientific algorithm research profile.
pub const SCIENTIFIC_ALGORITHM_PROFILE_ID: &str = "scientific-algorithm";

/// Identifier of the scientific simulations profile.
pub const SCIENTIFIC_SIMULATIONS_PROFILE_ID: &str = "scientific-simulations";

/// Identifier of the scientific measurements profile.
pub const SCIENTIFIC_MEASUREMENTS_PROFILE_ID: &str = "scientific-measurements";

const SCIENTIFIC_ALGORITHM_PROMPT: &str =
    include_str!("../profiles/scientific-algorithm/prompt.md");
const SCIENTIFIC_ALGORITHM_MULTI_AGENT_GUIDANCE: &str =
    include_str!("../profiles/scientific-algorithm/multi_agent.md");

const SCIENTIFIC_SIMULATIONS_PROMPT: &str =
    include_str!("../profiles/scientific-simulations/prompt.md");
const SCIENTIFIC_SIMULATIONS_MULTI_AGENT_GUIDANCE: &str =
    include_str!("../profiles/scientific-simulations/multi_agent.md");

const SCIENTIFIC_MEASUREMENTS_PROMPT: &str =
    include_str!("../profiles/scientific-measurements/prompt.md");
const SCIENTIFIC_MEASUREMENTS_MULTI_AGENT_GUIDANCE: &str =
    include_str!("../profiles/scientific-measurements/multi_agent.md");

/// A selectable agent profile for new threads.
///
/// `base_instructions` is `None` only for the default profile, which keeps
/// the model's shipped instructions; any other profile fully replaces them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentProfile {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub base_instructions: Option<&'static str>,
    /// Subagent roles shipped with this profile, offered to the spawn tool
    /// in addition to the generic built-in roles.
    pub roles: &'static [AgentProfileRole],
    /// Orchestration guidance attached to the spawn tool at Ultra reasoning
    /// effort only, so lower efforts keep a delegation-free prompt surface.
    pub multi_agent_guidance: Option<&'static str>,
    /// Extra tooling this profile enables.
    pub capabilities: &'static [ProfileCapability],
}

/// Extra tooling a profile can enable beyond prompts and roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileCapability {
    /// Deterministic monitoring of long-running SLURM jobs and detached
    /// processes (`job_attach` / `job_await` / `job_status`), so agents are
    /// resumed on completion instead of polling schedulers themselves.
    JobMonitor,
}

/// A subagent role bundled with an agent profile. `config_path` is a virtual
/// path resolved to `config_contents` (a ConfigToml overlay, the same format
/// as `$CODEX_HOME/agents/*.toml` role files without the name/description
/// keys) through [`find_agent_profile_role_config`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentProfileRole {
    pub name: &'static str,
    pub description: &'static str,
    pub config_path: &'static str,
    pub config_contents: &'static str,
}

const SCIENTIFIC_ALGORITHM_ROLES: &[AgentProfileRole] = &[
    AgentProfileRole {
        name: "algorithm_theorist",
        description: "Scientific theorist for deriving distinct candidate algorithms, assumptions, invariants, and falsifiable predictions before implementation.",
        config_path: "agent_profile/scientific-algorithm/algorithm_theorist.toml",
        config_contents: include_str!(
            "../profiles/scientific-algorithm/agents/algorithm_theorist.toml"
        ),
    },
    AgentProfileRole {
        name: "scaling_analyst",
        description: "Complexity and accelerator analyst for lower-N-scaling, memory traffic, communication, synchronization, and parallel-depth assessment.",
        config_path: "agent_profile/scientific-algorithm/scaling_analyst.toml",
        config_contents: include_str!(
            "../profiles/scientific-algorithm/agents/scaling_analyst.toml"
        ),
    },
    AgentProfileRole {
        name: "numerical_falsifier",
        description: "Adversarial numerical analyst for counterexamples, conditioning, stability, convergence, precision, and scientific validation.",
        config_path: "agent_profile/scientific-algorithm/numerical_falsifier.toml",
        config_contents: include_str!(
            "../profiles/scientific-algorithm/agents/numerical_falsifier.toml"
        ),
    },
    AgentProfileRole {
        name: "independent_replicator",
        description: "Independent scientific replicator that rederives the method and checks whether results follow without relying on the main agent's rationale.",
        config_path: "agent_profile/scientific-algorithm/independent_replicator.toml",
        config_contents: include_str!(
            "../profiles/scientific-algorithm/agents/independent_replicator.toml"
        ),
    },
    AgentProfileRole {
        name: "gpu_implementer",
        description: "Scientific GPU implementer for minimal JAX, PyTorch, CuPy, Triton, or CUDA reference implementations after algorithm selection.",
        config_path: "agent_profile/scientific-algorithm/gpu_implementer.toml",
        config_contents: include_str!(
            "../profiles/scientific-algorithm/agents/gpu_implementer.toml"
        ),
    },
];

const SCIENTIFIC_SIMULATIONS_ROLES: &[AgentProfileRole] = &[
    AgentProfileRole {
        name: "model_auditor",
        description: "Physical-model auditor for governing equations, assumptions, units, boundary conditions, and parameter validity before production runs.",
        config_path: "agent_profile/scientific-simulations/model_auditor.toml",
        config_contents: include_str!(
            "../profiles/scientific-simulations/agents/model_auditor.toml"
        ),
    },
    AgentProfileRole {
        name: "convergence_analyst",
        description: "Convergence designer and judge for resolution, timestep, domain size, tolerances, equilibration, and sampling of the target observables.",
        config_path: "agent_profile/scientific-simulations/convergence_analyst.toml",
        config_contents: include_str!(
            "../profiles/scientific-simulations/agents/convergence_analyst.toml"
        ),
    },
    AgentProfileRole {
        name: "result_falsifier",
        description: "Adversarial reviewer attacking conservation drift, unconverged claims, statistical malpractice, and benchmark disagreement in simulation results.",
        config_path: "agent_profile/scientific-simulations/result_falsifier.toml",
        config_contents: include_str!(
            "../profiles/scientific-simulations/agents/result_falsifier.toml"
        ),
    },
    AgentProfileRole {
        name: "independent_replicator",
        description: "Independent scientific replicator that reproduces a key simulation result from the specification and artifacts, preferably by another route.",
        config_path: "agent_profile/scientific-simulations/independent_replicator.toml",
        config_contents: include_str!(
            "../profiles/scientific-simulations/agents/independent_replicator.toml"
        ),
    },
    AgentProfileRole {
        name: "simulation_implementer",
        description: "Simulation implementer for input decks, thin drivers, submit scripts, and analysis after the setup is agreed; the write-owning role.",
        config_path: "agent_profile/scientific-simulations/simulation_implementer.toml",
        config_contents: include_str!(
            "../profiles/scientific-simulations/agents/simulation_implementer.toml"
        ),
    },
];

const SCIENTIFIC_MEASUREMENTS_ROLES: &[AgentProfileRole] = &[
    AgentProfileRole {
        name: "experimental_designer",
        description: "Experimental designer for decisive acquisitions, controls, randomization, sample budgets, and pre-registered analysis choices.",
        config_path: "agent_profile/scientific-measurements/experimental_designer.toml",
        config_contents: include_str!(
            "../profiles/scientific-measurements/agents/experimental_designer.toml"
        ),
    },
    AgentProfileRole {
        name: "calibration_auditor",
        description: "Calibration-chain auditor for standards, drift, gain and unit handling, instrument settings, and signal-integrity pitfalls.",
        config_path: "agent_profile/scientific-measurements/calibration_auditor.toml",
        config_contents: include_str!(
            "../profiles/scientific-measurements/agents/calibration_auditor.toml"
        ),
    },
    AgentProfileRole {
        name: "uncertainty_analyst",
        description: "Uncertainty-budget builder and attacker covering statistics, systematics, correlations, propagation, and sensitivity to analysis choices.",
        config_path: "agent_profile/scientific-measurements/uncertainty_analyst.toml",
        config_contents: include_str!(
            "../profiles/scientific-measurements/agents/uncertainty_analyst.toml"
        ),
    },
    AgentProfileRole {
        name: "independent_replicator",
        description: "Independent analyst that re-derives the measured result from raw data without adopting the primary analysis' choices.",
        config_path: "agent_profile/scientific-measurements/independent_replicator.toml",
        config_contents: include_str!(
            "../profiles/scientific-measurements/agents/independent_replicator.toml"
        ),
    },
    AgentProfileRole {
        name: "acquisition_implementer",
        description: "Acquisition implementer for instrument drivers and analysis pipelines; the only role that may drive hardware, within confirmed limits.",
        config_path: "agent_profile/scientific-measurements/acquisition_implementer.toml",
        config_contents: include_str!(
            "../profiles/scientific-measurements/agents/acquisition_implementer.toml"
        ),
    },
];

/// All built-in profiles, in display order for pickers.
pub const BUILT_IN_AGENT_PROFILES: &[AgentProfile] = &[
    AgentProfile {
        id: DEFAULT_AGENT_PROFILE_ID,
        display_name: "Default Codex",
        description: "Shipped Codex instructions and software-engineering workflow",
        base_instructions: None,
        roles: &[],
        multi_agent_guidance: None,
        capabilities: &[],
    },
    AgentProfile {
        id: SCIENTIFIC_ALGORITHM_PROFILE_ID,
        display_name: "Scientific Algorithm",
        description: "Hypothesis-driven search for correct, lower-scaling algorithms and long-benchmark monitoring",
        base_instructions: Some(SCIENTIFIC_ALGORITHM_PROMPT),
        roles: SCIENTIFIC_ALGORITHM_ROLES,
        multi_agent_guidance: Some(SCIENTIFIC_ALGORITHM_MULTI_AGENT_GUIDANCE),
        capabilities: &[ProfileCapability::JobMonitor],
    },
    AgentProfile {
        id: SCIENTIFIC_SIMULATIONS_PROFILE_ID,
        display_name: "Scientific Simulations",
        description: "HPC simulation setup, convergence, long-job monitoring, and validation",
        base_instructions: Some(SCIENTIFIC_SIMULATIONS_PROMPT),
        roles: SCIENTIFIC_SIMULATIONS_ROLES,
        multi_agent_guidance: Some(SCIENTIFIC_SIMULATIONS_MULTI_AGENT_GUIDANCE),
        capabilities: &[ProfileCapability::JobMonitor],
    },
    AgentProfile {
        id: SCIENTIFIC_MEASUREMENTS_PROFILE_ID,
        display_name: "Scientific Measurements",
        description: "Calibration, uncertainty, provenance, and long-acquisition monitoring",
        base_instructions: Some(SCIENTIFIC_MEASUREMENTS_PROMPT),
        roles: SCIENTIFIC_MEASUREMENTS_ROLES,
        multi_agent_guidance: Some(SCIENTIFIC_MEASUREMENTS_MULTI_AGENT_GUIDANCE),
        capabilities: &[ProfileCapability::JobMonitor],
    },
];

/// Looks up a built-in profile by identifier.
pub fn find_agent_profile(id: &str) -> Option<&'static AgentProfile> {
    BUILT_IN_AGENT_PROFILES
        .iter()
        .find(|profile| profile.id == id)
}

/// Looks up the built-in profile whose replacement instructions match `text`
/// exactly, used to recognize profile-created threads on resume.
pub fn find_agent_profile_by_instructions(text: &str) -> Option<&'static AgentProfile> {
    BUILT_IN_AGENT_PROFILES
        .iter()
        .find(|profile| profile.base_instructions == Some(text))
}

/// Resolves a profile role's virtual `config_path` to its embedded contents.
pub fn find_agent_profile_role_config(path: &str) -> Option<&'static str> {
    BUILT_IN_AGENT_PROFILES
        .iter()
        .flat_map(|profile| profile.roles)
        .find(|role| role.config_path == path)
        .map(|role| role.config_contents)
}

/// Whether the identified profile enables the given capability.
pub fn profile_has_capability(id: &str, capability: ProfileCapability) -> bool {
    find_agent_profile(id).is_some_and(|profile| profile.capabilities.contains(&capability))
}
