//! Session-resident job watcher (fermilink fork).
//!
//! When a turn ends while attached jobs are still active, nothing would
//! otherwise wake the agent when a job finishes. This watcher subscribes to
//! the session's agent-status channel and, while the agent is idle, polls the
//! thread's durable job records with the same deterministic engine the
//! `jobs.*` tools use. When a job reaches a terminal state or a suspicious
//! log line appears (or an optional check-in interval elapses), it injects a
//! bounded `[job monitor]` message as user input, which starts a new turn
//! with full session context — so an "end-to-end" workflow keeps moving
//! without the agent polling or the user babysitting. The task exits when
//! the status channel closes at session teardown.

use crate::agent::AgentStatus;
use crate::agent::control::AgentControl;
use codex_job_monitor::INITIAL_POLL_INTERVAL;
use codex_job_monitor::JobRecord;
use codex_job_monitor::JobSnapshot;
use codex_job_monitor::next_poll_interval;
use codex_job_monitor::snapshot_job;
use codex_protocol::ThreadId;
use codex_protocol::turn_input::TurnStartOptions;
use codex_protocol::user_input::UserInput;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::watch;

/// `turn_trigger` recorded on turns the watcher starts.
const JOB_MONITOR_TURN_TRIGGER: &str = "job-monitor";

pub(crate) struct JobWatcherHandles {
    pub(crate) thread_id: ThreadId,
    pub(crate) store_dir: PathBuf,
    pub(crate) agent_status: watch::Receiver<AgentStatus>,
    pub(crate) agent_control: AgentControl,
    pub(crate) check_in: Option<Duration>,
    pub(crate) max_auto_continues: u32,
}

/// Spawns the watcher task; call once per session when the fork's job
/// monitoring capability and `jobs.auto_continue` are active.
pub(crate) fn spawn_job_watcher(handles: JobWatcherHandles) {
    tokio::spawn(watch_loop(handles));
}

async fn watch_loop(mut handles: JobWatcherHandles) {
    let mut auto_continues: u32 = 0;
    loop {
        // Wait for the agent to be idle after a completed turn. Interrupted
        // sessions are deliberately not resumed: the user intervened.
        while !matches!(*handles.agent_status.borrow(), AgentStatus::Completed(_)) {
            if handles.agent_status.changed().await.is_err() {
                return;
            }
        }

        let mut poll_interval = INITIAL_POLL_INTERVAL;
        let mut idle_for = Duration::ZERO;
        let mut next_check_in = handles.check_in;
        loop {
            if !matches!(*handles.agent_status.borrow(), AgentStatus::Completed(_)) {
                // A new turn started (user input or our injection); go back
                // to waiting for the next idle period.
                break;
            }

            let records = JobRecord::load_all(&handles.store_dir)
                .await
                .unwrap_or_default();
            let watched: Vec<JobRecord> = records
                .into_iter()
                .filter(|record| record.notified_at.is_none())
                .collect();
            if watched.is_empty() {
                // Nothing to watch; sleep until the status changes again.
                if handles.agent_status.changed().await.is_err() {
                    return;
                }
                break;
            }

            let mut snapshots = Vec::with_capacity(watched.len());
            for record in watched {
                match snapshot_job(&handles.store_dir, record).await {
                    Ok(snapshot) => snapshots.push(snapshot),
                    Err(error) => {
                        tracing::warn!(%error, "job watcher failed to poll a job record");
                    }
                }
            }

            let reason = wake_reason(&snapshots, idle_for, &mut next_check_in);
            if let Some(reason) = reason {
                if auto_continues >= handles.max_auto_continues {
                    tracing::warn!(
                        "job watcher reached jobs.max_auto_continues ({}); not waking the agent again",
                        handles.max_auto_continues
                    );
                    return;
                }
                auto_continues += 1;
                // Record the notification before injecting so a failed turn
                // start cannot produce a wake loop.
                for snapshot in &snapshots {
                    let mut record = snapshot.record.clone();
                    if snapshot.state.is_terminal() {
                        record.notified_at = Some(chrono::Utc::now());
                    }
                    if snapshot.has_suspicious_logs() {
                        record.suspicious_signature = Some(suspicious_signature(snapshot));
                    }
                    if let Err(error) = record.save(&handles.store_dir).await {
                        tracing::warn!(%error, "job watcher failed to persist notification state");
                    }
                }
                let message = wake_message(&reason, &snapshots);
                let result = handles
                    .agent_control
                    .send_input(
                        handles.thread_id,
                        vec![UserInput::Text {
                            text: message,
                            text_elements: Vec::new(),
                        }],
                        TurnStartOptions {
                            turn_trigger: Some(JOB_MONITOR_TURN_TRIGGER.to_string()),
                            ..Default::default()
                        },
                    )
                    .await;
                if let Err(error) = result {
                    tracing::warn!(%error, "job watcher failed to wake the agent");
                }
                break;
            }

            let sleep = tokio::time::sleep(poll_interval);
            tokio::pin!(sleep);
            tokio::select! {
                () = &mut sleep => {
                    idle_for += poll_interval;
                    poll_interval = next_poll_interval(poll_interval);
                }
                changed = handles.agent_status.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    break;
                }
            }
        }
    }
}

