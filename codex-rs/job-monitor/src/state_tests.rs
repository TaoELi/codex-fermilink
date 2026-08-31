use super::*;
use pretty_assertions::assert_eq;

#[test]
fn normalizes_aliases_suffixes_and_case() {
    assert_eq!(normalize_state_token("PD"), Some("PENDING".to_string()));
    assert_eq!(
        normalize_state_token("CANCELLED+"),
        Some("CANCELLED".to_string())
    );
    assert_eq!(
        normalize_state_token("running by user"),
        Some("RUNNING".to_string())
    );
    assert_eq!(
        normalize_state_token("OOM|extra"),
        Some("OUT_OF_MEMORY".to_string())
    );
    assert_eq!(normalize_state_token(""), None);
    assert_eq!(normalize_state_token("NOT_A_STATE"), None);
}

#[test]
fn failure_outranks_active_outranks_completed() {
    let classified = classify_states(&[
        "COMPLETED".to_string(),
        "RUNNING".to_string(),
        "NODE_FAIL".to_string(),
    ]);
    assert_eq!(classified, JobState::failed("NODE_FAIL"));

    let classified = classify_states(&["COMPLETED".to_string(), "PENDING".to_string()]);
    assert_eq!(classified, JobState::active("PENDING"));

    let classified = classify_states(&["COMPLETED".to_string()]);
    assert_eq!(classified, JobState::completed());

    assert_eq!(classify_states(&[]), JobState::unknown());
}

#[test]
fn sacct_output_matches_exact_job_id_only() {
    let stdout = "123|RUNNING\n123.batch|FAILED\n124|COMPLETED\n";
    // The failed token belongs to a step of job 123 and to job 124's line,
    // not to the requested allocation itself.
    assert_eq!(
        classify_sacct_output(stdout, "123"),
        JobState::active("RUNNING")
    );
    assert_eq!(classify_sacct_output(stdout, "124"), JobState::completed());
    assert_eq!(classify_sacct_output(stdout, "999"), JobState::unknown());
}

#[test]
fn squeue_output_classifies_state_lines() {
    assert_eq!(
        classify_squeue_output("PENDING\n"),
        JobState::active("PENDING")
    );
    assert_eq!(classify_squeue_output(""), JobState::unknown());
}

#[test]
fn terminal_states_are_terminal() {
    assert!(JobState::completed().is_terminal());
    assert!(JobState::failed("TIMEOUT").is_terminal());
    assert!(!JobState::active("RUNNING").is_terminal());
    assert!(!JobState::unknown().is_terminal());
}
