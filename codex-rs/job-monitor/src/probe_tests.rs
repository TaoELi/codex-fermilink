use super::*;

#[cfg(unix)]
#[test]
fn own_process_is_alive_and_bogus_pid_is_not() {
    assert!(pid_alive(std::process::id()));
    assert!(!pid_alive(u32::MAX - 1));
}
