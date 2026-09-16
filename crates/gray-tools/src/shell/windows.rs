//! Native Windows shell ownership. A PID is not a process tree on Windows.
//!
//! Create the shell suspended, attach it to an unnamed Job Object, then resume
//! its initial thread. Assigning a running shell races its first descendant.
//! The job is non-inheritable and forbids breakaway; closing our last handle
//! also cleans up descendants on errors, dropped futures, and Gray shutdown.

use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;
use std::ptr::null;

use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

pub struct Job(OwnedHandle);

impl Job {
    fn new() -> io::Result<Self> {
        // SAFETY: null security/name creates a private, non-inheritable job.
        let raw = unsafe { CreateJobObjectW(null(), null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful CreateJobObjectW transfers an owned handle to us.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
        // SAFETY: Win32 accepts a zeroed structure with its selected flags set.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the live handle and structure pointer/size match the API.
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    pub fn terminate(&self) -> io::Result<()> {
        // No POSIX SIGTERM emulation: terminate precisely the job we own.
        // SAFETY: our OwnedHandle remains alive for the entire call.
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

pub fn spawn_owned(cmd: &mut Command) -> io::Result<(Child, Job)> {
    let job = Job::new()?;
    cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
    let mut child = cmd.spawn()?;
    let attach = (|| {
        let handle = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("no shell handle"))?;
        // SAFETY: both handles are live and the child cannot execute yet.
        if unsafe { AssignProcessToJobObject(job.0.as_raw_handle(), handle) } == 0 {
            return Err(io::Error::last_os_error());
        }
        resume_initial_thread(child.id().ok_or_else(|| io::Error::other("no shell PID"))?)
    })();
    if let Err(error) = attach {
        // Still suspended if attachment failed: no descendants can have escaped.
        // Kill the root explicitly too, because an unassigned child isn't in job.
        child.start_kill().map_err(|kill| {
            io::Error::other(format!(
                "shell setup failed ({error}); cleanup failed ({kill})"
            ))
        })?;
        return Err(error);
    }
    Ok((child, job))
}

fn resume_initial_thread(pid: u32) -> io::Result<()> {
    // Tokio/std discard the initial thread handle. ToolHelp is the documented
    // stable Win32 route to recover it; no nightly Rust or NtResumeProcess.
    // SAFETY: no pointers supplied; INVALID_HANDLE_VALUE is checked below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful snapshot creation returns an owned handle.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    // SAFETY: ToolHelp requires a zeroed entry with dwSize initialized.
    let mut entry: THREADENTRY32 = unsafe { zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    // SAFETY: snapshot and writable entry are valid through enumeration.
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
    while found != 0 {
        if entry.th32OwnerProcessID == pid {
            // The root is suspended and unreaped, so its PID cannot be reused.
            // SAFETY: request only resume access to its enumerated thread.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful OpenThread transfers ownership.
            let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
            // SAFETY: thread belongs to our suspended child, already in its job.
            if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            return Ok(());
        }
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        // SAFETY: same initialized entry and live snapshot as above.
        found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
    }
    Err(io::Error::other(
        "cannot find suspended shell's initial thread",
    ))
}

/// Never resolve System32's bash.exe (a WSL launcher) as our native backend.
/// An explicit override must be absolute; auto discovery only accepts Git's
/// usr/bin/sh.exe alongside its MSYS runtime, not arbitrary PATH executables.
pub fn shell_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("GRAY_BASH") {
        let path = PathBuf::from(path);
        if path.is_absolute() && path.is_file() {
            return Ok(path);
        }
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "GRAY_BASH must name an existing absolute Git Bash/sh executable",
        ));
    }
    let mut candidates = Vec::new();
    for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(key) {
            candidates.push(PathBuf::from(root).join("Git/usr/bin/sh.exe"));
        }
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(root).join("Programs/Git/usr/bin/sh.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join("sh.exe"));
            // Git's usual PATH entry is Git/cmd, not Git/usr/bin.
            candidates.push(dir.join("../usr/bin/sh.exe"));
        }
    }
    candidates.into_iter().find(|p| {
        p.is_absolute() && p.is_file()
            && p.parent().is_some_and(|d| d.join("msys-2.0.dll").is_file())
    }).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound,
        "Git for Windows is required for shell commands; install Git Bash or set GRAY_BASH to its absolute sh.exe path (not WSL)"))
}
