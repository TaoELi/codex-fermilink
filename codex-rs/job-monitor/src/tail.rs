//! Bounded log tails and suspicious-event scanning.

use regex_lite::Regex;
use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;

/// Per-file tail cap; keeps tool output bounded regardless of log size.
pub const TAIL_BYTES: u64 = 4096;

/// Log lines matching these are worth waking the agent for: numerical
/// breakdown, crashes, scheduler kills. Case-insensitive.
static SUSPICIOUS_EVENT: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used)]
    Regex::new(
        r"(?i)\bnan\b|\binf\b|diverg|not converged|convergence fail|traceback|segmentation fault|segfault|out[- ]of[- ]memory|\boom\b|killed|error|fatal|assert",
    )
    .expect("suspicious-event regex is valid")
});

/// The tail of one log file plus any suspicious lines inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogTail {
    pub path: PathBuf,
    /// Total file length at read time; lets callers detect growth.
    pub file_len: u64,
    pub tail: String,
    pub suspicious_lines: Vec<String>,
}

/// Reads the last [`TAIL_BYTES`] of `path`, resynchronized to a line start,
/// and scans it for suspicious lines. Missing files yield an empty tail so a
/// job that has not created its log yet is not an error.
pub async fn read_log_tail(path: &Path) -> LogTail {
    let (file_len, tail) = match read_tail_bytes(path).await {
        Ok(result) => result,
        Err(_) => (0, String::new()),
    };
    let suspicious_lines = tail
        .lines()
        .filter(|line| SUSPICIOUS_EVENT.is_match(line))
        .map(str::to_owned)
        .collect();
    LogTail {
        path: path.to_path_buf(),
        file_len,
        tail,
        suspicious_lines,
    }
}

async fn read_tail_bytes(path: &Path) -> std::io::Result<(u64, String)> {
    let mut file = tokio::fs::File::open(path).await?;
    let file_len = file.metadata().await?.len();
    let start = file_len.saturating_sub(TAIL_BYTES);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut buffer = Vec::with_capacity(TAIL_BYTES as usize);
    file.read_to_end(&mut buffer).await?;
    let mut text = String::from_utf8_lossy(&buffer).into_owned();
    // Drop the first, likely partial, line when reading from mid-file.
    if start > 0
        && let Some(newline) = text.find('\n')
    {
        text.drain(..=newline);
    }
    Ok((file_len, text))
}

#[cfg(test)]
#[path = "tail_tests.rs"]
mod tests;
