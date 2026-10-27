use std::io;
use std::path::{Path, PathBuf};

use tokio::process::Command;

use super::contract::Spawned;

pub const EXEC_PREFIX_ENV: &str = "GRAY_EXEC_PREFIX";

pub fn split_prefix(raw: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            Some(_) => match c {
                c if c == quote.expect("double-quoted") => quote = None,
                '\\' => match chars.next() {
                    Some(escaped) => cur.push(escaped),
                    None => return Err("exec_prefix ends inside a quote".into()),
                },
                c => cur.push(c),
            },
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                '\\' => {
                    started = true;
                    match chars.next() {
                        Some(escaped) => cur.push(escaped),
                        None => return Err("exec_prefix ends with a backslash".into()),
                    }
                }
                c if c.is_whitespace() => {
                    if started {
                        out.push(std::mem::take(&mut cur));
                        started = false;
                    }
                }
                c => {
                    started = true;
                    cur.push(c);
                }
            },
        }
    }
    if let Some(unterminated) = quote {
        return Err(format!(
            "exec_prefix has an unterminated {unterminated} quote"
        ));
    }
    if started {
        out.push(cur);
    }
    Ok(out)
}

fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Environment contributed by plugins through the `shell_env` manifest
/// field, merged once per registry build in plugin order.
#[derive(Debug, Clone, Default)]
pub struct PluginEnv {
    vars: Vec<(String, String)>,
    path_prepend: Vec<PathBuf>,
}

/// Variables gray sets itself in [`shell_env`]; plugins may not override
/// them. `PATH` is reserved too — plugins extend it via `path_prepend`.
const RESERVED_ENV: &[&str] = &[
    "DEBIAN_FRONTEND",
    "GIT_TERMINAL_PROMPT",
    "SUDO_ASKPASS",
    "TERM",
    "NO_COLOR",
    "CLICOLOR",
    "PAGER",
    "MANPAGER",
    "GIT_PAGER",
    "SYSTEMD_PAGER",
    "GRAY_SESSION_ID",
    "GRAY_CWD_REPORT",
    "GRAY_PS_REPORT",
    "PATH",
];

impl PluginEnv {
    /// Fold one plugin's `shell_env` contribution into the merged set.
    /// Reserved variable names and non-absolute `path_prepend` entries are
    /// skipped with a warning.
    pub fn add_plugin(
        &mut self,
        plugin: &str,
        vars: impl IntoIterator<Item = (String, String)>,
        path_prepend: impl IntoIterator<Item = PathBuf>,
    ) {
        for (key, value) in vars {
            if RESERVED_ENV.contains(&key.as_str()) {
                log::warn!("plugin {plugin} shell_env var {key:?} is reserved — skipping");
                continue;
            }
            match self.vars.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => self.vars.push((key, value)),
            }
        }
        for dir in path_prepend {
            if !dir.is_absolute() {
                log::warn!(
                    "plugin {plugin} shell_env path_prepend {:?} is not absolute — skipping",
                    dir.display()
                );
                continue;
            }
            self.path_prepend.push(dir);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vars.is_empty() && self.path_prepend.is_empty()
    }
}

