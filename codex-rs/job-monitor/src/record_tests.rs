use super::*;
use crate::state::JobState;
use chrono::Utc;
use pretty_assertions::assert_eq;

#[test]
fn parses_job_specs() {
    assert_eq!(
        JobTarget::parse("slurm:12345"),
        Ok(JobTarget::Slurm {
            job_id: "12345".to_string()
        })
    );
    assert_eq!(
        JobTarget::parse("pid:4242"),
        Ok(JobTarget::Pid { pid: 4242 })
    );
    assert!(JobTarget::parse("12345").is_err());
    assert!(JobTarget::parse("pid:0").is_err());
    assert!(JobTarget::parse("pbs:1").is_err());
    assert!(JobTarget::parse("slurm:not a job").is_err());
}

#[test]
fn observe_collapses_consecutive_duplicates() {
    let mut record = JobRecord::new(
        JobTarget::Pid { pid: 7 },
        Some("acquisition".to_string()),
        Vec::new(),
    );
    record.observe(JobState::active("RUNNING"));
    record.observe(JobState::active("RUNNING"));
    record.observe(JobState::completed());
    assert_eq!(record.history.len(), 2);
    assert_eq!(record.latest_state(), Some(&JobState::completed()));
}

#[test]
fn legacy_records_deserialize_with_defaults() {
    let json = r#"{"target":{"kind":"pid","pid":7},"attached_at":"2026-01-01T00:00:00Z"}"#;
    let record: JobRecord = serde_json::from_str(json).expect("legacy record loads");
    assert_eq!(record.wake_policy, WakePolicy::Each);
    assert_eq!(record.expected_runtime_seconds, None);
    assert!(record.watch_patterns.is_empty());
    assert!(!record.overrun_notified);
}

#[test]
fn runtime_ratio_counts_from_the_first_running_observation() {
    let mut record = JobRecord::new(JobTarget::Pid { pid: 7 }, None, Vec::new());
    record.expected_runtime_seconds = Some(100);
    // Not yet observed running: no ratio.
    assert_eq!(record.runtime_ratio(Utc::now()), None);
    // Queue time does not count.
    record.observe(JobState::active("PENDING"));
    assert_eq!(record.runtime_ratio(Utc::now()), None);
    record.observe(JobState::active("RUNNING"));
    record.history.last_mut().expect("running observation").at =
        Utc::now() - chrono::Duration::seconds(250);
    let ratio = record
        .runtime_ratio(Utc::now())
        .expect("ratio once running");
    assert!((2.4..2.7).contains(&ratio), "unexpected ratio {ratio}");
}

#[test]
fn history_is_capped() {
    let mut record = JobRecord::new(JobTarget::Pid { pid: 7 }, None, Vec::new());
    for step in 0..300 {
        record.observe(JobState::active(&format!("S{step}")));
    }
    assert_eq!(record.history.len(), 100);
    // The newest observations survive.
    assert_eq!(record.latest_state(), Some(&JobState::active("S299")));
}

#[tokio::test]
async fn records_round_trip_and_list() -> std::io::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = dir.path();
    let mut record = JobRecord::new(
        JobTarget::Slurm {
            job_id: "31415".to_string(),
        },
        None,
        vec![store.join("run.log")],
    );
    record.observe(JobState::active("PENDING"));
    record.save(store).await?;

    let loaded = JobRecord::load(
        store,
        &JobTarget::Slurm {
            job_id: "31415".to_string(),
        },
    )
    .await?;
    assert_eq!(loaded, record);

    let all = JobRecord::load_all(store).await?;
    assert_eq!(all, vec![record]);

    assert_eq!(JobRecord::load_all(&store.join("missing")).await?, vec![]);
    Ok(())
}