/// Decides whether the idle agent should be woken, and why.
fn wake_reason(
    snapshots: &[JobSnapshot],
    idle_for: Duration,
    next_check_in: &mut Option<Duration>,
) -> Option<String> {
    if let Some(terminal) = snapshots
        .iter()
        .find(|snapshot| snapshot.state.is_terminal())
    {
        return Some(format!(
            "{} reached terminal state {}",
            terminal.record.target.display(),
            terminal.state.token
        ));
    }
    if let Some(suspicious) = snapshots.iter().find(|snapshot| {
        snapshot.has_suspicious_logs()
            && snapshot.record.suspicious_signature.as_deref()
                != Some(suspicious_signature(snapshot).as_str())
    }) {
        return Some(format!(
            "suspicious log lines appeared for {}",
            suspicious.record.target.display()
        ));
    }
    if let Some(interval) = *next_check_in
        && idle_for >= interval
    {
        *next_check_in = Some(interval + interval);
        return Some(format!(
            "check-in: jobs still running after {} min idle",
            idle_for.as_secs() / 60
        ));
    }
    None
}

pub(crate) fn suspicious_signature(snapshot: &JobSnapshot) -> String {
    let mut text = String::new();
    for tail in &snapshot.log_tails {
        for line in &tail.suspicious_lines {
            text.push_str(line);
            text.push('\n');
        }
    }
    format!("{:x}", fnv1a(&text))
}

// Small non-cryptographic fingerprint; collisions only risk a skipped or
// repeated wake notice, never correctness.
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn wake_message(reason: &str, snapshots: &[JobSnapshot]) -> String {
    let mut message = format!(
        "[job monitor] {reason}. This is an automated wake-up from the deterministic job watcher.\n\n"
    );
    for snapshot in snapshots {
        let label = snapshot
            .record
            .label
            .as_deref()
            .map(|label| format!(" ({label})"))
            .unwrap_or_default();
        let _ = writeln!(
            message,
            "{}{label}: {} [{:?}]",
            snapshot.record.target.display(),
            snapshot.state.token,
            snapshot.state.phase,
        );
        for tail in &snapshot.log_tails {
            let _ = writeln!(
                message,
                "  {} ({} bytes)",
                tail.path.display(),
                tail.file_len
            );
            for line in tail.suspicious_lines.iter().take(5) {
                let _ = writeln!(message, "    suspicious: {line}");
            }
        }
    }
    message.push_str(
        "\nClassify each job's outcome from its state and logs (and update the project's memory.md), then continue the workflow: verify expected artifacts, post-process completed work, diagnose failures, and proceed to the next step. Use jobs.job_status for details.",
    );
    message
}

#[cfg(test)]
#[path = "job_watcher_tests.rs"]
mod tests;
