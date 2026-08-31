//! Deterministic long-running job tools (fermilink fork).
//!
//! `job_attach` registers a SLURM job or detached PID that the agent
//! submitted through the normal shell; `job_await` parks the turn while
//! non-agent code polls scheduler state and logs with adaptive backoff,
//! resuming the agent only on terminal states, suspicious log events, new
//! user input, or a wait budget; `job_status` reports instantly. Records are
//! durable under `$CODEX_HOME/jobs/<thread-id>/`, so jobs survive session
//! restarts. Enabled by agent profiles with the `JobMonitor` capability.

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_extension_items::ExtensionItem;
use codex_extension_items::sleep::SleepItem;
use codex_job_monitor::INITIAL_POLL_INTERVAL;
use codex_job_monitor::JobRecord;
use codex_job_monitor::JobSnapshot;
use codex_job_monitor::JobTarget;
use codex_job_monitor::MAX_POLL_INTERVAL;
use codex_job_monitor::UNKNOWN_CONSECUTIVE_LIMIT;
use codex_job_monitor::next_poll_interval;
use codex_job_monitor::snapshot_job;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolExposure;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

const NAMESPACE: &str = "jobs";
const DEFAULT_AWAIT_BUDGET_SECONDS: u64 = 1800;
const MAX_AWAIT_BUDGET_SECONDS: u64 = 12 * 60 * 60;

fn store_dir(thread_id: codex_protocol::ThreadId, codex_home: &Path) -> PathBuf {
    codex_home.join("jobs").join(thread_id.to_string())
}

fn namespace_tool(tool: ResponsesApiTool) -> ToolSpec {
    ToolSpec::Namespace(ResponsesApiNamespace {
        name: NAMESPACE.to_string(),
        description: "Deterministic monitoring of long-running jobs (SLURM or detached PIDs); submit through the shell, then attach and await here instead of polling.".to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(tool)],
    })
}

fn job_field_schema() -> (String, JsonSchema) {
    (
        "job".to_string(),
        JsonSchema::string(Some(
            "Job to act on: `slurm:<job-id>` for a scheduler job or `pid:<pid>` for a detached process (the real process, not a wrapper shell).".to_string(),
        )),
    )
}

fn format_snapshot(snapshot: &JobSnapshot) -> String {
    let mut text = String::new();
    let label = snapshot
        .record
        .label
        .as_deref()
        .map(|label| format!(" ({label})"))
        .unwrap_or_default();
    let _ = writeln!(
        text,
        "{}{label}: {} [{:?}]",
        snapshot.record.target.display(),
        snapshot.state.token,
        snapshot.state.phase,
    );
    let history: Vec<String> = snapshot
        .record
        .history
        .iter()
        .rev()
        .take(6)
        .map(|observation| {
            format!(
                "  {} {}",
                observation.at.format("%Y-%m-%dT%H:%M:%SZ"),
                observation.state.token
            )
        })
        .collect();
    if !history.is_empty() {
        let _ = writeln!(text, "state history (newest first):");
        for line in history {
            let _ = writeln!(text, "{line}");
        }
    }
    for tail in &snapshot.log_tails {
        if !tail.suspicious_lines.is_empty() {
            let _ = writeln!(text, "suspicious lines in {}:", tail.path.display());
            for line in tail.suspicious_lines.iter().take(10) {
                let _ = writeln!(text, "  {line}");
            }
        }
    }
    if let Some(tail) = snapshot.log_tails.iter().find(|tail| !tail.tail.is_empty()) {
        let _ = writeln!(
            text,
            "log tail of {} ({} bytes total):\n{}",
            tail.path.display(),
            tail.file_len,
            tail.tail
        );
    }
    text
}

pub struct JobAttachHandler;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobAttachArgs {
    job: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    log_paths: Option<Vec<String>>,
}

