use super::*;
use codex_job_monitor::JobState;
use codex_job_monitor::JobTarget;
use codex_job_monitor::LogTail;
use pretty_assertions::assert_eq;

fn snapshot(state: JobState, suspicious: Vec<String>) -> JobSnapshot {
    let mut record = JobRecord::new(JobTarget::Pid { pid: 42 }, None, Vec::new());
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

#[test]
fn terminal_state_wakes_and_check_in_wakes_after_interval() {
    let mut check_in = None;
    let reason = wake_reason(
        &[snapshot(JobState::completed(), Vec::new())],
        Duration::ZERO,
        &mut check_in,
    );
    assert_eq!(
        reason.as_deref(),
        Some("pid:42 reached terminal state COMPLETED")
    );

    let running = [snapshot(JobState::active("RUNNING"), Vec::new())];
    let mut check_in = Some(Duration::from_secs(600));
    assert_eq!(
        wake_reason(&running, Duration::from_secs(60), &mut check_in),
        None
    );
    let reason = wake_reason(&running, Duration::from_secs(700), &mut check_in);
    assert_eq!(
        reason.as_deref(),
        Some("check-in: jobs still running after 11 min idle")
    );
    // The next check-in threshold moved forward.
    assert_eq!(check_in, Some(Duration::from_secs(1200)));
}

#[test]
fn suspicious_logs_wake_once_per_signature() {
    let mut fresh = snapshot(
        JobState::active("RUNNING"),
        vec!["energy is NaN at step 2".to_string()],
    );
    let reason = wake_reason(
        std::slice::from_ref(&fresh),
        Duration::ZERO,
        &mut /*next_check_in*/ None,
    );
    assert_eq!(
        reason.as_deref(),
        Some("suspicious log lines appeared for pid:42")
    );

    // Once the signature is recorded, the same lines no longer wake.
    fresh.record.suspicious_signature = Some(suspicious_signature(&fresh));
    assert_eq!(
        wake_reason(
            std::slice::from_ref(&fresh),
            Duration::ZERO,
            &mut /*next_check_in*/ None,
        ),
        None
    );
}

#[test]
fn wake_message_is_bounded_and_actionable() {
    let message = wake_message(
        "pid:42 reached terminal state EXITED",
        &[snapshot(
            JobState {
                token: "EXITED".to_string(),
                phase: codex_job_monitor::JobPhase::Completed,
            },
            Vec::new(),
        )],
    );
    assert!(message.starts_with("[job monitor] pid:42 reached terminal state EXITED"));
    assert!(message.contains("jobs.job_status"));
    assert!(message.contains("memory.md"));
}
