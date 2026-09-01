//! Deterministic monitoring of long-running scientific jobs (fermilink fork).
//!
//! Agents submit jobs through the normal shell, then hand the job ID or
//! detached PID to this crate via the `job_attach`/`job_await`/`job_status`
//! tools. Non-agent code polls SLURM (`sacct`, then `squeue`) or process
//! liveness with adaptive backoff and durable on-disk records, and the agent
//! is resumed only on terminal states, suspicious log events, or a per-call
//! wait budget — never by burning turns on scheduler polling. The semantics
//! (state taxonomy, precedence, alias table, already-dead-PID handling) are
//! ported from the FermiLink harness' monitoring loop.

mod probe;
mod record;
mod state;
mod tail;

pub use probe::pid_alive;
pub use probe::query_slurm_state;
pub use record::JobRecord;
pub use record::JobTarget;
pub use record::StateObservation;
pub use record::WakePolicy;
pub use state::JobPhase;
pub use state::JobState;
pub use state::classify_sacct_output;
pub use state::classify_squeue_output;
pub use state::classify_states;
pub use state::normalize_state_token;
pub use tail::LogTail;
pub use tail::TAIL_BYTES;
pub use tail::compile_watch_patterns;
pub use tail::read_log_tail;
pub use tail::read_log_tail_with_patterns;
pub use tail::validate_watch_pattern;

use std::path::Path;
use std::time::Duration;

/// Poll cadence: start fast, back off toward a five-minute ceiling so a
/// multi-day job costs a handful of scheduler queries per hour.
pub const INITIAL_POLL_INTERVAL: Duration = Duration::from_secs(15);
pub const MAX_POLL_INTERVAL: Duration = Duration::from_secs(300);
const POLL_BACKOFF_NUMERATOR: u32 = 3;
const POLL_BACKOFF_DENOMINATOR: u32 = 2;

/// Consecutive UNKNOWN scheduler answers tolerated before reporting a polling
/// problem instead of waiting forever on a job the scheduler forgot.
pub const UNKNOWN_CONSECUTIVE_LIMIT: u32 = 3;

/// A running job this far past its expected runtime is reported once as
/// suspicious (hung dynamics and deadlocked I/O look like "still running").
pub const OVERRUN_WAKE_RATIO: f64 = 2.0;

/// Returns the next poll interval after `current`.
pub fn next_poll_interval(current: Duration) -> Duration {
    (current * POLL_BACKOFF_NUMERATOR / POLL_BACKOFF_DENOMINATOR).min(MAX_POLL_INTERVAL)
}

/// Parses a human expected-runtime spec — `"90"`, `"45m"`, `"6h"`, `"2d"` —
/// into seconds.
pub fn parse_expected_runtime(spec: &str) -> Result<u64, String> {
    let spec = spec.trim();
    let (number, unit) = match spec.chars().last() {
        Some(unit) if unit.is_ascii_alphabetic() => (&spec[..spec.len() - 1], unit),
        _ => (spec, 's'),
    };
    let value: u64 = number
        .trim()
        .parse()
        .map_err(|_| format!("invalid expected_runtime `{spec}`; use e.g. 90s, 45m, 6h, 2d"))?;
    let seconds = match unit.to_ascii_lowercase() {
        's' => Some(value),
        'm' => value.checked_mul(60),
        'h' => value.checked_mul(3600),
        'd' => value.checked_mul(86400),
        _ => {
            return Err(format!(
                "invalid expected_runtime unit `{unit}`; use s, m, h, or d"
            ));
        }
    };
    seconds
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| format!("expected_runtime `{spec}` must be a positive duration"))
}

/// Probes one target's current state. PID targets report `RUNNING` while
/// alive and a terminal `EXITED` once gone (a dead PID cannot distinguish
/// success from failure; the logs must tell the rest).
pub async fn probe_target(target: &JobTarget) -> JobState {
    match target {
        JobTarget::Slurm { job_id } => query_slurm_state(job_id).await,
        JobTarget::Pid { pid } => {
            if pid_alive(*pid) {
                JobState::active("RUNNING")
            } else {
                JobState {
                    token: "EXITED".to_string(),
                    phase: JobPhase::Completed,
                    detail: None,
                }
            }
        }
    }
}

/// One job's polled status plus its bounded log evidence.
#[derive(Debug, Clone)]
pub struct JobSnapshot {
    pub record: JobRecord,
    pub state: JobState,
    pub log_tails: Vec<LogTail>,
}

impl JobSnapshot {
    pub fn has_suspicious_logs(&self) -> bool {
        self.log_tails
            .iter()
            .any(|tail| !tail.suspicious_lines.is_empty())
    }

    /// `Some(ratio)` once a still-running job has passed
    /// [`OVERRUN_WAKE_RATIO`]× its expected runtime and the overrun has not
    /// been reported to the agent yet.
    pub fn unreported_overrun(&self) -> Option<f64> {
        if self.state.is_terminal() || self.record.overrun_notified {
            return None;
        }
        let ratio = self.record.runtime_ratio(chrono::Utc::now())?;
        (ratio >= OVERRUN_WAKE_RATIO).then_some(ratio)
    }
}

/// Probes a target, updates and persists its record, and gathers log tails
/// scanned with the record's extra watch patterns.
pub async fn snapshot_job(store_dir: &Path, mut record: JobRecord) -> std::io::Result<JobSnapshot> {
    let state = probe_target(&record.target).await;
    record.observe(state.clone());
    record.save(store_dir).await?;
    let extra_patterns = compile_watch_patterns(&record.watch_patterns);
    let mut log_tails = Vec::with_capacity(record.log_paths.len());
    for path in &record.log_paths {
        log_tails.push(read_log_tail_with_patterns(path, &extra_patterns).await);
    }
    Ok(JobSnapshot {
        record,
        state,
        log_tails,
    })
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
