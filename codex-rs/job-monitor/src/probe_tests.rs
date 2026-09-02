use super::*;

#[cfg(unix)]
#[test]
fn own_process_is_alive_and_bogus_pid_is_not() {
    assert!(pid_alive(std::process::id()));
    assert!(!pid_alive(u32::MAX - 1));
}

#[cfg(target_os = "linux")]
#[test]
fn zombie_child_counts_as_exited() {
    // A child that exits before its parent reaps it stays a zombie that
    // still answers `kill(pid, 0)`; the job it ran is nonetheless over.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        unsafe { libc::_exit(0) };
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while pid_alive(pid as u32) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let alive = pid_alive(pid as u32);
    unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
    assert!(!alive, "zombie child must count as exited");
}

/// Needs a live SLURM controller; run explicitly with
/// `cargo test -p codex-job-monitor -- --ignored`.
#[tokio::test]
#[ignore]
async fn live_slurm_probe_follows_a_job_to_a_terminal_state() {
    let submitted = tokio::process::Command::new("sbatch")
        .args([
            "--parsable",
            "--job-name=codex-job-monitor-probe",
            "--output=/dev/null",
            "--wrap=sleep 1",
        ])
        .output()
        .await
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    let Some(submitted) = submitted else {
        panic!("sbatch failed; is SLURM available?");
    };
    let job_id = submitted.split(';').next().unwrap_or_default().to_string();
    assert!(!job_id.is_empty(), "sbatch --parsable returned no job id");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut seen = Vec::new();
    let terminal = loop {
        let probe = probe_slurm(&job_id).await;
        seen.push(probe.clone());
        if let SlurmProbe::State(state) = &probe
            && state.is_terminal()
        {
            break state.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "job {job_id} never reached a terminal state; probes: {seen:?}"
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    };
    // A finished job must be classified from the scheduler, not guessed.
    assert_eq!(terminal, JobState::completed(), "probes: {seen:?}");
    assert!(
        !seen.contains(&SlurmProbe::NotFound) && !seen.contains(&SlurmProbe::Unavailable),
        "a job inside MinJobAge must never look unknown: {seen:?}"
    );
    assert_eq!(probe_slurm("999999999").await, SlurmProbe::NotFound);
}
