//! Generic plugin-owned above-editor rows. No subagent dependency or layout here.
use ratatui::text::Line;
use std::time::{Duration, Instant};

#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct Snapshot {
    pub version: u32,
    pub text: String,
    #[serde(default)]
    pub shimmer_lines: Vec<usize>,
}

pub(crate) fn lines(snapshot: &Snapshot, elapsed: Duration, max_rows: usize) -> Vec<Line<'static>> {
    if snapshot.version != 1 {
        return Vec::new();
    }
    snapshot
        .text
        .lines()
        .take(max_rows.min(12))
        .enumerate()
        .map(|(index, text)| {
            let safe: String = text
                .chars()
                .filter(|ch| !ch.is_control())
                .take(4096)
                .collect();
            if snapshot.shimmer_lines.contains(&index) {
                Line::from(super::draw::shimmer_spans(&safe, elapsed))
            } else {
                Line::styled(
                    safe,
                    ratatui::style::Style::default().fg(crate::theme::theme().text_dim),
                )
            }
        })
        .collect()
}

pub(crate) struct Widget {
    latest: std::sync::mpsc::Receiver<Snapshot>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    snapshot: Snapshot,
    started: Instant,
}
impl Widget {
    pub(crate) fn new(cwd: std::path::PathBuf) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    let snapshot = read_widget(&cwd).await.unwrap_or_default();
                    if tx.try_send(snapshot).is_err()
                        && flag.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            });
        });
        Self {
            latest: rx,
            stop,
            snapshot: Snapshot::default(),
            started: Instant::now(),
        }
    }
    pub(crate) fn refresh(&mut self) -> bool {
        if let Ok(snapshot) = self.latest.try_recv() {
            self.snapshot = snapshot;
            return true;
        }
        false
    }
    pub(crate) fn active(&self) -> bool {
        !self.snapshot.text.is_empty()
    }
    pub(crate) fn rows(&self, max_rows: usize) -> Vec<Line<'static>> {
        lines(&self.snapshot, self.started.elapsed(), max_rows)
    }
}
impl Drop for Widget {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

async fn read_widget(cwd: &std::path::Path) -> anyhow::Result<Snapshot> {
    use tokio::io::AsyncReadExt;
    let home = crate::plugin_cli::home()?;
    let raw = tokio::fs::read(home.join("plugins/widgets.json")).await?;
    let spec: serde_json::Value = serde_json::from_slice(&raw)?;
    let argv: Vec<String> = serde_json::from_value(spec["argv"].clone())?;
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty widget argv"))?;
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().unwrap().take(65537);
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await?;
        anyhow::ensure!(bytes.len() <= 65536, "widget snapshot too large");
        anyhow::ensure!(child.wait().await?.success(), "widget process failed");
        Ok::<Snapshot, anyhow::Error>(serde_json::from_slice(&bytes)?)
    })
    .await;
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result?
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        widgets::{Paragraph, Widget as _},
    };
    #[test]
    fn snapshot_paints_rows_above_input_with_existing_shimmer() {
        let Some(binary) = std::env::var_os("GRAY_WIDGET_TEST_BIN") else {
            return;
        };
        let output = std::process::Command::new(binary)
            .args(["widget", "--demo"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let snapshot: Snapshot = serde_json::from_slice(&output.stdout).unwrap();
        let rows = lines(&snapshot, Duration::from_millis(900), 12);
        assert_eq!(rows.len(), 5);
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 10));
        Paragraph::new(rows.clone()).render(Rect::new(0, 0, 100, 5), &mut buf);
        Paragraph::new("› draft survives").render(Rect::new(0, 6, 100, 1), &mut buf);
        let text = |y| (0..100).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert!(text(0).starts_with("⬢ Agents"));
        assert!(text(1).contains("⬡ Scout"));
        assert!(text(4).contains("⎿  reading…"));
        assert!(text(6).starts_with("› draft survives"));
        let expected = super::super::draw::shimmer_spans(
            snapshot.text.lines().nth(1).unwrap(),
            Duration::from_millis(900),
        );
        assert_eq!(rows[1].spans, expected);
    }
    #[test]
    fn invalid_versions_and_control_characters_cannot_escape_widget() {
        assert!(
            lines(
                &Snapshot {
                    version: 2,
                    text: "x".into(),
                    shimmer_lines: vec![]
                },
                Duration::ZERO,
                12
            )
            .is_empty()
        );
        let rows = lines(
            &Snapshot {
                version: 1,
                text: "\x1b[2Jhello\nnext".into(),
                shimmer_lines: vec![],
            },
            Duration::ZERO,
            1,
        );
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].to_string().contains('\x1b'));
    }
}