fn shell_env(
    session: Option<&str>,
    cwd_report: Option<&Path>,
    ps_report: Option<&Path>,
    plugin: Option<&PluginEnv>,
) -> Vec<(String, String)> {
    let mut env: Vec<(&str, &str)> = vec![
        ("DEBIAN_FRONTEND", "noninteractive"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("SUDO_ASKPASS", "/bin/false"),
        ("TERM", "dumb"),
        ("NO_COLOR", "1"),
        ("CLICOLOR", "0"),
        ("PAGER", "cat"),
        ("MANPAGER", "cat"),
        ("GIT_PAGER", "cat"),
        ("SYSTEMD_PAGER", "cat"),
    ];
    if let Some(session) = session.filter(|s| !s.trim().is_empty()) {
        env.push(("GRAY_SESSION_ID", session.trim()));
    }
    if let Some(report) = cwd_report {
        env.push(("GRAY_CWD_REPORT", report.to_str().unwrap_or_default()));
    }
    if let Some(report) = ps_report {
        env.push(("GRAY_PS_REPORT", report.to_str().unwrap_or_default()));
    }
    let mut env: Vec<(String, String)> = env
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    if let Some(plugin) = plugin {
        env.extend(plugin.vars.iter().cloned());
    }
    env
}

fn write_script(command: &str, env: &[(String, String)]) -> io::Result<PathBuf> {
    let mut script = String::new();
    for (key, value) in env {
        script.push_str(&format!("export {key}={}\n", shell_quote(value)));
    }
    script.push_str(command);
    if !script.ends_with('\n') {
        script.push('\n');
    }
    let path = std::env::temp_dir().join(format!("gray-exec-{}.sh", uuid::Uuid::new_v4()));
    std::fs::write(&path, script)?;
    Ok(path)
}

pub fn spawn(
    command: &str,
    cwd: &Path,
    session: Option<&str>,
    cwd_report: Option<&Path>,
    ps_report: Option<&Path>,
    plugin_env: Option<&PluginEnv>,
) -> io::Result<Spawned> {
    let raw = std::env::var(EXEC_PREFIX_ENV).unwrap_or_default();
    let prefix = match raw.trim() {
        "" => None,
        _ => match split_prefix(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
            .split_first()
        {
            Some((program, args)) if !program.is_empty() => {
                let mut argv = vec![program.clone()];
                argv.extend(args.iter().cloned());
                Some(argv)
            }
            _ => None,
        },
    };
    spawn_with_prefix(
        command, cwd, session, cwd_report, prefix, ps_report, plugin_env,
    )
}

pub fn pipestatus_shell() -> bool {
    if std::env::var(EXEC_PREFIX_ENV)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    #[cfg(windows)]
    {
        true
    }
    #[cfg(not(windows))]
    {
        bash_available()
    }
}

#[cfg(not(windows))]
fn bash_available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        std::process::Command::new("bash")
            .args(["-c", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

pub fn spawn_with_prefix(
    command: &str,
    cwd: &Path,
    session: Option<&str>,
    cwd_report: Option<&Path>,
    prefix: Option<Vec<String>>,
    ps_report: Option<&Path>,
    plugin_env: Option<&PluginEnv>,
) -> io::Result<Spawned> {
    let mut env = shell_env(session, cwd_report, ps_report, plugin_env);
    let mut cmd = match &prefix {
        Some(argv) => {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..]);
            cmd
        }
        None => local_command()?,
    };
    cmd.current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (key, value) in &env {
        cmd.env(key, value);
    }
    if let Some(dirs) = plugin_env.map(|p| p.path_prepend.as_slice())
        && !dirs.is_empty()
    {
        // Plugin directories go ahead of whatever PATH the child would
        // otherwise inherit (on Windows `local_command` has already added
        // the shell's own directory, which is kept).
        let base = cmd
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new("PATH"))
            .and_then(|(_, value)| value.map(std::ffi::OsString::from))
            .or_else(|| std::env::var_os("PATH"));
        let mut paths: Vec<PathBuf> = dirs.to_vec();
        if let Some(base) = base {
            paths.extend(std::env::split_paths(&base));
        }
        let joined = std::env::join_paths(&paths).map_err(io::Error::other)?;
        cmd.env("PATH", &joined);
        env.push(("PATH".to_string(), joined.to_string_lossy().into_owned()));
    }
    match &prefix {
        Some(_) => {
            let script = write_script(command, &env)?;
            let file = std::fs::File::open(&script);

            cmd.stdin(std::process::Stdio::from(file?));
            match spawn_child(&mut cmd) {
                Ok(spawned) => {
                    let _ = std::fs::remove_file(&script);
                    Ok(spawned)
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&script);
                    Err(e)
                }
            }
        }
        None => {
            cmd.arg("-c")
                .arg(command)
                .stdin(std::process::Stdio::null());
            spawn_child(&mut cmd)
        }
    }
}

fn local_command() -> io::Result<Command> {
    #[cfg(not(windows))]
    {
        Ok(Command::new(if bash_available() { "bash" } else { "sh" }))
    }
    #[cfg(windows)]
    {
        let shell = super::windows::shell_path()?;
        let mut cmd = Command::new(&shell);

        let mut paths = vec![
            shell
                .parent()
                .expect("absolute executable has parent")
                .to_path_buf(),
        ];
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        cmd.env(
            "PATH",
            std::env::join_paths(paths).map_err(io::Error::other)?,
        );

        cmd.env_remove("BASH_ENV").env_remove("ENV");
        Ok(cmd)
    }
}

fn spawn_child(cmd: &mut Command) -> io::Result<Spawned> {
    #[cfg(unix)]
    {
        unsafe {
            cmd.pre_exec(|| {
                if check_setsid(libc::setsid()).is_err() {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(windows))]
    let child = cmd.spawn()?;
    #[cfg(windows)]
    let (child, job) = super::windows::spawn_owned(cmd)?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("spawned child has no pid"))?;
    Ok(Spawned {
        child,
        pid,
        #[cfg(not(windows))]
        pgid: pid as i32,
        #[cfg(windows)]
        job,
    })
}

#[cfg(unix)]
fn check_setsid(ret: libc::pid_t) -> io::Result<()> {
    if ret == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
