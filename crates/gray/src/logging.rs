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
            redact(&record.args().to_string())
        );
        let _ = file.flush();
    }

    fn flush(&self) {}
}

fn level_from_env() -> LevelFilter {
    std::env::var("GRAY_LOG").ok().and_then(|s| s.parse().ok()).unwrap_or(log::LevelFilter::Info)
}

/// Replaces secret-looking values with `[REDACTED]`: `sk-…` keys (8+ key
/// chars), `Bearer <token>`, and `x-api-key[:=]<value>`. Byte-scans ASCII
/// patterns only, so non-ASCII text passes through untouched.
pub(crate) fn redact(input: &str) -> String {
    let b = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"Bearer ") {
            let mut j = i + "Bearer ".len();
            while j < b.len() && !b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j > i + "Bearer ".len() {
                out.extend_from_slice(b"Bearer [REDACTED]");
                i = j;
                continue;
            }
        }
        if b.len() - i >= "x-api-key".len() && b[i..i + "x-api-key".len()].eq_ignore_ascii_case(b"x-api-key") {
            let mut k = i + "x-api-key".len();
            if k < b.len() && (b[k] == b'"' || b[k] == b'\'') {
                k += 1; // quoted key: "x-api-key":
            }
            while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
                k += 1;
            }
            if k < b.len() && (b[k] == b':' || b[k] == b'=') {
                k += 1;
                while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
                    k += 1;
                }
                let quoted = (k < b.len() && (b[k] == b'"' || b[k] == b'\'')) as usize;
                let vs = k + quoted;
                let mut e = vs;
                while e < b.len()
                    && (if quoted == 1 {
                        b[e] != b[k]
                    } else {
                        !b[e].is_ascii_whitespace() && b[e] != b',' && b[e] != b'}' && b[e] != b']'
                    })
                {
                    e += 1;
                }
                if e > vs {
                    out.extend_from_slice(&b[i..vs]);
                    out.extend_from_slice(b"[REDACTED]");
                    i = e;
                    continue;
                }
            }
        }
        if b[i..].starts_with(b"sk-") {
            let mut j = i + "sk-".len();
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b'_') {
                j += 1;
            }
            if j - (i + "sk-".len()) >= 8 {
                out.extend_from_slice(b"[REDACTED]");
                i = j;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_sk_key() {
        assert_eq!(redact("key sk-abcDEF123-_xyz done"), "key [REDACTED] done");
        // Short suffixes are not keys.
        assert_eq!(redact("sk-abc"), "sk-abc");
    }

    #[test]
    fn redact_bearer_token() {
        assert_eq!(redact("Authorization: Bearer tok123"), "Authorization: Bearer [REDACTED]");
        assert_eq!(redact("Bearer "), "Bearer ");
    }

    #[test]
    fn redact_x_api_key() {
        assert_eq!(redact("x-api-key: secret123"), "x-api-key: [REDACTED]");
        assert_eq!(redact(r#""x-api-key": "abc""#), r#""x-api-key": "[REDACTED]""#);
        assert_eq!(redact("X-API-KEY=zzz"), "X-API-KEY=[REDACTED]");
    }

    #[test]
    fn redact_leaves_plain_text_alone() {
        assert_eq!(redact("hello world, no secrets"), "hello world, no secrets");
        assert_eq!(redact("my x-api-key is secret"), "my x-api-key is secret");
    }
}

