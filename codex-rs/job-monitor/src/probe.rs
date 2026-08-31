//! Deterministic probes for SLURM jobs and detached processes.

use crate::state::JobState;
use crate::state::classify_sacct_output;
use crate::state::classify_squeue_output;
use std::time::Duration;
use tokio::process::Command;

const SLURM_QUERY_TIMEOUT: Duration = Duration::from_secs(10);

async fn run_query(program: &str, args: &[&str]) -> Option<String> {
    let output = tokio::time::timeout(
        SLURM_QUERY_TIMEOUT,
        Command::new(program).args(args).output(),
    )
    .await
    .ok()?
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Queries one SLURM job's state, preferring `sacct` (which knows finished
/// jobs) and falling back to `squeue` (which only knows queued/running ones).
/// Returns `JobState::unknown()` when neither tool yields a usable answer.
pub async fn query_slurm_state(job_id: &str) -> JobState {
    let sacct = run_query("sacct", &["-n", "-P", "-o", "JobID,State", "-j", job_id]).await;
    if let Some(stdout) = sacct {
        let state = classify_sacct_output(&stdout, job_id);
        if !matches!(state.phase, crate::state::JobPhase::Unknown) {
            return state;
        }
    }
    let squeue = run_query("squeue", &["-h", "-j", job_id, "-o", "%T"]).await;
    if let Some(stdout) = squeue {
        let state = classify_squeue_output(&stdout);
        if !matches!(state.phase, crate::state::JobPhase::Unknown) {
            return state;
        }
    }
    JobState::unknown()
}

/// Whether a detached process is still alive.
///
/// A dead PID cannot distinguish success from failure, so PID jobs complete
/// with an `EXITED` state and the logs must tell the rest.
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    // Signal 0 performs error checking only. EPERM still proves existence.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
