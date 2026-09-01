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
use crate::session::session::Session;
use crate::tasks::CompactTask;
use codex_job_monitor::INITIAL_POLL_INTERVAL;
use codex_job_monitor::JobPhase;
use codex_job_monitor::JobRecord;
use codex_job_monitor::JobSnapshot;
use codex_job_monitor::WakePolicy;
use codex_job_monitor::next_poll_interval;
use codex_job_monitor::snapshot_job;
use codex_protocol::ThreadId;
use codex_protocol::turn_input::TurnStartOptions;
use codex_protocol::user_input::UserInput;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Weak;
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

/// `turn_trigger` recorded on turns the watcher starts.
const JOB_MONITOR_TURN_TRIGGER: &str = "job-monitor";

/// Below this context size, compact-before-wake is skipped: the wake turn is
/// cheap anyway and the raw history is worth more than the summary.
const COMPACT_BEFORE_WAKE_MIN_TOKENS: i64 = 50_000;

/// Upper bound on waiting for the pre-wake compaction turn to finish; on
/// expiry the wake proceeds on the uncompacted history.
const COMPACT_WAIT_TIMEOUT: Duration = Duration::from_secs(600);

pub(crate) struct JobWatcherHandles {
    pub(crate) thread_id: ThreadId,
    pub(crate) store_dir: PathBuf,
    pub(crate) agent_status: watch::Receiver<AgentStatus>,
    pub(crate) agent_control: AgentControl,
    /// Weak so the watcher never keeps a torn-down session alive; compaction
    /// is skipped once the session is gone.
    pub(crate) session: Weak<Session>,
    pub(crate) check_in: Option<Duration>,
    pub(crate) max_auto_continues: u32,
    pub(crate) compact_before_wake: bool,
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

