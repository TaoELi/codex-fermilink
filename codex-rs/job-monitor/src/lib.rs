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
pub use state::JobPhase;
pub use state::JobState;
pub use state::classify_sacct_output;
pub use state::classify_squeue_output;
pub use state::classify_states;
pub use state::normalize_state_token;
pub use tail::LogTail;
pub use tail::TAIL_BYTES;
pub use tail::read_log_tail;

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

/// Returns the next poll interval after `current`.
pub fn next_poll_interval(current: Duration) -> Duration {
    (current * POLL_BACKOFF_NUMERATOR / POLL_BACKOFF_DENOMINATOR).min(MAX_POLL_INTERVAL)
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
}

/// Probes a target, updates and persists its record, and gathers log tails.
pub async fn snapshot_job(store_dir: &Path, mut record: JobRecord) -> std::io::Result<JobSnapshot> {
    let state = probe_target(&record.target).await;
    record.observe(state.clone());
    record.save(store_dir).await?;
    let mut log_tails = Vec::with_capacity(record.log_paths.len());
    for path in &record.log_paths {
        log_tails.push(read_log_tail(path).await);
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