impl ToolExecutor<ToolInvocation> for JobAttachHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(NAMESPACE, "job_attach")
    }

    fn spec(&self) -> ToolSpec {
        let mut properties = BTreeMap::from([job_field_schema()]);
        properties.insert(
            "label".to_string(),
            JsonSchema::string(Some(
                "Short human label for the job, e.g. `meep production N=4096`.".to_string(),
            )),
        );
        properties.insert(
            "log_paths".to_string(),
            JsonSchema::array(
                JsonSchema::string(None),
                Some("Absolute paths of log or output files to tail and scan for failures while awaiting.".to_string()),
            ),
        );
        namespace_tool(ResponsesApiTool {
            name: "job_attach".to_string(),
            description: "Register an already-submitted long-running job for deterministic monitoring. Submit via the shell first; then attach the SLURM job ID or the detached process PID together with its log paths. Returns the initial state; a job that is already dead should be debugged, not awaited.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["job".to_string()]),
                /*additional_properties*/ Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::DirectModelOnly
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolInvocation {
                session,
                turn,
                payload,
                ..
            } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(
                    "job_attach handler received unsupported payload".to_string(),
                ));
            };
            let args: JobAttachArgs = parse_arguments(&arguments)?;
            let target = JobTarget::parse(&args.job).map_err(FunctionCallError::RespondToModel)?;
            let log_paths = args
                .log_paths
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect();
            let store = store_dir(session.thread_id, &turn.config.codex_home);
            let record = JobRecord::new(target, args.label, log_paths);
            let snapshot = snapshot_job(&store, record).await.map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to persist job record: {err}"))
            })?;
            if snapshot.state.is_terminal() {
                // The attach reply is the notification: keep the session job
                // watcher from waking the agent again for this dead job.
                let mut record = snapshot.record.clone();
                record.notified_at = Some(chrono::Utc::now());
                if let Err(err) = record.save(&store).await {
                    tracing::warn!(%err, "failed to persist attach-time notification state");
                }
            }
            let mut text = format!("Attached {}.\n", snapshot.record.target.display());
            text.push_str(&format_snapshot(&snapshot));
            if snapshot.state.is_terminal() {
                text.push_str(
                    "\nThe job is already terminal: diagnose or resubmit instead of awaiting.\n",
                );
            } else {
                text.push_str("\nCall jobs.job_await to wait for completion.\n");
            }
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                text,
                /*success*/ Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for JobAttachHandler {}

pub struct JobStatusHandler;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobStatusArgs {
    #[serde(default)]
    job: Option<String>,
}

impl ToolExecutor<ToolInvocation> for JobStatusHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(NAMESPACE, "job_status")
    }

    fn spec(&self) -> ToolSpec {
        let mut properties = BTreeMap::new();
        properties.insert(
            "job".to_string(),
            JsonSchema::string(Some(
                "Optional `slurm:<id>` or `pid:<pid>` to check one job; omit to list every job attached in this thread.".to_string(),
            )),
        );
        namespace_tool(ResponsesApiTool {
            name: "job_status".to_string(),
            description: "Instantly report the current state, state history, and bounded log tail of attached jobs, without waiting. Use after a resume to reattach to work that ran while the session was closed.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                /*required*/ None,
                /*additional_properties*/ Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::DirectModelOnly
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolInvocation {
                session,
                turn,
                payload,
                ..
            } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(
                    "job_status handler received unsupported payload".to_string(),
                ));
            };
            let args: JobStatusArgs = parse_arguments(&arguments)?;
            let store = store_dir(session.thread_id, &turn.config.codex_home);
            let records = match &args.job {
                Some(spec) => {
                    let target =
                        JobTarget::parse(spec).map_err(FunctionCallError::RespondToModel)?;
                    vec![JobRecord::load(&store, &target).await.map_err(|err| {
                        FunctionCallError::RespondToModel(format!(
                            "no record for {spec} in this thread: {err}"
                        ))
                    })?]
                }
                None => JobRecord::load_all(&store).await.map_err(|err| {
                    FunctionCallError::RespondToModel(format!("failed to list jobs: {err}"))
                })?,
            };
            if records.is_empty() {
                return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                    "No jobs are attached in this thread. Submit via the shell, then jobs.job_attach.".to_string(),
                    /*success*/ Some(true),
                )));
            }
            let mut text = String::new();
            for record in records {
                let snapshot = snapshot_job(&store, record).await.map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to persist job record: {err}"
                    ))
                })?;
                text.push_str(&format_snapshot(&snapshot));
                text.push('\n');
            }
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                text,
                /*success*/ Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for JobStatusHandler {}

pub struct JobAwaitHandler;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobAwaitArgs {
    #[serde(default)]
    job: Option<String>,
    #[serde(default)]
    max_wait_seconds: Option<u64>,
}

