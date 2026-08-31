//! Minimal file logger: appends timestamped lines to `$GRAY_HOME/logs/gray.log`.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use log::{LevelFilter, Log, Metadata, Record};

struct FileLogger {
    file: Mutex<std::fs::File>,
}

static INIT: OnceLock<()> = OnceLock::new();

/// ISO-ish timestamp: `YYYY-MM-DDTHH:MM:SSZ` from UNIX epoch seconds.
fn iso_timestamp(secs: u64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil-from-days algorithm (Howard Hinnant) — days since 1970-01-01.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        let Ok(mut file) = self.file.lock() else { return };
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(
            &mut *file,
            "{} {:<5} [{}] {}",
            iso_timestamp(secs),
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

