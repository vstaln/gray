use super::*;
use std::sync::Arc;
use tokio::process::Command;

/// Inline POSIX script with both pipes captured; the caller takes what it needs.
fn spawn_sh(
    script: &str,
) -> (
    tokio::process::Child,
    Option<ChildStdout>,
    Option<ChildStderr>,
) {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (out, err) = (child.stdout.take(), child.stderr.take());
    (child, out, err)
}

fn tmp_log(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("gray-pump-{}-{name}.log", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn shell_log_cap_trips_past_10mib() {
    assert!(!log_capped(0, 1));
    assert!(!log_capped(0, SHELL_LOG_MAX_BYTES));
    assert!(log_capped(0, SHELL_LOG_MAX_BYTES + 1));
    assert!(log_capped(SHELL_LOG_MAX_BYTES, 1));
    assert!(!log_capped(SHELL_LOG_MAX_BYTES - 2, 1));
}

#[test]
fn mem_view_tracks_cr_across_chunks() {
    let mut mem = MemView::new();
    mem.push(b"line one\nline two");
    assert!(!mem.has_cr);
    mem.push(b"\r\nline three\r\n");
    assert!(mem.has_cr);
    let s = mem.into_summary(false);
    assert!(s.has_cr);
}

#[test]
fn mem_view_bounds_head_and_tail() {
    // 5 MiB in 8 KiB pushes with a newline every 100th byte.
    let total = 5 * 1024 * 1024;
    let data: Vec<u8> = (0..total)
        .map(|i| {
            if i % 100 == 99 {
                b'\n'
            } else {
                b'a' + (i % 26) as u8
            }
        })
        .collect();
    let mut mem = MemView::new();
    for chunk in data.chunks(8 * 1024) {
        mem.push(chunk);
    }
    assert_eq!(mem.total_bytes, total as u64);
    assert_eq!(
        mem.total_lines,
        data.iter().filter(|&&b| b == b'\n').count()
    );
    assert_eq!(mem.head.len(), MEM_HEAD_BYTES);
    assert_eq!(mem.head, &data[..MEM_HEAD_BYTES]);
    assert_eq!(mem.tail.len(), MEM_TAIL_BYTES);
    assert_eq!(
        mem.tail.iter().copied().collect::<Vec<u8>>(),
        &data[total - MEM_TAIL_BYTES..]
    );
    assert!(mem.head.len() + mem.tail.len() <= 64 * 1024);
}

#[tokio::test]
async fn pump_logs_both_streams_verbatim() {
    let (mut child, out, err) =
        spawn_sh("printf 'out-1\\n'; printf 'err-1\\n' >&2; printf 'out-2\\n'");
    let log = tmp_log("both");
    let h = Pump::start(out, err, log.clone(), Arc::new(AtomicU64::new(0)));
    assert!(child.wait().await.unwrap().success());
    let s = h.await.unwrap();
    assert_eq!(s.total_bytes, 18);
    assert_eq!(s.total_lines, 3);
    assert!(!s.log_write_failed);
    let file = std::fs::read(&log).unwrap();
    assert_eq!(file.len() as u64, s.total_bytes);
    for marker in [b"out-1\n", b"err-1\n", b"out-2\n"] {
        assert!(
            file.windows(marker.len()).any(|w| w == marker),
            "log holds {marker:?}"
        );
    }
    assert_eq!(s.head, file); // under budget: head is the whole stream
    let _ = std::fs::remove_file(&log);
}

#[tokio::test]
async fn pump_redacts_secret_shaped_assignment() {
    // `*_token=<value>` hits SECRET_NAME_MARKERS ("token"); the name is
    // assembled at runtime so no secret-shaped literal sits in source.
    let name: String = [109u8, 121, 95, 116, 111, 107, 101, 110]
        .iter()
        .map(|b| *b as char)
        .collect();
    // "value123": lowercase + digits, no shape of its own — only the
    // secret-bearing name triggers redaction.
    let val = "value123";
    let script = format!("printf '\\n{name}={val}\\n'");
    let (mut child, out, err) = spawn_sh(&script);
    drop(err);
    let log = tmp_log("redact");
    let h = Pump::start(out, None, log.clone(), Arc::new(AtomicU64::new(0)));
    assert!(child.wait().await.unwrap().success());
    let s = h.await.unwrap();
    let file = std::fs::read(&log).unwrap();
    let text = String::from_utf8_lossy(&file);
    assert!(!text.contains(val), "{text}");
    assert!(
        text.contains(&name),
        "name is the useful half, kept: {text}"
    );
    // Memory view and file agree (both sides see the redacted bytes).
    assert_eq!(s.total_bytes, file.len() as u64);
    let _ = std::fs::remove_file(&log);
}

#[tokio::test]
async fn pump_unwritable_log_sets_flag_without_panic() {
    let blocker = tmp_log("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let bad = blocker.join("bash-test.log"); // parent is a file: mkdir + open both fail
    let (mut child, out, _err) = spawn_sh("printf 'still counted\\n'");
    let h = Pump::start(out, None, bad, Arc::new(AtomicU64::new(0)));
    assert!(child.wait().await.unwrap().success());
    assert!(h.await.unwrap().log_write_failed);
    let _ = std::fs::remove_file(&blocker);
}

/// Feeds fixed byte groups, one per `read()`, then EOF. Deterministic split
/// points without a child process or a sleep.
struct SplitReads(Vec<Vec<u8>>);

impl tokio::io::AsyncRead for SplitReads {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.0.is_empty() {
            return std::task::Poll::Ready(Ok(()));
        }
        let next = self.0.remove(0);
        buf.put_slice(&next);
        std::task::Poll::Ready(Ok(()))
    }
}

/// A read can end mid-token; the redactor's unit is the line. Redacting the
/// pieces separately leaked the tail of any secret that straddled a boundary.
#[tokio::test]
async fn pump_holds_a_partial_line_until_its_terminator() {
    use gray_core::redaction::redact_bytes_for_log;
    // The name assembles at runtime so no secret-shaped literal sits in source.
    let name: String = [109u8, 121, 95, 116, 111, 107, 101, 110]
        .iter()
        .map(|b| *b as char)
        .collect();
    let (head, tail) = ("value12", "3456789");
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(8);
    let last = Arc::new(AtomicU64::new(0));
    let reads = SplitReads(vec![
        format!("{name}={head}").into_bytes(),
        format!("{tail}\n").into_bytes(),
    ]);
    read_pipe(reads, tx, Arc::clone(&last)).await;

    let mut messages = Vec::new();
    while let Some(m) = rx.recv().await {
        messages.push(m);
    }
    assert_eq!(messages.len(), 1, "the split line is forwarded whole");
    assert!(
        last.load(Ordering::Relaxed) > 0,
        "liveness stamped on raw arrival"
    );
    let text = String::from_utf8_lossy(&redact_bytes_for_log(&messages[0])).into_owned();
    assert!(!text.contains(tail), "secret tail leaked: {text}");
    assert!(
        text.contains(&name),
        "name is the useful half, kept: {text}"
    );
}
