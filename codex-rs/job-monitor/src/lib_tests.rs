use super::*;
use pretty_assertions::assert_eq;

#[test]
fn poll_interval_backs_off_to_ceiling() {
    let mut interval = INITIAL_POLL_INTERVAL;
    let mut steps = 0;
    while interval < MAX_POLL_INTERVAL {
        interval = next_poll_interval(interval);
        steps += 1;
        assert!(steps < 32, "backoff must reach the ceiling");
    }
    assert_eq!(next_poll_interval(MAX_POLL_INTERVAL), MAX_POLL_INTERVAL);
}

#[test]
fn expected_runtime_specs_parse_to_seconds() {
    assert_eq!(parse_expected_runtime("90"), Ok(90));
    assert_eq!(parse_expected_runtime("90s"), Ok(90));
    assert_eq!(parse_expected_runtime("45m"), Ok(2700));
    assert_eq!(parse_expected_runtime("6h"), Ok(21600));
    assert_eq!(parse_expected_runtime("2d"), Ok(172800));
    assert_eq!(parse_expected_runtime(" 6H "), Ok(21600));
    assert!(parse_expected_runtime("0h").is_err());
    assert!(parse_expected_runtime("6 hours").is_err());
    assert!(parse_expected_runtime("-1h").is_err());
    assert!(parse_expected_runtime("").is_err());
}

#[tokio::test]
async fn snapshot_scans_logs_with_the_record_watch_patterns() -> std::io::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = dir.path();
    let log = store.join("run.log");
    tokio::fs::write(&log, "phase flip detected in cell 3\nstep 2 ok\n").await?;
    let mut record = JobRecord::new(JobTarget::Pid { pid: u32::MAX - 1 }, None, vec![log]);
    record.watch_patterns = vec!["phase flip".to_string()];
    let snapshot = snapshot_job(store, record).await?;
    assert!(snapshot.has_suspicious_logs());
    assert_eq!(
        snapshot.log_tails[0].suspicious_lines,
        vec!["phase flip detected in cell 3".to_string()]
    );
    Ok(())
}

#[tokio::test]
async fn snapshot_of_dead_pid_is_terminal_and_persisted() -> std::io::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = dir.path();
    // A PID from the unreachable end of the space; if it happens to exist the
    // assertion on phase still holds only for the dead case, so pick the max.
    let record = JobRecord::new(JobTarget::Pid { pid: u32::MAX - 1 }, None, Vec::new());
    let snapshot = snapshot_job(store, record).await?;
    assert_eq!(snapshot.state.token, "EXITED");
    assert!(snapshot.state.is_terminal());

    let reloaded = JobRecord::load(store, &JobTarget::Pid { pid: u32::MAX - 1 }).await?;
    assert_eq!(reloaded.latest_state(), Some(&snapshot.state));
    Ok(())
}

#[test]
fn vanished_job_seen_running_has_exited() {
    let mut record = JobRecord::new(
        JobTarget::Slurm {
            job_id: "3044".to_string(),
        },
        None,
        Vec::new(),
    );
    record.observe(JobState::active("RUNNING"));
    let state = resolve_slurm_probe(SlurmProbe::NotFound, &record);
    assert_eq!(
        (state.token.as_str(), state.phase, state.detail.is_some()),
        ("EXITED", JobPhase::Completed, true)
    );
    assert!(state.is_terminal());
}

#[test]
fn vanished_job_never_seen_active_is_not_found() {
    let mut record = JobRecord::new(
        JobTarget::Slurm {
            job_id: "3043".to_string(),
        },
        None,
        Vec::new(),
    );
    let fresh = resolve_slurm_probe(SlurmProbe::NotFound, &record);
    assert_eq!(
        (fresh.token.as_str(), fresh.phase),
        ("NOT_FOUND", JobPhase::Failed)
    );
    assert!(fresh.is_terminal());

    // An earlier UNKNOWN (scheduler unreachable) is not evidence the job ran.
    record.observe(JobState::unknown());
    let after_unknown = resolve_slurm_probe(SlurmProbe::NotFound, &record);
    assert_eq!(after_unknown.token, "NOT_FOUND");
}

#[test]
fn scheduler_answers_pass_through_resolution() {
    let record = JobRecord::new(
        JobTarget::Slurm {
            job_id: "77".to_string(),
        },
        None,
        Vec::new(),
    );
    assert_eq!(
        resolve_slurm_probe(SlurmProbe::State(JobState::active("PENDING")), &record),
        JobState::active("PENDING")
    );
    assert_eq!(
        resolve_slurm_probe(SlurmProbe::State(JobState::failed("TIMEOUT")), &record),
        JobState::failed("TIMEOUT")
    );
    assert_eq!(
        resolve_slurm_probe(SlurmProbe::Unavailable, &record),
        JobState::unknown()
    );
}
