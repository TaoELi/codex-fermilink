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
use crate::session::InputQueueActivity;
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
use codex_job_monitor::JobPhase;
use codex_job_monitor::JobRecord;
use codex_job_monitor::JobSnapshot;
use codex_job_monitor::JobTarget;
use codex_job_monitor::OVERRUN_WAKE_RATIO;
use codex_job_monitor::UNKNOWN_CONSECUTIVE_LIMIT;
use codex_job_monitor::WakePolicy;
use codex_job_monitor::next_poll_interval;
use codex_job_monitor::parse_expected_runtime;
use codex_job_monitor::snapshot_job;
use codex_job_monitor::validate_watch_pattern;
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
            "Job to act on: `slurm:<job-id>` for a scheduler job (an array parent ID tracks all of its tasks) or `pid:<pid>` for a detached process (the real process, not a wrapper shell).".to_string(),
        )),
    )
}

fn format_duration_secs(seconds: u64) -> String {
    if seconds < 120 {
        format!("{seconds}s")
    } else if seconds < 7200 {
        format!("{}m", seconds / 60)
    } else {
        format!("{:.1}h", seconds as f64 / 3600.0)
    }
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
        snapshot.state.display(),
        snapshot.state.phase,
    );
    if let Some(expected) = snapshot.record.expected_runtime_seconds
        && !snapshot.state.is_terminal()
        && let Some(started) = snapshot.record.run_started_at()
    {
        let elapsed = (chrono::Utc::now() - started).num_seconds().max(0) as u64;
        let ratio = elapsed as f64 / expected as f64;
        let overdue = if ratio >= OVERRUN_WAKE_RATIO {
            format!(" — {ratio:.1}x over; check for a hang")
        } else {
            String::new()
        };
        let _ = writeln!(
            text,
            "runtime: {} of ~{} expected{overdue}",
            format_duration_secs(elapsed),
            format_duration_secs(expected),
        );
    }
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

/// Persists that this snapshot's outcome has been shown to the agent, so the
/// session job watcher never wakes it again for the same terminal state,
/// suspicious lines, or overrun. Every tool reply that renders a snapshot is
/// a notification: attach, status, and await alike.
async fn mark_reported(store: &Path, snapshot: &JobSnapshot) {
    let overrun = snapshot.unreported_overrun().is_some();
    if !snapshot.state.is_terminal() && !snapshot.has_suspicious_logs() && !overrun {
        return;
    }
    let mut record = snapshot.record.clone();
    if snapshot.state.is_terminal() {
        record.notified_at = Some(chrono::Utc::now());
    }
    if snapshot.has_suspicious_logs() {
        record.suspicious_signature = Some(crate::job_watcher::suspicious_signature(snapshot));
    }
    if overrun {
        record.overrun_notified = true;
    }
    if let Err(err) = record.save(store).await {
        tracing::warn!(%err, "failed to persist job notification state");
    }
}

pub struct JobAttachHandler;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobAttachArgs {
    #[serde(default)]
    job: Option<String>,
    #[serde(default)]
    jobs: Option<Vec<String>>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    log_paths: Option<Vec<String>>,
    #[serde(default)]
    expected_runtime: Option<String>,
    #[serde(default)]
    watch_patterns: Option<Vec<String>>,
    #[serde(default)]
    wake_policy: Option<String>,
}