impl ToolExecutor<ToolInvocation> for JobAwaitHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(NAMESPACE, "job_await")
    }

    fn spec(&self) -> ToolSpec {
        let mut properties = BTreeMap::new();
        properties.insert(
            "job".to_string(),
            JsonSchema::string(Some(
                "Optional `slurm:<id>` or `pid:<pid>` to await one job; omit to await every attached job that is still active.".to_string(),
            )),
        );
        properties.insert(
            "max_wait_seconds".to_string(),
            JsonSchema::number(Some(format!(
                "Wait budget for this call in seconds (default {DEFAULT_AWAIT_BUDGET_SECONDS}, max {MAX_AWAIT_BUDGET_SECONDS}). On expiry the call returns `still running`; simply call job_await again."
            ))),
        );
        namespace_tool(ResponsesApiTool {
            name: "job_await".to_string(),
            description: "Wait for attached jobs with deterministic, adaptive polling (no agent turns are spent while waiting). Returns when a job reaches a terminal state, a suspicious log line appears (NaN, divergence, OOM, crash), the scheduler stops answering, new user input arrives, or the wait budget elapses. If you instead end your turn with jobs running, a deterministic watcher wakes you when they complete.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                /*required*/ None,
                /*additional_properties*/ Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::DirectModelOnly
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolInvocation {
                session,
                turn,
                call_id,
                payload,
                ..
            } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(
                    "job_await handler received unsupported payload".to_string(),
                ));
            };
            let args: JobAwaitArgs = parse_arguments(&arguments)?;
            let budget = Duration::from_secs(
                args.max_wait_seconds
                    .unwrap_or(DEFAULT_AWAIT_BUDGET_SECONDS)
                    .clamp(1, MAX_AWAIT_BUDGET_SECONDS),
            );
            let store = store_dir(session.thread_id, &turn.config.codex_home);
            let mut records = match &args.job {
                Some(spec) => {
                    let target =
                        JobTarget::parse(spec).map_err(FunctionCallError::RespondToModel)?;
                    vec![JobRecord::load(&store, &target).await.map_err(|err| {
                        FunctionCallError::RespondToModel(format!(
                            "no record for {spec} in this thread; jobs.job_attach it first: {err}"
                        ))
                    })?]
                }
                None => JobRecord::load_all(&store).await.map_err(|err| {
                    FunctionCallError::RespondToModel(format!("failed to list jobs: {err}"))
                })?,
            };
            if records.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "no jobs are attached in this thread; submit via the shell, then jobs.job_attach".to_string(),
                ));
            }

            let turn_state = session
                .input_queue
                .turn_state_for_sub_id(&session.active_turn, &turn.sub_id)
                .await;
            let (mut activity_rx, pending_activity) = session
                .input_queue
                .subscribe_activity(turn_state.as_deref())
                .await;

            // Visibility while parked: reuse the interruptible-sleep item so
            // the UI shows this turn is deliberately waiting on jobs, with
            // the wait budget as its duration.
            let await_item = TurnItem::Extension(ExtensionItem::Sleep(SleepItem {
                id: call_id,
                duration_ms: budget.as_millis().min(u128::from(u64::MAX)) as u64,
            }));
            session
                .emit_turn_item_started(turn.as_ref(), &await_item)
                .await;
            let waiting_on = records
                .iter()
                .map(|record| record.target.display())
                .collect::<Vec<_>>()
                .join(", ");
            session
                .send_event(
                    turn.as_ref(),
                    EventMsg::Warning(WarningEvent {
                        message: format!(
                            "[jobs] awaiting {waiting_on}; budget {}s, polling {}s\u{2192}{}s; new input interrupts the wait",
                            budget.as_secs(),
                            INITIAL_POLL_INTERVAL.as_secs(),
                            codex_job_monitor::MAX_POLL_INTERVAL.as_secs(),
                        ),
                    }),
                )
                .await;

            let started = Instant::now();
            let mut poll_interval = INITIAL_POLL_INTERVAL;
            let mut consecutive_unknown: u32 = 0;
            let mut interrupted = pending_activity.is_some();
            let mut stop_reason: Option<String> = None;
            let mut snapshots: Vec<JobSnapshot>;
            let mut last_state_tokens: BTreeMap<String, String> = records
                .iter()
                .filter_map(|record| {
                    record
                        .latest_state()
                        .map(|state| (record.target.display(), state.token.clone()))
                })
                .collect();

            loop {
                let mut next_records = Vec::with_capacity(records.len());
                snapshots = Vec::with_capacity(records.len());
                for record in records {
                    let snapshot = snapshot_job(&store, record).await.map_err(|err| {
                        FunctionCallError::RespondToModel(format!(
                            "failed to persist job record: {err}"
                        ))
                    })?;
                    next_records.push(snapshot.record.clone());
                    snapshots.push(snapshot);
                }
                records = next_records;

                if snapshots.iter().all(|snapshot| {
                    matches!(snapshot.state.phase, codex_job_monitor::JobPhase::Unknown)
                }) {
                    consecutive_unknown += 1;
                } else {
                    consecutive_unknown = 0;
                }

                // Surface state transitions (e.g. PENDING -> RUNNING) live,
                // so long waits stay legible in the UI.
                for snapshot in &snapshots {
                    let target = snapshot.record.target.display();
                    let token = snapshot.state.token.clone();
                    let previous = last_state_tokens.insert(target.clone(), token.clone());
                    if previous
                        .as_deref()
                        .is_some_and(|previous| previous != token)
                    {
                        session
                            .send_event(
                                turn.as_ref(),
                                EventMsg::Warning(WarningEvent {
                                    message: format!(
                                        "[jobs] {target} {} \u{2192} {token} ({}m elapsed)",
                                        previous.unwrap_or_default(),
                                        started.elapsed().as_secs() / 60,
                                    ),
                                }),
                            )
                            .await;
                    }
                }

                if let Some(terminal) = snapshots
                    .iter()
                    .find(|snapshot| snapshot.state.is_terminal())
                {
                    stop_reason = Some(format!(
                        "{} reached terminal state {}",
                        terminal.record.target.display(),
                        terminal.state.token
                    ));
                } else if let Some(suspicious) = snapshots
                    .iter()
                    .find(|snapshot| snapshot.has_suspicious_logs())
                {
                    stop_reason = Some(format!(
                        "suspicious log lines appeared for {}",
                        suspicious.record.target.display()
                    ));
                } else if consecutive_unknown >= UNKNOWN_CONSECUTIVE_LIMIT {
                    stop_reason = Some(format!(
                        "scheduler state unknown {consecutive_unknown} polls in a row; check sacct/squeue availability and the job ID"
                    ));
                }

                if interrupted || stop_reason.is_some() {
                    break;
                }
                let elapsed = started.elapsed();
                if elapsed >= budget {
                    stop_reason = Some(format!(
                        "wait budget of {}s elapsed; jobs are still running — call job_await again",
                        budget.as_secs()
                    ));
                    break;
                }
                let sleep_for = poll_interval.min(budget - elapsed);
                let sleep = session
                    .services
                    .time_provider
                    .sleep(session.thread_id, sleep_for);
                tokio::pin!(sleep);
                tokio::select! {
                    result = &mut sleep => {
                        result.map_err(|err| {
                            FunctionCallError::Fatal(format!("failed to sleep between polls: {err:#}"))
                        })?;
                    }
                    result = activity_rx.changed() => {
                        if result.is_ok() {
                            interrupted = true;
                        }
                    }
                }
                poll_interval = next_poll_interval(poll_interval);
            }

            session
                .emit_turn_item_completed(turn.as_ref(), await_item)
                .await;

            // The await's own return is the notification for anything
            // terminal or suspicious it observed; keep the session job
            // watcher from waking the agent again for the same outcome.
            for snapshot in &snapshots {
                if !snapshot.state.is_terminal() && !snapshot.has_suspicious_logs() {
                    continue;
                }
                let mut record = snapshot.record.clone();
                if snapshot.state.is_terminal() {
                    record.notified_at = Some(chrono::Utc::now());
                }
                if snapshot.has_suspicious_logs() {
                    record.suspicious_signature =
                        Some(crate::job_watcher::suspicious_signature(snapshot));
                }
                if let Err(err) = record.save(&store).await {
                    tracing::warn!(%err, "failed to persist await notification state");
                }
            }

            let mut text = String::new();
            let reason = if interrupted {
                "Wait interrupted by new input.".to_string()
            } else {
                stop_reason.unwrap_or_else(|| "wait ended".to_string())
            };
            let _ = writeln!(
                text,
                "{reason}\nWaited {:.0}s.",
                started.elapsed().as_secs_f64()
            );
            for snapshot in &snapshots {
                text.push_str(&format_snapshot(snapshot));
                text.push('\n');
            }
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                text,
                /*success*/ Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for JobAwaitHandler {}
