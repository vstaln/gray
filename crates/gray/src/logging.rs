//! Minimal file logger: appends timestamped lines to `$GRAY_HOME/logs/gray.log`.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use log::{LevelFilter, Log, Metadata, Record};

struct FileLogger {
    file: Mutex<std::fs::File>,
}

static INIT: OnceLock<()> = OnceLock::new();

/// ISO-ish timestamp: `YYYY-MM-DDTHH:MM:SSZ`.
fn iso_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        let Ok(mut file) = self.file.lock() else { return };
        let _ = writeln!(
            &mut *file,
            "{} {:<5} [{}] {}",
            iso_timestamp(),
            record.level().as_str(),
            record.target(),
            record.args()
        );
        let _ = file.flush();
    }

    fn flush(&self) {}
}

fn level_from_env() -> LevelFilter {
    match std::env::var("GRAY_LOG").as_deref() {
        Ok("error") => LevelFilter::Error,
        Ok("warn") => LevelFilter::Warn,
        Ok("debug") => LevelFilter::Debug,
        Ok("trace") => LevelFilter::Trace,
        _ => LevelFilter::Info, // unset or invalid → info
    }
}

/// Installs the file logger once; silently no-ops on second call or when
/// home cannot be resolved.
pub fn init() {
    INIT.get_or_init(|| {
        let Ok(home) = crate::setup::gray_home() else { return };
        let path = home.join("logs").join("gray.log");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
            if log::set_boxed_logger(Box::new(FileLogger { file: Mutex::new(file) })).is_ok() {
                log::set_max_level(level_from_env());
            }
        }
    });
}

