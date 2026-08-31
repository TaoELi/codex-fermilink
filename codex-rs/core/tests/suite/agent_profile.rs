//! Fermilink fork: integration coverage for the `agent_profile` config setting.

use anyhow::Result;
use codex_agent_profiles::SCIENTIFIC_ALGORITHM_PROFILE_ID;
use codex_agent_profiles::SCIENTIFIC_SIMULATIONS_PROFILE_ID;
use codex_agent_profiles::find_agent_profile;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;

const RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";

fn profile_prompt(id: &str) -> &'static str {
    find_agent_profile(id)
        .and_then(|profile| profile.base_instructions)
        .expect("scientific profiles ship base instructions")
}

fn scientific_prompt() -> &'static str {
    profile_prompt(SCIENTIFIC_ALGORITHM_PROFILE_ID)
}

/// The scientific mode's prompt must replace the shipped instructions at the
/// top level of a standard Responses request, even for a model whose catalog
/// entry prefers Responses Lite.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scientific_agent_profile_replaces_instructions_on_standard_responses() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;

    let test = test_codex()
        .with_model_info_override("gpt-5.4", |model_info| {
            model_info.use_responses_lite = true;
        })
        .with_config(|config| {
            config.agent_profile = SCIENTIFIC_ALGORITHM_PROFILE_ID.to_string();
            config.base_instructions = Some(scientific_prompt().to_string());
        })
        .build(&server)
        .await?;

    test.submit_turn("hello").await?;

    let request = response_mock.single_request();
    assert_eq!(request.instructions_text(), scientific_prompt());
    assert_eq!(request.header(RESPONSES_LITE_HEADER), None);
    assert!(!request.has_content_kinds(&["model.base_instructions"]));
    assert!(!request.instructions_text().starts_with("You are Codex"));

    Ok(())
}

/// Profiles with the JobMonitor capability expose the `jobs` tools; the
/// default profile must not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_monitor_tools_are_exposed_only_with_the_capability() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.agent_profile = SCIENTIFIC_SIMULATIONS_PROFILE_ID.to_string();
            config.base_instructions =
                Some(profile_prompt(SCIENTIFIC_SIMULATIONS_PROFILE_ID).to_string());
        })
        .build(&server)
        .await?;
    test.submit_turn("hello").await?;
    let request = response_mock.single_request();
    let tools_json = request.body_json()["tools"].to_string();
    assert!(
        tools_json.contains("job_await"),
        "simulations profile should plan the jobs tools: {tools_json}"
    );

    let default_server = responses::start_mock_server().await;
    let default_mock = responses::mount_sse_once(
        &default_server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;
    let default_test = test_codex().build(&default_server).await?;
    default_test.submit_turn("hello").await?;
    let default_tools = default_mock.single_request().body_json()["tools"].to_string();
    assert!(
        !default_tools.contains("job_await"),
        "default profile must not plan the jobs tools"
    );

    Ok(())
}
