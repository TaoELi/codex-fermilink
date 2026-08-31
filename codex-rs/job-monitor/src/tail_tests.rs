use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn tails_are_bounded_and_line_aligned() -> std::io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("run.log");
    let mut contents = String::new();
    for step in 0..600 {
        contents.push_str(&format!("step {step:04} energy -1.0\n"));
    }
    tokio::fs::write(&path, &contents).await?;

    let tail = read_log_tail(&path).await;
    assert_eq!(tail.file_len, contents.len() as u64);
    assert!(tail.tail.len() as u64 <= TAIL_BYTES);
    assert!(tail.tail.starts_with("step "));
    assert!(tail.tail.ends_with("step 0599 energy -1.0\n"));
    assert_eq!(tail.suspicious_lines, Vec::<String>::new());
    Ok(())
}

#[tokio::test]
async fn suspicious_lines_are_detected() -> std::io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("run.log");
    tokio::fs::write(&path, "step 1 ok\nenergy is NaN at step 2\nstep 3 ok\n").await?;

    let tail = read_log_tail(&path).await;
    assert_eq!(
        tail.suspicious_lines,
        vec!["energy is NaN at step 2".to_string()]
    );
    Ok(())
}

#[tokio::test]
async fn missing_log_is_empty_not_an_error() {
    let tail = read_log_tail(std::path::Path::new("/nonexistent/run.log")).await;
    assert_eq!(tail.file_len, 0);
    assert_eq!(tail.tail, "");
    assert_eq!(tail.suspicious_lines, Vec::<String>::new());
}
