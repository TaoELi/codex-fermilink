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