            let decision = wake_decision(&snapshots, idle_for, &mut next_check_in);
            if let Some((reason, cause)) = decision {
                if auto_continues >= handles.max_auto_continues {
                    tracing::warn!(
                        "job watcher reached jobs.max_auto_continues ({}); not waking the agent again",
                        handles.max_auto_continues
                    );
                    return;
                }
                auto_continues += 1;
                // Record the notification before injecting so a failed turn
                // start cannot produce a wake loop. Only the records this
                // wake is about are marked: a sweep member that merely
                // completed stays watched until its whole batch is terminal.
                for &index in cause.notified_indices() {
                    let snapshot = &snapshots[index];
                    let mut record = snapshot.record.clone();
                    match cause {
                        WakeCause::Terminal { .. } | WakeCause::BatchComplete { .. } => {
                            record.notified_at = Some(chrono::Utc::now());
                            if snapshot.has_suspicious_logs() {
                                record.suspicious_signature = Some(suspicious_signature(snapshot));
                            }
                        }
                        WakeCause::Suspicious { .. } => {
                            record.suspicious_signature = Some(suspicious_signature(snapshot));
                        }
                        WakeCause::Overrun { .. } => {
                            record.overrun_notified = true;
                        }
                        WakeCause::CheckIn => {}
                    }
                    if let Err(error) = record.save(&handles.store_dir).await {
                        tracing::warn!(%error, "job watcher failed to persist notification state");
                    }
                }
                // Long jobs outlive the provider prompt cache, so a wake turn
                // dragging a large history is billed at full input price and
                // reasons over stale detail. Compacting first makes the wake
                // turn start from the summary plus the durable `memory.md`.
                compact_history_before_wake(&mut handles).await;
                let message = wake_message(&reason);
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

/// What a wake-up is about; carries the snapshot indices whose records must
/// be marked so the same outcome never wakes the agent twice.
#[derive(Debug, PartialEq, Eq)]
enum WakeCause {
    /// A job whose completion wakes on its own (`each` policy), or a batch
    /// member that failed — failures always wake immediately.
    Terminal { indices: Vec<usize> },
    /// Every batch (sweep) job is terminal.
    BatchComplete { indices: Vec<usize> },
    /// New suspicious log lines.
    Suspicious { indices: Vec<usize> },
    /// A running job far past its expected runtime.
    Overrun { indices: Vec<usize> },
    /// Periodic "still running" check-in; marks nothing.
    CheckIn,
}

impl WakeCause {
    fn notified_indices(&self) -> &[usize] {
        match self {
            Self::Terminal { indices }
            | Self::BatchComplete { indices }
            | Self::Suspicious { indices }
            | Self::Overrun { indices } => indices,
            Self::CheckIn => &[],
        }
    }
}

/// Decides whether the idle agent should be woken, and why.
fn wake_decision(
    snapshots: &[JobSnapshot],
    idle_for: Duration,
    next_check_in: &mut Option<Duration>,
) -> Option<(String, WakeCause)> {
    // Immediate wakes: terminal `each` jobs, and failed batch members (a
    // completed batch member waits for the rest of its sweep).
    let immediate: Vec<usize> = snapshots
        .iter()
        .enumerate()
        .filter(|(_, snapshot)| {
            snapshot.state.is_terminal()
                && (snapshot.record.wake_policy == WakePolicy::Each
                    || matches!(snapshot.state.phase, JobPhase::Failed))
        })
        .map(|(index, _)| index)
        .collect();
    if let [only] = immediate.as_slice() {
        let snapshot = &snapshots[*only];
        return Some((
            format!(
                "{} reached terminal state {}",
                snapshot.record.target.display(),
                snapshot.state.display()
            ),
            WakeCause::Terminal { indices: immediate },
        ));
    }
    if !immediate.is_empty() {
        return Some((
            format!("{} jobs reached terminal states", immediate.len()),
            WakeCause::Terminal { indices: immediate },
        ));
    }

    let batch: Vec<usize> = snapshots
        .iter()
        .enumerate()
        .filter(|(_, snapshot)| snapshot.record.wake_policy == WakePolicy::Batch)
        .map(|(index, _)| index)
        .collect();
    if !batch.is_empty()
        && batch
            .iter()
            .all(|index| snapshots[*index].state.is_terminal())
    {
        let reason = if batch.len() == 1 {
            format!(
                "{} reached terminal state {}",
                snapshots[batch[0]].record.target.display(),
                snapshots[batch[0]].state.display()
            )
        } else {
            format!("all {} batch jobs reached terminal states", batch.len())
        };
        return Some((reason, WakeCause::BatchComplete { indices: batch }));
    }

    let suspicious: Vec<usize> = snapshots
        .iter()
        .enumerate()
        .filter(|(_, snapshot)| {
            snapshot.has_suspicious_logs()
                && snapshot.record.suspicious_signature.as_deref()
                    != Some(suspicious_signature(snapshot).as_str())
        })
        .map(|(index, _)| index)
        .collect();
    if let Some(first) = suspicious.first() {
        return Some((
            format!(
                "suspicious log lines appeared for {}",
                snapshots[*first].record.target.display()
            ),
            WakeCause::Suspicious {
                indices: suspicious,
            },
        ));
    }

    let overrun: Vec<(usize, f64)> = snapshots
        .iter()
        .enumerate()
        .filter_map(|(index, snapshot)| snapshot.unreported_overrun().map(|ratio| (index, ratio)))
        .collect();
    if let Some((first, ratio)) = overrun.first() {
        return Some((
            format!(
                "{} has been running {ratio:.1}x its expected runtime; check for a hang",
                snapshots[*first].record.target.display()
            ),
            WakeCause::Overrun {
                indices: overrun.iter().map(|(index, _)| *index).collect(),
            },
        ));
    }

    if let Some(interval) = *next_check_in
        && idle_for >= interval
    {
        *next_check_in = Some(interval + interval);
        return Some((
            format!(
                "check-in: jobs still running after {} min idle",
                idle_for.as_secs() / 60
            ),
            WakeCause::CheckIn,
        ));
    }
    None
}

/// One line shown when a session starts with jobs already tracked (a
/// resume), so neither the user nor the model has to remember to ask. States
/// are the last observed ones; `jobs.job_status` re-probes.
pub(crate) fn session_start_briefing(records: &[JobRecord]) -> String {
    let mut parts = Vec::new();
    for record in records.iter().take(8) {
        let state = record
            .latest_state()
            .map(codex_job_monitor::JobState::display)
            .unwrap_or_else(|| "never probed".to_string());
        let label = record
            .label
            .as_deref()
            .map(|label| format!(" ({label})"))
            .unwrap_or_default();
        parts.push(format!(
            "{}{label} last seen {state}",
            record.target.display()
        ));
    }
    let mut message = format!(
        "[jobs] {} tracked job(s) from earlier work: {}",
        records.len(),
        parts.join("; ")
    );
    if records.len() > 8 {
        let _ = write!(message, "; and {} more", records.len() - 8);
    }
    message.push_str(". jobs.job_status re-probes them.");
    message
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

/// Deliberately minimal: the agent already knows the campaign context, the
/// profile prompt tells it to open wake turns with `job_status` and keep
/// `memory.md` current, and the reason line names the job and outcome.
fn wake_message(reason: &str) -> String {
    format!("[job monitor] {reason}. Update memory.md and continue the workflow.")
}

fn should_compact_before_wake(enabled: bool, context_tokens: Option<i64>) -> bool {
    enabled && context_tokens.is_some_and(|tokens| tokens >= COMPACT_BEFORE_WAKE_MIN_TOKENS)
}

/// Best-effort history compaction ahead of a wake-up, mirroring what
/// `Op::Compact` does. Every failure mode degrades to waking on the
/// uncompacted history, never to losing the wake.
async fn compact_history_before_wake(handles: &mut JobWatcherHandles) {
    let Some(session) = handles.session.upgrade() else {
        return;
    };
    let context_tokens = session
        .token_usage_info()
        .await
        .map(|info| info.last_token_usage.tokens_in_context_window());
    if !should_compact_before_wake(handles.compact_before_wake, context_tokens) {
        return;
    }
    {
        // `spawn_task` replaces any running task, so compact only while the
        // agent is still idle: if the user started a turn since the wake
        // decision, waking must steer into it rather than abort it. Marking
        // the status as seen here also keeps the wait below from tripping on
        // stale change notifications.
        let status = handles.agent_status.borrow_and_update();
        if !matches!(*status, AgentStatus::Completed(_)) {
            return;
        }
    }
    let sub_id = format!("job-monitor-compact-{}", Uuid::now_v7());
    let turn_context = session
        .new_turn_with_default_settings(sub_id, Default::default())
        .await;
    session
        .spawn_task(turn_context, Vec::new(), CompactTask)
        .await;
    // The compact task holds its own session reference; drop ours so the
    // wait cannot keep a shutting-down session alive.
    drop(session);

    let compacting = async {
        loop {
            if handles.agent_status.changed().await.is_err() {
                return;
            }
            let running = matches!(
                *handles.agent_status.borrow(),
                AgentStatus::Running | AgentStatus::PendingInit
            );
            if !running {
                return;
            }
        }
    };
    if tokio::time::timeout(COMPACT_WAIT_TIMEOUT, compacting)
        .await
        .is_err()
    {
        tracing::warn!("job watcher compact-before-wake timed out; waking on the full history");
    }
}

#[cfg(test)]
#[path = "job_watcher_tests.rs"]
mod tests;
