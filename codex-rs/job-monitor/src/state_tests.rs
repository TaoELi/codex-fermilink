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
    assert_eq!(classified.token, "NODE_FAIL");
    assert_eq!(classified.phase, JobPhase::Failed);
    assert_eq!(
        classified.detail.as_deref(),
        Some("1\u{d7}COMPLETED, 1\u{d7}NODE_FAIL, 1\u{d7}RUNNING")
    );

    let classified = classify_states(&["COMPLETED".to_string(), "PENDING".to_string()]);
    assert_eq!(classified.token, "PENDING");
    assert_eq!(classified.phase, JobPhase::Active);

    // A single state carries no counts detail.
    let classified = classify_states(&["COMPLETED".to_string()]);
    assert_eq!(classified, JobState::completed());

    assert_eq!(classify_states(&[]), JobState::unknown());
}

#[test]
fn sacct_output_matches_the_job_but_not_its_steps() {
    let stdout = "123|RUNNING\n123.batch|FAILED\n124|COMPLETED\n";
    // The failed token belongs to a step of job 123 and to job 124's line,
    // not to the requested allocation itself.
    assert_eq!(
        classify_sacct_output(stdout, "123"),
        JobState::active("RUNNING")
    );
    assert_eq!(classify_sacct_output(stdout, "124"), JobState::completed());
    assert_eq!(classify_sacct_output(stdout, "999"), JobState::unknown());
    // A shorter ID is not a prefix match.
    assert_eq!(classify_sacct_output(stdout, "12"), JobState::unknown());
}

#[test]
fn sacct_output_aggregates_array_tasks_under_the_parent_id() {
    let stdout =
        "900_1|COMPLETED\n900_1.batch|COMPLETED\n900_2|RUNNING\n900_[3-5]|PENDING\n901|FAILED\n";
    let state = classify_sacct_output(stdout, "900");
    assert_eq!(state.token, "RUNNING");
    assert_eq!(state.phase, JobPhase::Active);
    assert_eq!(
        state.detail.as_deref(),
        Some("1\u{d7}COMPLETED, 1\u{d7}PENDING, 1\u{d7}RUNNING")
    );

    // A single task spec still matches only itself, steps excluded.
    assert_eq!(
        classify_sacct_output(stdout, "900_2"),
        JobState::active("RUNNING")
    );

    // One failed task outranks the rest of the array.
    let failed = classify_sacct_output("900_1|COMPLETED\n900_2|FAILED\n", "900");
    assert_eq!(failed.phase, JobPhase::Failed);
    assert_eq!(failed.token, "FAILED");

    // All tasks done completes the array.
    let done = classify_sacct_output("900_1|COMPLETED\n900_2|COMPLETED\n", "900");
    assert_eq!(done.phase, JobPhase::Completed);
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
