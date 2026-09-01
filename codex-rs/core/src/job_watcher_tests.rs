use super::*;
use codex_job_monitor::JobState;
use codex_job_monitor::JobTarget;
use codex_job_monitor::LogTail;
use pretty_assertions::assert_eq;

fn snapshot(state: JobState, suspicious: Vec<String>) -> JobSnapshot {
    snapshot_for_pid(42, state, suspicious)
}

fn snapshot_for_pid(pid: u32, state: JobState, suspicious: Vec<String>) -> JobSnapshot {
    let mut record = JobRecord::new(JobTarget::Pid { pid }, None, Vec::new());
    record.observe(state.clone());
    JobSnapshot {
        record,
        state,
        log_tails: vec![LogTail {
            path: PathBuf::from("/tmp/run.log"),
            file_len: 100,
            tail: String::new(),
            suspicious_lines: suspicious,
        }],
    }
}

fn reason_of(decision: Option<(String, WakeCause)>) -> Option<String> {
    decision.map(|(reason, _)| reason)
}

#[test]
fn terminal_state_wakes_and_check_in_wakes_after_interval() {
    let mut check_in = None;
    let decision = wake_decision(
        &[snapshot(JobState::completed(), Vec::new())],
        Duration::ZERO,
        &mut check_in,
    );
    let (reason, cause) = decision.expect("terminal job wakes");
    assert_eq!(reason, "pid:42 reached terminal state COMPLETED");
    assert_eq!(cause, WakeCause::Terminal { indices: vec![0] });

    let running = [snapshot(JobState::active("RUNNING"), Vec::new())];
    let mut check_in = Some(Duration::from_secs(600));
    assert_eq!(
        reason_of(wake_decision(
            &running,
            Duration::from_secs(60),
            &mut check_in
        )),
        None
    );
    let (reason, cause) = wake_decision(&running, Duration::from_secs(700), &mut check_in)
        .expect("check-in interval elapsed");
    assert_eq!(reason, "check-in: jobs still running after 11 min idle");
    assert_eq!(cause, WakeCause::CheckIn);
    // The next check-in threshold moved forward.
    assert_eq!(check_in, Some(Duration::from_secs(1200)));
}

#[test]
fn batch_jobs_wake_together_but_failures_wake_immediately() {
    let batch = |pid: u32, state: JobState| {
        let mut snapshot = snapshot_for_pid(pid, state, Vec::new());
        snapshot.record.wake_policy = WakePolicy::Batch;
        snapshot
    };

    // One sweep member done, one still running: stay parked.
    let partial = [
        batch(1, JobState::completed()),
        batch(2, JobState::active("RUNNING")),
    ];
    assert_eq!(
        reason_of(wake_decision(&partial, Duration::ZERO, &mut None)),
        None
    );

    // A failed member wakes at once, and only it is marked notified, so the
    // survivors keep being watched.
    let failed = [
        batch(1, JobState::failed("FAILED")),
        batch(2, JobState::active("RUNNING")),
    ];
    let (reason, cause) = wake_decision(&failed, Duration::ZERO, &mut None).expect("failure wakes");
    assert_eq!(reason, "pid:1 reached terminal state FAILED");
    assert_eq!(cause, WakeCause::Terminal { indices: vec![0] });

    // The whole sweep terminal wakes once for all members.
    let done = [
        batch(1, JobState::completed()),
        batch(2, JobState::completed()),
    ];
    let (reason, cause) = wake_decision(&done, Duration::ZERO, &mut None).expect("batch done");
    assert_eq!(reason, "all 2 batch jobs reached terminal states");
    assert_eq!(
        cause,
        WakeCause::BatchComplete {
            indices: vec![0, 1]
        }
    );
}

#[test]
fn suspicious_logs_wake_once_per_signature() {
    let mut fresh = snapshot(
        JobState::active("RUNNING"),
        vec!["energy is NaN at step 2".to_string()],
    );
    let (reason, cause) = wake_decision(
        std::slice::from_ref(&fresh),
        Duration::ZERO,
        &mut /*next_check_in*/ None,
    )
    .expect("new suspicious lines wake");
    assert_eq!(reason, "suspicious log lines appeared for pid:42");
    assert_eq!(cause, WakeCause::Suspicious { indices: vec![0] });

    // Once the signature is recorded, the same lines no longer wake.
    fresh.record.suspicious_signature = Some(suspicious_signature(&fresh));
    assert_eq!(
        reason_of(wake_decision(
            std::slice::from_ref(&fresh),
            Duration::ZERO,
            &mut /*next_check_in*/ None,
        )),
        None
    );
}

#[test]
fn runtime_overrun_wakes_once() {
    let mut overdue = snapshot(JobState::active("RUNNING"), Vec::new());
    overdue.record.expected_runtime_seconds = Some(60);
    // Backdate the RUNNING observation to three expected runtimes ago.
    let started = chrono::Utc::now() - chrono::Duration::seconds(180);
    overdue.record.history[0].at = started;

    let (reason, cause) = wake_decision(std::slice::from_ref(&overdue), Duration::ZERO, &mut None)
        .expect("overrun wakes");
    assert!(
        reason.starts_with("pid:42 has been running 3.")
            && reason.ends_with("its expected runtime; check for a hang"),
        "unexpected overrun reason: {reason}"
    );
    assert_eq!(cause, WakeCause::Overrun { indices: vec![0] });

    // Marked overruns stay silent.
    overdue.record.overrun_notified = true;
    assert_eq!(
        reason_of(wake_decision(
            std::slice::from_ref(&overdue),
            Duration::ZERO,
            &mut None
        )),
        None
    );
}

#[test]
fn wake_message_is_minimal_and_actionable() {
    let message = wake_message("pid:42 reached terminal state EXITED");
    assert_eq!(
        message,
        "[job monitor] pid:42 reached terminal state EXITED. Update memory.md and continue the workflow."
    );
}

#[test]
fn compact_before_wake_gates_on_flag_and_context_size() {
    // Disabled by config: never compacts.
    assert!(!should_compact_before_wake(false, Some(200_000)));
    // No usage recorded yet (nothing to compact).
    assert!(!should_compact_before_wake(true, None));
    // Small histories are left intact: the raw detail beats the summary.
    assert!(!should_compact_before_wake(
        true,
        Some(COMPACT_BEFORE_WAKE_MIN_TOKENS - 1)
    ));
    // Large histories compact so the wake turn starts small.
    assert!(should_compact_before_wake(
        true,
        Some(COMPACT_BEFORE_WAKE_MIN_TOKENS)
    ));
}