impl ToolExecutor<ToolInvocation> for JobAttachHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(NAMESPACE, "job_attach")
    }

    fn spec(&self) -> ToolSpec {
        let mut properties = BTreeMap::from([job_field_schema()]);
        properties.insert(
            "jobs".to_string(),
            JsonSchema::array(
                JsonSchema::string(None),
                Some("Attach a whole sweep in one call: several `slurm:<id>`/`pid:<pid>` specs. Defaults them to `batch` waking (failures wake immediately; completions wake when the whole sweep is done).".to_string()),
            ),
        );
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
                Some("Absolute paths of log or output files to tail and scan while awaiting. With `jobs`, pass one path per job (same order) or none.".to_string()),
            ),
        );
        properties.insert(
            "expected_runtime".to_string(),
            JsonSchema::string(Some(
                "Expected wall-clock runtime once running, e.g. `90s`, `45m`, `6h`. A job running far past this is reported once as suspicious (possible hang).".to_string(),
            )),
        );
        properties.insert(
            "watch_patterns".to_string(),
            JsonSchema::array(
                JsonSchema::string(None),
                Some("Extra regexes scanned in log tails alongside the built-in failure patterns (NaN, OOM, crash). Add campaign-specific anomalies worth waking for, e.g. `Fermi level not converged`.".to_string()),
            ),
        );
        properties.insert(
            "wake_policy".to_string(),
            JsonSchema::string(Some(
                "`each` (default for a single job): completion wakes on its own. `batch` (default for a `jobs` sweep): failures wake immediately, completions wake when every batch job is terminal.".to_string(),
            )),
        );
        namespace_tool(ResponsesApiTool {
            name: "job_attach".to_string(),
            description: "Register already-submitted long-running jobs for deterministic monitoring. Submit via the shell first; then attach the SLURM job ID (an array parent ID like `slurm:12345` tracks every task) or the detached process PID, with log paths. For a parameter sweep, attach all jobs in one call via `jobs`. Returns the initial state; a job that is already dead should be debugged, not awaited.".to_string(),
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
                    "job_attach handler received unsupported payload".to_string(),
                ));
            };
            let args: JobAttachArgs = parse_arguments(&arguments)?;
            let specs: Vec<String> = match (&args.job, &args.jobs) {
                (Some(job), None) => vec![job.clone()],
                (None, Some(jobs)) if !jobs.is_empty() => jobs.clone(),
                _ => {
                    return Err(FunctionCallError::RespondToModel(
                        "pass exactly one of `job` (single) or a non-empty `jobs` list (sweep)"
                            .to_string(),
                    ));
                }
            };
            let targets = specs
                .iter()
                .map(|spec| JobTarget::parse(spec))
                .collect::<Result<Vec<_>, _>>()
                .map_err(FunctionCallError::RespondToModel)?;

            // With one job, all log paths belong to it; with a sweep, pair
            // one path per job by position.
            let log_paths_arg = args.log_paths.unwrap_or_default();
            let per_job_logs: Vec<Vec<PathBuf>> = if targets.len() == 1 {
                vec![log_paths_arg.iter().map(PathBuf::from).collect()]
            } else if log_paths_arg.is_empty() {
                vec![Vec::new(); targets.len()]
            } else if log_paths_arg.len() == targets.len() {
                log_paths_arg
                    .iter()
                    .map(|path| vec![PathBuf::from(path)])
                    .collect()
            } else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "with a `jobs` sweep, pass one log path per job in the same order ({} paths for {} jobs)",
                    log_paths_arg.len(),
                    targets.len()
                )));
            };

            let mut watch_patterns = args.watch_patterns.unwrap_or_default();
            for pattern in &watch_patterns {
                validate_watch_pattern(pattern).map_err(FunctionCallError::RespondToModel)?;
            }
            // User-level config patterns join every attach; invalid ones are
            // skipped rather than blocking the tool.
            for pattern in &turn.config.jobs_watch_patterns {
                if validate_watch_pattern(pattern).is_ok() && !watch_patterns.contains(pattern) {
                    watch_patterns.push(pattern.clone());
                }
            }

            let expected_runtime_seconds = args
                .expected_runtime
                .as_deref()
                .map(parse_expected_runtime)
                .transpose()
                .map_err(FunctionCallError::RespondToModel)?;

            let wake_policy = match args.wake_policy.as_deref() {
                None if targets.len() > 1 => WakePolicy::Batch,
                None => WakePolicy::Each,
                Some("each") => WakePolicy::Each,
                Some("batch") => WakePolicy::Batch,
                Some(other) => {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "invalid wake_policy `{other}`; use `each` or `batch`"
                    )));
                }
            };

            let store = store_dir(session.thread_id, &turn.config.codex_home);
            let multi = targets.len() > 1;
            let mut text = String::new();
            let mut any_terminal = false;
            for (index, (target, log_paths)) in targets.into_iter().zip(per_job_logs).enumerate() {
                let label = match (&args.label, multi) {
                    (Some(label), true) => Some(format!("{label} [{}]", index + 1)),
                    (Some(label), false) => Some(label.clone()),
                    (None, _) => None,
                };
                let mut record = JobRecord::new(target, label, log_paths);
                record.wake_policy = wake_policy;
                record.expected_runtime_seconds = expected_runtime_seconds;
                record.watch_patterns = watch_patterns.clone();
                let snapshot = snapshot_job(&store, record).await.map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to persist job record: {err}"
                    ))
                })?;
                if snapshot.state.is_terminal() {
                    any_terminal = true;
                }
                // The attach reply is the notification for whatever it shows:
                // a job that is already dead, or suspicious lines already in
                // its log, must not wake the agent again once it goes idle.
                mark_reported(&store, &snapshot).await;
                let _ = writeln!(text, "Attached {}.", snapshot.record.target.display());
                text.push_str(&format_snapshot(&snapshot));
            }
            if any_terminal {
                text.push_str(
                    "\nA job is already terminal: diagnose or resubmit instead of awaiting.\n",
                );
            } else if multi || wake_policy == WakePolicy::Batch {
                text.push_str(
                    "\nCall jobs.job_await to wait; batch jobs return on first failure, suspicious logs, or when the whole sweep is done.\n",
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
                mark_reported(&store, &snapshot).await;
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
            description: "Wait for attached jobs with deterministic, adaptive polling (no agent turns are spent while waiting). Returns when a job reaches a terminal state (batch/sweep jobs: on first failure or once the whole sweep is done), a suspicious or watched log line appears (NaN, OOM, crash, your watch_patterns), a job runs far past its expected_runtime, the scheduler stops answering, new user input arrives, or the wait budget elapses. If you instead end your turn with jobs running, a deterministic watcher wakes you when they complete.".to_string(),
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
            let mut waiting_on = records
                .iter()
                .take(5)
                .map(|record| record.target.display())
                .collect::<Vec<_>>()
                .join(", ");
            if records.len() > 5 {
                let _ = write!(waiting_on, " and {} more", records.len() - 5);
            }
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
            // `Some` once the wait was cut short, carrying what arrived so the
            // reply can say whether it was the user or an agent message.
            let mut interrupted_by: Option<InputQueueActivity> = pending_activity;
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
                // so long waits stay legible in the UI, and poll quickly
                // again after one: the moments right after a change are when
                // the next one is most likely (a crash just after start, a
                // scheduler outage clearing), and a backed-off interval would
                // otherwise confirm it minutes late.
                let mut transitioned = false;
                for snapshot in &snapshots {
                    let target = snapshot.record.target.display();
                    let token = snapshot.state.token.clone();
                    let previous = last_state_tokens.insert(target.clone(), token.clone());
                    if previous
                        .as_deref()
                        .is_some_and(|previous| previous != token)
                    {
                        transitioned = true;
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
                if transitioned {
                    poll_interval = INITIAL_POLL_INTERVAL;
                }

                // Batch (sweep) awaits return early only for failures or
                // suspicious logs; plain completions return once the whole
                // batch is terminal.
                let batch_mode = snapshots
                    .iter()
                    .all(|snapshot| snapshot.record.wake_policy == WakePolicy::Batch);
                let terminal_stop = if batch_mode {
                    snapshots
                        .iter()
                        .find(|snapshot| matches!(snapshot.state.phase, JobPhase::Failed))
                        .or_else(|| {
                            snapshots
                                .iter()
                                .all(|snapshot| snapshot.state.is_terminal())
                                .then(|| snapshots.first())
                                .flatten()
                        })
                } else {
                    snapshots
                        .iter()
                        .find(|snapshot| snapshot.state.is_terminal())
                };
                if let Some(terminal) = terminal_stop {
                    stop_reason = Some(
                        if batch_mode
                            && snapshots.len() > 1
                            && !matches!(terminal.state.phase, JobPhase::Failed)
                        {
                            format!("all {} batch jobs reached terminal states", snapshots.len())
                        } else {
                            format!(
                                "{} reached terminal state {}",
                                terminal.record.target.display(),
                                terminal.state.display()
                            )
                        },
                    );
                } else if let Some(suspicious) = snapshots
                    .iter()
                    .find(|snapshot| snapshot.has_suspicious_logs())
                {
                    stop_reason = Some(format!(
                        "suspicious log lines appeared for {}",
                        suspicious.record.target.display()
                    ));
                } else if let Some((overdue, ratio)) = snapshots.iter().find_map(|snapshot| {
                    snapshot.unreported_overrun().map(|ratio| (snapshot, ratio))
                }) {
                    stop_reason = Some(format!(
                        "{} has been running {ratio:.1}x its expected runtime; check for a hang",
                        overdue.record.target.display()
                    ));
                } else if consecutive_unknown >= UNKNOWN_CONSECUTIVE_LIMIT {
                    stop_reason = Some(format!(
                        "no usable scheduler answer {consecutive_unknown} polls in a row (sacct/squeue failing or timing out); check the scheduler, then job_await again"
                    ));
                }

                if interrupted_by.is_some() || stop_reason.is_some() {
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
                            interrupted_by = Some(*activity_rx.borrow_and_update());
                        }
                    }
                }
                poll_interval = next_poll_interval(poll_interval);
            }

            session
                .emit_turn_item_completed(turn.as_ref(), await_item)
                .await;

            // The await's own return is the notification for anything
            // terminal, suspicious, or overdue it observed.
            for snapshot in &snapshots {
                mark_reported(&store, snapshot).await;
            }

            let mut text = String::new();
            let reason = match interrupted_by {
                Some(InputQueueActivity::Steer) => "Wait interrupted by new user input.".to_string(),
                Some(InputQueueActivity::Mailbox) => {
                    "Wait interrupted by an incoming agent message; the jobs keep running — handle the message, then job_await again."
                        .to_string()
                }
                None => stop_reason.unwrap_or_else(|| "wait ended".to_string()),
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
