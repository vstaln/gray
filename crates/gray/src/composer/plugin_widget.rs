use ratatui::text::Line;
use std::time::{Duration, Instant};

#[derive(Debug, Default, PartialEq, serde::Deserialize)]
pub(crate) struct Snapshot {
    pub version: u32,
    pub text: String,
    #[serde(default)]
    pub shimmer_lines: Vec<usize>,

    #[serde(default)]
    pub shimmer_ranges: Vec<(usize, usize, usize)>,
}

pub(crate) fn lines(
    snapshot: &Snapshot,
    elapsed: Duration,
    max_rows: usize,
    width: usize,
) -> Vec<Line<'static>> {
    if snapshot.version != 1 || max_rows == 0 {
        return Vec::new();
    }
    let dim = ratatui::style::Style::default().fg(crate::theme::theme().text_dim);
    let text = |chars: &[char]| chars.iter().collect::<String>();

    let total = snapshot.text.lines().count();
    let cut = total > max_rows;
    let keep = if cut { max_rows - 1 } else { total };
    let mut rows: Vec<Line<'static>> = snapshot
        .text
        .lines()
        .take(keep)
        .enumerate()
        .map(|(index, line)| {
            let mut chars: Vec<char> = line
                .chars()
                .filter(|ch| !ch.is_control())
                .take(4096)
                .collect();

            let room = width.saturating_sub(1);
            if width > 0 && crate::text_width::chars_width(&chars) > room {
                chars.truncate(crate::text_width::fit_char_count(
                    &chars,
                    room.saturating_sub(1),
                ));
                chars.push('\u{2026}');
            }
            let mut ranges: Vec<(usize, usize)> = snapshot
                .shimmer_ranges
                .iter()
                .filter(|r| r.0 == index)
                .map(|r| (r.1.min(chars.len()), r.2.min(chars.len())))
                .filter(|(start, end)| start < end)
                .collect();
            if ranges.is_empty() && snapshot.shimmer_lines.contains(&index) {
                ranges.push((0, chars.len()));
            }
            let mut spans = vec![ratatui::text::Span::styled(" ", dim)];
            let mut at = 0;
            for (start, end) in ranges {
                if start < at {
                    continue;
                }
                if start > at {
                    spans.push(ratatui::text::Span::styled(text(&chars[at..start]), dim));
                }
                spans.extend(super::draw::shimmer_spans(
                    &text(&chars[start..end]),
                    elapsed,
                ));
                at = end;
            }
            if at < chars.len() {
                spans.push(ratatui::text::Span::styled(text(&chars[at..]), dim));
            }
            Line::from(spans)
        })
        .collect();
    if cut {
        rows.push(Line::from(ratatui::text::Span::styled(
            format!(" \u{2193} {} more lines", total - keep),
            dim,
        )));
    }
    rows
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
                let mut gate = SpecGate {
                    stamps: [None; 3],
                    argv: None,
                };
                let mut active = false;
                let mut last_spawn = Instant::now() - IDLE_POLL;
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Ok(home) = crate::plugin_cli::home() {
                        let resolved_fresh = gate.refresh(&home);
                        let interval = if active { TICK } else { IDLE_POLL };
                        if let Some(argv) = gate.argv() {
                            if resolved_fresh || last_spawn.elapsed() >= interval {
                                last_spawn = Instant::now();
                                let snapshot = spawn_widget(argv, &cwd).await.unwrap_or_default();
                                active = !snapshot.text.is_empty();
                                if tx.try_send(snapshot).is_err()
                                    && flag.load(std::sync::atomic::Ordering::Relaxed)
                                {
                                    break;
                                }
                            }
                        } else if resolved_fresh {
                            active = false;
                            if tx.try_send(Snapshot::default()).is_err()
                                && flag.load(std::sync::atomic::Ordering::Relaxed)
                            {
                                break;
                            }
                        }
                    }
                    tokio::time::sleep(TICK).await;
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
            let changed = self.snapshot != snapshot;
            self.snapshot = snapshot;
            return changed;
        }
        false
    }
    pub(crate) fn active(&self) -> bool {
        !self.snapshot.text.is_empty()
    }
    pub(crate) fn rows(&self, max_rows: usize, width: usize) -> Vec<Line<'static>> {
        lines(&self.snapshot, self.started.elapsed(), max_rows, width)
    }
}
impl Drop for Widget {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

const TICK: Duration = Duration::from_millis(500);

const IDLE_POLL: Duration = Duration::from_secs(5);

struct SpecGate {
    stamps: [Option<std::time::SystemTime>; 3],
    argv: Option<Vec<String>>,
}

impl SpecGate {
    fn refresh(&mut self, home: &std::path::Path) -> bool {
        let mtime =
            |path: std::path::PathBuf| std::fs::metadata(path).and_then(|m| m.modified()).ok();

        let project = std::env::current_dir()
            .ok()
            .map(|cwd| gray_plugin::lock::project_lock_path(&cwd));
        let stamps = [
            mtime(home.join("plugins/widgets.json")),
            mtime(gray_plugin::lock::lock_path(home)),
            project.and_then(mtime),
        ];
        if stamps == self.stamps {
            return false;
        }
        self.stamps = stamps;
        self.argv = resolve_widget_argv(home).ok();
        true
    }

    fn argv(&self) -> Option<&[String]> {
        self.argv.as_deref()
    }
}

fn resolve_widget_argv(home: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let raw = std::fs::read(home.join("plugins/widgets.json"))?;
    let spec: serde_json::Value = serde_json::from_slice(&raw)?;
    let name = spec["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing widget owner"))?;
    let registry = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(home))?;
    let entry = registry
        .plugins
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("widget owner is not registered"))?;
    anyhow::ensure!(
        crate::plugin_cli::enabled(home, name, entry),
        "widget owner is disabled"
    );
    Ok(entry.cli_argv.clone().unwrap_or_else(|| entry.argv.clone()))
}

async fn spawn_widget(argv: &[String], cwd: &std::path::Path) -> anyhow::Result<Snapshot> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty widget argv"))?;
    let mut command = tokio::process::Command::new(program);
    command.args(args).arg("widget").current_dir(cwd);
    let bytes = crate::plugin_cli::capture(command, Duration::from_secs(2)).await?;
    let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(snapshot.version == 1, "unsupported widget snapshot version");
    Ok(snapshot)
}
