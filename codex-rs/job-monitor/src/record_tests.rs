use super::*;
use crate::state::JobState;
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
