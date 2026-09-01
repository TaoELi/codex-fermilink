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
