//! Durable job records, one JSON file per monitored job.
//!
//! Records live under `$CODEX_HOME/jobs/<thread-id>/` so a job attached in a
//! session survives restarts: after a resume, `job_status` reattaches from
//! the record while the SLURM job keeps running on the cluster.

use crate::state::JobState;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

/// What is being watched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobTarget {
    Slurm { job_id: String },
    Pid { pid: u32 },
}

impl JobTarget {
    /// Parses `"slurm:12345"` or `"pid:4242"`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (kind, value) = spec
            .split_once(':')
            .ok_or_else(|| format!("job spec `{spec}` must look like slurm:<id> or pid:<pid>"))?;
        let value = value.trim();
        match kind.trim() {
            "slurm"
                if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit() || c == '_') =>
            {
                Ok(Self::Slurm {
                    job_id: value.to_string(),
                })
            }
            "pid" => value
                .parse::<u32>()
                .ok()
                .filter(|pid| *pid > 0)
                .map(|pid| Self::Pid { pid })
                .ok_or_else(|| format!("invalid pid in job spec `{spec}`")),
            _ => Err(format!(
                "job spec `{spec}` must look like slurm:<id> or pid:<pid>"
            )),
        }
    }

    pub fn key(&self) -> String {
        match self {
            Self::Slurm { job_id } => format!("slurm-{job_id}"),
            Self::Pid { pid } => format!("pid-{pid}"),
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Slurm { job_id } => format!("slurm:{job_id}"),
            Self::Pid { pid } => format!("pid:{pid}"),
        }
    }
}

/// One observed state, timestamped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateObservation {
    pub at: DateTime<Utc>,
    pub state: JobState,
}

/// When a job's plain completion should wake the idle agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WakePolicy {
    /// Wake or return as soon as this job alone reaches a terminal state.
    #[default]
    Each,
    /// Sweep member: failures and suspicious logs still wake immediately, but
    /// plain completions wait until every batch job is terminal.
    Batch,
}

/// Bounded state history; an array whose tasks finish one by one would
/// otherwise grow a record without limit.
const HISTORY_CAP: usize = 100;

/// A monitored job's durable record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    pub target: JobTarget,
    /// Short human label, e.g. "meep production N=4096".
    #[serde(default)]
    pub label: Option<String>,
    /// Log or output files to tail and scan for events.
    #[serde(default)]
    pub log_paths: Vec<PathBuf>,
    pub attached_at: DateTime<Utc>,
    /// Newest last; consecutive duplicates collapsed.
    #[serde(default)]
    pub history: Vec<StateObservation>,
    /// When the session job watcher woke the agent for this job's terminal
    /// state; a notified job is never woken for again.
    #[serde(default)]
    pub notified_at: Option<DateTime<Utc>>,
    /// Fingerprint of the suspicious log lines already reported, so repeated
    /// idle polls do not re-wake the agent for the same lines.
    #[serde(default)]
    pub suspicious_signature: Option<String>,
    /// Whether this job wakes on its own completion or with its batch.
    #[serde(default)]
    pub wake_policy: WakePolicy,
    /// Expected wall-clock runtime once running; large overruns are reported
    /// once as suspicious instead of waiting out the whole budget.
    #[serde(default)]
    pub expected_runtime_seconds: Option<u64>,
    /// Extra regexes scanned in log tails alongside the built-in failure
    /// patterns, so domain anomalies become wake events.
    #[serde(default)]
    pub watch_patterns: Vec<String>,
    /// Set once the agent has been informed of an expected-runtime overrun.
    #[serde(default)]
    pub overrun_notified: bool,
}

impl JobRecord {
    pub fn new(target: JobTarget, label: Option<String>, log_paths: Vec<PathBuf>) -> Self {
        Self {
            target,
            label,
            log_paths,
            attached_at: Utc::now(),
            history: Vec::new(),
            notified_at: None,
            suspicious_signature: None,
            wake_policy: WakePolicy::default(),
            expected_runtime_seconds: None,
            watch_patterns: Vec::new(),
            overrun_notified: false,
        }
    }

    /// Records an observation, collapsing consecutive identical states.
    pub fn observe(&mut self, state: JobState) {
        if self.history.last().map(|last| &last.state) != Some(&state) {
            self.history.push(StateObservation {
                at: Utc::now(),
                state,
            });
            if self.history.len() > HISTORY_CAP {
                let excess = self.history.len() - HISTORY_CAP;
                self.history.drain(..excess);
            }
        }
    }

    /// When the job was first observed `RUNNING`; scheduler queue time does
    /// not count toward the expected runtime.
    pub fn run_started_at(&self) -> Option<DateTime<Utc>> {
        self.history
            .iter()
            .find(|observation| observation.state.token == "RUNNING")
            .map(|observation| observation.at)
    }

    /// Elapsed runtime as a multiple of the expected runtime, once the job
    /// has been observed running and an expectation was given.
    pub fn runtime_ratio(&self, now: DateTime<Utc>) -> Option<f64> {
        let expected = self
            .expected_runtime_seconds
            .filter(|seconds| *seconds > 0)?;
        let started = self.run_started_at()?;
        let elapsed = (now - started).num_seconds().max(0) as f64;
        Some(elapsed / expected as f64)
    }

    pub fn latest_state(&self) -> Option<&JobState> {
        self.history.last().map(|observation| &observation.state)
    }

    fn file_path(store_dir: &Path, target: &JobTarget) -> PathBuf {
        store_dir.join(format!("{}.json", target.key()))
    }

    pub async fn save(&self, store_dir: &Path) -> std::io::Result<()> {
        tokio::fs::create_dir_all(store_dir).await?;
        let contents = serde_json::to_vec_pretty(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        tokio::fs::write(Self::file_path(store_dir, &self.target), contents).await
    }

    pub async fn load(store_dir: &Path, target: &JobTarget) -> std::io::Result<Self> {
        let contents = tokio::fs::read(Self::file_path(store_dir, target)).await?;
        serde_json::from_slice(&contents)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
    }

    /// Loads every record in the store, newest attachment first.
    pub async fn load_all(store_dir: &Path) -> std::io::Result<Vec<Self>> {
        let mut records = Vec::new();
        let mut entries = match tokio::fs::read_dir(store_dir).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(records),
            Err(err) => return Err(err),
        };
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let contents = tokio::fs::read(entry.path()).await?;
            if let Ok(record) = serde_json::from_slice::<Self>(&contents) {
                records.push(record);
            }
        }
        records.sort_by_key(|record| std::cmp::Reverse(record.attached_at));
        Ok(records)
    }
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
