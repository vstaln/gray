#[cfg(unix)]
use super::*;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
#[test]
fn setsid_failure_is_an_error() {
    // The pre_exec closure maps -1 to Err so a failed setsid can never
    // record a fictitious pgid == pid.
    assert!(check_setsid(-1).is_err());
    assert!(check_setsid(1234).is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn spawn_records_real_process_group() {
    let spawned = spawn("true", &PathBuf::from("/tmp")).expect("sh -c true must spawn");
    assert_eq!(spawned.pgid, spawned.pid as i32);
}
