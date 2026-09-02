//! Deterministic probes for SLURM jobs and detached processes.

use crate::state::JobPhase;
use crate::state::JobState;
use crate::state::classify_sacct_output;
use crate::state::classify_squeue_output;
use std::time::Duration;
use tokio::process::Command;

const SLURM_QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// slurmctld's `ESLURM_INVALID_JOB_ID` message: the controller answered, and
/// it has no record of the job.
const SLURM_INVALID_JOB_ID: &str = "Invalid job id specified";

/// One scheduler command's answer.
enum QueryOutcome {
    /// Exit status zero; stdout, possibly empty.
    Ok(String),
    /// Non-zero exit status; stderr, which tells "job unknown" from
    /// "controller down".
    Failed(String),
    /// Missing binary, spawn failure, or timeout.
    Unavailable,
}

async fn run_query(program: &str, args: &[&str]) -> QueryOutcome {
    let Ok(Ok(output)) = tokio::time::timeout(
        SLURM_QUERY_TIMEOUT,
        Command::new(program).args(args).output(),
    )
    .await
    else {
        return QueryOutcome::Unavailable;
    };
    if output.status.success() {
        QueryOutcome::Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        QueryOutcome::Failed(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// What the scheduler knows about one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlurmProbe {
    /// A recognizable state from `sacct` or `squeue`.
    State(JobState),
    /// The controller answered but has no record of the job ID. Without an
    /// accounting database a job vanishes `MinJobAge` seconds (300 by
    /// default) after it ends, so this is how every finished job eventually
    /// looks where `AccountingStorageType=none` — or the ID is wrong.
    NotFound,
    /// No usable answer: controller unreachable, commands missing or timing
    /// out, or an unrecognized state token.
    Unavailable,
}

/// Queries one SLURM job, preferring `sacct` (which knows finished jobs
/// wherever accounting is configured) and falling back to `squeue
/// --states=all`. The `--states=all` matters: squeue's default filter shows
/// only pending, running, and completing jobs, so a job that just finished
/// would look like it never existed even while slurmctld still remembers it.
pub async fn probe_slurm(job_id: &str) -> SlurmProbe {
    if let QueryOutcome::Ok(stdout) =
        run_query("sacct", &["-n", "-P", "-o", "JobID,State", "-j", job_id]).await
    {
        let state = classify_sacct_output(&stdout, job_id);
        if !matches!(state.phase, JobPhase::Unknown) {
            return SlurmProbe::State(state);
        }
    }
    match run_query("squeue", &["-h", "--states=all", "-j", job_id, "-o", "%T"]).await {
        QueryOutcome::Ok(stdout) => {
            let state = classify_squeue_output(&stdout);
            if matches!(state.phase, JobPhase::Unknown) {
                SlurmProbe::Unavailable
            } else {
                SlurmProbe::State(state)
            }
        }
        QueryOutcome::Failed(stderr) if stderr.contains(SLURM_INVALID_JOB_ID) => {
            SlurmProbe::NotFound
        }
        QueryOutcome::Failed(_) | QueryOutcome::Unavailable => SlurmProbe::Unavailable,
    }
}

/// Whether a detached process is still alive.
///
/// A dead PID cannot distinguish success from failure, so PID jobs complete
/// with an `EXITED` state and the logs must tell the rest. On Linux a zombie
/// (exited, not yet reaped by its parent) counts as dead: it still answers
/// `kill(pid, 0)`, but the job is over.
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    // Signal 0 performs error checking only. EPERM still proves existence.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    let exists = result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    exists && !is_zombie(pid)
}

/// `/proc/<pid>/stat` reads `pid (comm) STATE ...`; `comm` may itself contain
/// spaces or parentheses, so the state is the first field after the last `)`.
#[cfg(target_os = "linux")]
fn is_zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            let after_comm = &stat[stat.rfind(')')? + 1..];
            after_comm
                .split_whitespace()
                .next()
                .map(|state| matches!(state, "Z" | "X"))
        })
        .unwrap_or(false)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn is_zombie(_pid: u32) -> bool {
    false
}

#[cfg(not(unix))]
pub fn pid_alive(_pid: u32) -> bool {
    // Windows PID liveness needs OpenProcess semantics this fork does not
    // wire up yet; report not-alive so awaits return instead of spinning.
    false
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
