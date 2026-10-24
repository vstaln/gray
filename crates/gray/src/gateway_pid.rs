use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct PidRecord {
    pub pid: u32,
    #[serde(default)]
    pub start_time: Option<u64>,
}

fn record_path(home: &Path) -> PathBuf {
    home.join("gateway.pid")
}

#[cfg(unix)]
fn proc_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(unix))]
fn proc_start_time(_pid: u32) -> Option<u64> {
    None
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    unsafe {
        libc::kill(pid as libc::pid_t, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;
    if pid == 0 {
        return false;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE
    }
}

#[cfg(not(any(unix, windows)))]
fn pid_alive(pid: u32) -> bool {
    pid != 0
}

fn alive(rec: &PidRecord) -> bool {
    if !pid_alive(rec.pid) {
        return false;
    }
    match (rec.start_time, proc_start_time(rec.pid)) {
        (Some(claimed), Some(now)) => claimed == now,
        _ => true,
    }
}

pub(crate) fn running(home: &Path) -> Option<PidRecord> {
    let body = std::fs::read_to_string(record_path(home)).ok()?;
    serde_json::from_str::<PidRecord>(&body).ok().filter(alive)
}
