//! SLURM job-state taxonomy and classification.
//!
//! Ported from the FermiLink harness' battle-tested monitoring loop: a job
//! (or job array/steps) may report several states at once, and a failure
//! state outranks an active state, which outranks completion.

use serde::Deserialize;
use serde::Serialize;

/// Coarse classification of a monitored job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    Active,
    Completed,
    Failed,
    /// The scheduler could not be queried or returned nothing usable.
    Unknown,
}

/// A normalized SLURM state token plus its coarse phase. For job arrays the
/// classifying token follows the usual precedence while `detail` carries the
/// per-task state counts (e.g. `7×RUNNING, 3×PENDING, 2×COMPLETED`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobState {
    pub token: String,
    pub phase: JobPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl JobState {
    pub fn unknown() -> Self {
        Self {
            token: "UNKNOWN".to_string(),
            phase: JobPhase::Unknown,
            detail: None,
        }
    }

    pub fn completed() -> Self {
        Self {
            token: "COMPLETED".to_string(),
            phase: JobPhase::Completed,
            detail: None,
        }
    }

    pub fn active(token: &str) -> Self {
        Self {
            token: token.to_string(),
            phase: JobPhase::Active,
            detail: None,
        }
    }

    pub fn failed(token: &str) -> Self {
        Self {
            token: token.to_string(),
            phase: JobPhase::Failed,
            detail: None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.phase, JobPhase::Completed | JobPhase::Failed)
    }

    /// The token plus the array-task counts when present.
    pub fn display(&self) -> String {
        match &self.detail {
            Some(detail) => format!("{} ({detail})", self.token),
            None => self.token.clone(),
        }
    }
}

const SLURM_FAILURE_STATES: &[&str] = &[
    "FAILED",
    "CANCELLED",
    "TIMEOUT",
    "NODE_FAIL",
    "OUT_OF_MEMORY",
    "PREEMPTED",
    "BOOT_FAIL",
    "DEADLINE",
    "REVOKED",
    "SPECIAL_EXIT",
    "STOPPED",
];

const SLURM_ACTIVE_STATES: &[&str] = &[
    "PENDING",
    "CONFIGURING",
    "RUNNING",
    "COMPLETING",
    "RESIZING",
    "SUSPENDED",
    "SIGNALING",
    "STAGE_OUT",
    "REQUEUED",
    "REQUEUE_FED",
    "REQUEUE_HOLD",
    "RESV_DEL_HOLD",
    "POWER_UP_NODE",
];

/// squeue/sacct short codes mapped to canonical state names.
const SLURM_STATE_ALIASES: &[(&str, &str)] = &[
    ("BF", "BOOT_FAIL"),
    ("CA", "CANCELLED"),
    ("CANCELED", "CANCELLED"),
    ("CD", "COMPLETED"),
    ("CF", "CONFIGURING"),
    ("CG", "COMPLETING"),
    ("DL", "DEADLINE"),
    ("F", "FAILED"),
    ("NF", "NODE_FAIL"),
    ("OOM", "OUT_OF_MEMORY"),
    ("PD", "PENDING"),
    ("PR", "PREEMPTED"),
    ("R", "RUNNING"),
    ("RD", "RESV_DEL_HOLD"),
    ("RF", "REQUEUE_FED"),
    ("RH", "REQUEUE_HOLD"),
    ("RQ", "REQUEUED"),
    ("RS", "RESIZING"),
    ("RV", "REVOKED"),
    ("SE", "SPECIAL_EXIT"),
    ("SI", "SIGNALING"),
    ("SO", "STAGE_OUT"),
    ("ST", "STOPPED"),
    ("S", "SUSPENDED"),
    ("TO", "TIMEOUT"),
];

/// Normalizes one raw scheduler token (`CANCELLED+`, `PD`, `RUNNING|...`) to a
/// canonical state name, or `None` when the token is not a known state.
pub fn normalize_state_token(raw: &str) -> Option<String> {
    let token = raw.trim();
    let token = token.split('|').next().unwrap_or_default().trim();
    let token = token.split('+').next().unwrap_or_default().trim();
    let token = token.split_whitespace().next().unwrap_or_default();
    if token.is_empty() {
        return None;
    }
    let token = token.to_ascii_uppercase();
    let token = SLURM_STATE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == token)
        .map_or(token.as_str(), |(_, canonical)| canonical)
        .to_string();
    let known = token == "COMPLETED"
        || SLURM_FAILURE_STATES.contains(&token.as_str())
        || SLURM_ACTIVE_STATES.contains(&token.as_str());
    known.then_some(token)
}

/// Classifies a set of per-step states into one job state: failure outranks
/// active, which outranks completed; no recognizable state means unknown.
/// With more than one state (a job array), `detail` carries the counts.
pub fn classify_states(states: &[String]) -> JobState {
    let mut classified = 'classify: {
        for state in states {
            if SLURM_FAILURE_STATES.contains(&state.as_str()) {
                break 'classify JobState::failed(state);
            }
        }
        for state in states {
            if SLURM_ACTIVE_STATES.contains(&state.as_str()) {
                break 'classify JobState::active(state);
            }
        }
        if states.iter().any(|state| state == "COMPLETED") {
            break 'classify JobState::completed();
        }
        JobState::unknown()
    };
    classified.detail = state_counts_detail(states);
    classified
}

/// `7×RUNNING, 3×PENDING` for multi-state (array) observations; `None` for
/// zero or one state, where the token already says everything.
fn state_counts_detail(states: &[String]) -> Option<String> {
    if states.len() < 2 {
        return None;
    }
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for state in states {
        *counts.entry(state.as_str()).or_default() += 1;
    }
    Some(
        counts
            .iter()
            .map(|(state, count)| format!("{count}\u{d7}{state}"))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// Whether a sacct `JobID` column value belongs to `job_id`: the job itself,
/// or — when `job_id` is an array parent — one of its tasks (`123_7`, and the
/// `123_[8-20]` form sacct uses for still-pending task ranges). Step rows
/// (`123.batch`, `123_7.extern`) are excluded so a failed prolog step cannot
/// mask the allocation's state precedence rules.
fn sacct_row_matches(job_token: &str, job_id: &str) -> bool {
    if job_token == job_id {
        return true;
    }
    job_token
        .strip_prefix(job_id)
        .and_then(|rest| rest.strip_prefix('_'))
        .is_some_and(|task| {
            !task.is_empty() && (task.chars().all(|c| c.is_ascii_digit()) || task.starts_with('['))
        })
}

/// Parses `sacct -n -P -o JobID,State` output and classifies the states that
/// belong to `job_id`: its own rows, plus every task row when the ID names a
/// job array parent.
pub fn classify_sacct_output(stdout: &str, job_id: &str) -> JobState {
    let mut states = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.split('|');
        let (Some(job_token), Some(state_raw)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !sacct_row_matches(job_token.trim(), job_id) {
            continue;
        }
        if let Some(state) = normalize_state_token(state_raw) {
            states.push(state);
        }
    }
    classify_states(&states)
}

/// Parses `squeue -h -j <id> -o %T` output (one state token per line; a job
/// array yields one line per queued or running task).
pub fn classify_squeue_output(stdout: &str) -> JobState {
    let states: Vec<String> = stdout.lines().filter_map(normalize_state_token).collect();
    classify_states(&states)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
