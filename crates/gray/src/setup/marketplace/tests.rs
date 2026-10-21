use crossterm::event::KeyCode;

fn snapshot(model: &super::model::MarketModel, width: u16, height: u16) -> String {
    snapshot_with_consent(model, width, height, None)
}

fn snapshot_with_consent(
    model: &super::model::MarketModel,
    width: u16,
    height: u16,
    consent: Option<&super::ConsentView>,
) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| super::draw::render(frame, frame.area(), model, consent))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn loading_snapshot_has_no_install_action() {
    let screen = snapshot(&super::model::MarketModel::default(), 80, 20);
    assert!(screen.contains("Loading marketplace"));
    assert!(!screen.contains("[ Install ]"));
}

#[test]
fn loading_replays_the_logo_intro_and_falls_back_to_a_spinner_when_small() {
    let mut model = super::model::MarketModel {
        loading_since: Some(std::time::Instant::now() - std::time::Duration::from_millis(1200)),
        ..Default::default()
    };
    let screen = snapshot(&model, 80, 20);
    let mark_rows = screen
        .lines()
        .filter(|line| line.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)))
        .count();
    assert_eq!(mark_rows, 6, "{screen}");
    let label = screen
        .lines()
        .find(|line| line.contains("Loading marketplace…"))
        .unwrap();
    assert!(!label.contains('⠋'), "{screen}");
    model.loading_since = None;
    assert!(snapshot(&model, 80, 20).contains("Loading marketplace…"));
    assert!(snapshot(&model, 40, 5).starts_with("⠋ Loading marketplace…"));
}

#[tokio::test]
async fn only_loading_requests_fast_redraws() {
    use super::ManagerExtension;
    let mut market = super::Marketplace::with_worker(super::worker::spawn_backend(
        std::sync::Arc::new(RaceBackend::default()),
    ));
    market.model.phase = super::model::MarketPhase::Loading;
    assert!(market.animating());
    market.model.loaded(index(&["demo"]), Default::default());
    assert!(!market.animating());
    finish_market(market).await;
}

fn index(names: &[&str]) -> gray_pkg::index::Index {
    let plugins = names
        .iter()
        .map(|name| {
            (
                name.to_string(),
                serde_json::json!({
                    "ecosystem": "gray-native", "version": "1.0.0",
                    "source": {"type": "tarball", "url": "https://example.invalid/demo.tgz"},
                    "hash": format!("sha256:{}", "0".repeat(64)),
                    "description": format!("{name} helper")
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    serde_json::from_value(serde_json::json!({"plugins": plugins})).unwrap()
}

#[test]
fn search_owns_j_and_k_and_installed_uses_catalog_key() {
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["jack", "ledger"]), ["jack".to_string()].into());
    model.search_focused = true;
    model.input(KeyCode::Char('j'));
    assert_eq!(model.query, "j");
    assert_eq!(model.visible_names(), vec!["jack"]);
    assert!(!model.can_install());
    model.input(KeyCode::Backspace);
    model.input(KeyCode::Char('k'));
    assert_eq!(model.query, "k");
}

#[test]
fn scrolling_keeps_last_selection_visible() {
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["a", "b", "c", "d", "e"]), Default::default());
    for _ in 0..4 {
        model.move_selection(1, 2);
    }
    assert_eq!(model.selected, 4);
    assert_eq!(model.offset, 3);
}

#[test]
fn stale_completion_cannot_advance_a_new_operation() {
    let mut model = super::model::MarketModel::default();
    let old = model.begin(super::model::MarketPhase::Preparing);
    let current = model.begin(super::model::MarketPhase::Loading);
    assert!(!model.accept_operation(old));
    assert!(model.accept_operation(current));
}

#[test]
fn description_search_is_case_insensitive_and_resets_selection() {
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["a", "b"]), Default::default());
    model.entries.get_mut("b").unwrap().description = Some("JOURNAL".into());
    model.move_selection(1, 1);
    model.search_focused = true;
    for c in "journal".chars() {
        model.input(KeyCode::Char(c));
    }
    assert_eq!(model.visible_names(), vec!["b"]);
    assert_eq!((model.selected, model.offset), (0, 0));
    model.input(KeyCode::Char('z'));
    model.move_selection(1, 0);
    assert!(model.visible_names().is_empty());
    assert!(!model.entries.is_empty());
    assert!(!model.can_install());
    assert_eq!((model.selected, model.offset), (0, 0));
}

#[test]
fn installed_identity_ignores_version_runtime_name_and_enabled_state() {
    let mut model = super::model::MarketModel::default();
    // Registry loading passes every key, independent of each row's metadata.
    model.loaded(index(&["demo"]), ["demo".into(), "local".into()].into());
    assert!(!model.can_install());
    model.input(KeyCode::Char('i'));
    assert_eq!(model.phase, super::model::MarketPhase::Browsing);
}

#[test]
fn confirmation_snapshot_is_locked_and_consent_requires_explicit_grant() {
    use super::model::{Action, MarketPhase};
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["demo"]), Default::default());
    model.input(KeyCode::Enter);
    assert_eq!(model.phase, MarketPhase::Confirm);
    model.entries.get_mut("demo").unwrap().version = "2.0.0".into();
    assert_eq!(model.approved.as_ref().unwrap().entry().version, "1.0.0");
    assert_eq!(model.input(KeyCode::Enter), Action::Prepare);
    model.phase = MarketPhase::Consent;
    assert_eq!(model.input(KeyCode::Enter), Action::Cancel);
    assert_eq!(model.input(KeyCode::Char('g')), Action::Grant);
    model.phase = MarketPhase::Committing;
    assert_eq!(model.input(KeyCode::Esc), Action::None);
    assert!(model.blocks_tabs());
}

#[test]
fn escape_leaves_search_before_closing_browsing() {
    use super::model::Action;
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["demo"]), Default::default());
    model.search_focused = true;
    assert_eq!(model.input(KeyCode::Esc), Action::None);
    assert!(!model.search_focused);
    assert_eq!(model.input(KeyCode::Esc), Action::Close);
}

fn compact(screen: &str) -> Vec<&str> {
    screen.lines().map(str::trim_end).collect()
}

#[test]
fn stable_status_buffer_snapshots() {
    use super::model::{MarketModel, MarketPhase};
    let mut model = MarketModel::default();
    assert_eq!(
        compact(&snapshot(&model, 40, 5)),
        ["⠋ Loading marketplace…", "", "", "", ""]
    );
    model.phase = MarketPhase::LoadError;
    model.message = Some("HTTP 503".into());
    assert_eq!(
        compact(&snapshot(&model, 40, 5)),
        [
            "Marketplace unavailable",
            "HTTP 503",
            "",
            "",
            "r retry · Esc back"
        ]
    );
    model.loaded(index(&[]), Default::default());
    assert_eq!(
        compact(&snapshot(&model, 40, 5)),
        [
            "No plugins available",
            "",
            "",
            "",
            "←→ tabs · / search · ↑↓/jk select · Ente"
        ]
    );
    model.loaded(index(&["demo"]), Default::default());
    model.query = "absent".into();
    let screen = snapshot(&model, 40, 5);
    let rows = compact(&screen);
    assert!(rows[0].starts_with("/ absent"), "{screen}");
    assert!(rows[0].ends_with("0 of 1"), "{screen}");
    assert_eq!(rows[1], "No matching plugins", "{screen}");
}

#[test]
fn browse_and_details_show_trust_installed_badges_and_metadata() {
    let mut model = super::model::MarketModel::default();
    let mut catalog = index(&["demo"]);
    let entry = catalog.plugins.get_mut("demo").unwrap();
    entry.verified = true;
    entry.author = Some("Builder".into());
    entry.kind = "plugin".into();
    entry.commands = vec!["/demo".into()];
    entry.license = "MIT".into();
    entry.homepage = Some("https://example.invalid/home".into());
    entry.repo = Some("https://example.invalid/repo".into());
    model.loaded(catalog, ["demo".into()].into());
    let screen = snapshot(&model, 80, 20);
    let row = screen.lines().find(|l| l.contains("> ◐ demo")).unwrap();
    assert!(row.trim_end().ends_with("1.0.0 ✓"), "{screen}");
    let screen = snapshot(&model, 110, 20);
    for field in [
        "demo  1.0.0  ✓ installed",
        "demo helper",
        "Author    Builder",
        "Trust     ◐ verified community",
        "Kind      plugin",
        "Commands  /demo",
        "License   MIT",
        "Homepage  https://example.invalid/home",
        "Repo      https://example.invalid/repo",
    ] {
        assert!(screen.contains(field), "{field} missing:\n{screen}");
    }
}

fn consent_fixture() -> super::ConsentView {
    super::ConsentView {
        capabilities: crate::plugin_install::policy::capability_lines(&[
            "host.ask".into(),
            "unknown.capability".into(),
        ]),
        commands: vec!["/demo".into()],
        scan_text: "Caution: shell invocation\nreview plugin.sh:7".into(),
    }
}

#[test]
fn confirmation_and_consent_render_the_approved_information() {
    use super::model::MarketPhase;
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["demo"]), Default::default());
    model.confirm();
    let screen = snapshot(&model, 100, 20);
    for expected in [
        "Confirm preparation: demo",
        "Version   1.0.0",
        "Source    https://example.invalid/demo.tgz",
        "Hash      sha256:",
        "Trust     ○ unverified community",
        "! A matching hash proves integrity, not safety.",
        "! Preparing will run this plugin's manifest command as you.",
        "Enter approve · Esc back",
    ] {
        assert!(screen.contains(expected), "{expected} missing:\n{screen}");
    }
    model.phase = MarketPhase::Consent;
    let consent = consent_fixture();
    let screen = snapshot_with_consent(&model, 100, 20, Some(&consent));
    for expected in [
        "Capability consent",
        "• host.ask",
        "• unknown.capability  not grantable",
        "Commands  /demo",
        "  Caution: shell invocation",
        "  review plugin.sh:7",
        " Grant · g ",
        " Cancel · Enter/Esc ",
    ] {
        assert!(screen.contains(expected), "{expected} missing:\n{screen}");
    }
}

#[test]
fn progress_success_and_failure_buffers() {
    use super::model::MarketPhase;
    use gray_pkg::staged::InstallStage;
    let mut model = super::model::MarketModel {
        phase: MarketPhase::Preparing,
        stage: Some(InstallStage::Scanning),
        ..Default::default()
    };
    assert_eq!(
        compact(&snapshot(&model, 60, 12)),
        [
            "",
            "Installing plugin",
            "",
            "✓ Downloading",
            "✓ Unpacking",
            "⠋ Scanning",
            "· Reading manifest",
            "· Committing",
            "",
            "",
            "",
            "Esc cancel · cleanup finishes before leaving"
        ]
    );
    model.phase = MarketPhase::Committing;
    model.stage = Some(InstallStage::Committing);
    assert!(snapshot(&model, 80, 8).contains("cancellation disabled"));
    model.phase = MarketPhase::Installed;
    model.message = Some("demo installed\nWarning: provider refresh failed".into());
    let screen = snapshot(&model, 80, 8);
    assert!(screen.contains("Installed"));
    assert!(screen.contains("Warning: provider refresh failed"));
    assert!(!screen.contains("Install failed"));
    model.phase = MarketPhase::InstallFailed;
    model.message = Some("missing required plugins: questions, ledger".into());
    let screen = snapshot(&model, 80, 8);
    assert!(screen.contains("missing required plugins: questions, ledger"));
    assert!(screen.contains("r retry · Esc back"));
}

#[test]
fn all_phases_render_on_narrow_and_tiny_terminals() {
    use super::model::MarketPhase::*;
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["demo"]), Default::default());
    model.confirm();
    let consent = consent_fixture();
    for phase in [
        Loading,
        LoadError,
        Browsing,
        Confirm,
        Preparing,
        Consent,
        Committing,
        Installed,
        InstallFailed,
    ] {
        model.phase = phase;
        model.message = Some("example message".into());
        for (width, height) in [(20, 6), (80, 20), (1, 1), (0, 0)] {
            snapshot_with_consent(&model, width, height, Some(&consent));
        }
    }
}

#[test]
fn final_row_stays_visible_and_long_text_wraps_and_scrolls() {
    let names = (0..150)
        .map(|i| format!("plugin-{i:03}"))
        .collect::<Vec<_>>();
    let mut model = super::model::MarketModel::default();
    model.loaded(
        index(&names.iter().map(String::as_str).collect::<Vec<_>>()),
        Default::default(),
    );
    model.move_selection(149, 15);
    assert!(snapshot(&model, 80, 20).contains("> ○ plugin-149"));
    model.entries.get_mut("plugin-149").unwrap().description = Some("a".repeat(200));
    let screen = snapshot(&model, 20, 10);
    assert!(screen.contains("aaaaaaaaaaaaaaaaaaaa"));
    model.scroll = u16::MAX;
    let screen = snapshot(&model, 20, 10);
    assert!(screen.contains("unverified community"), "{screen}");
    assert!(screen.contains("> ○ plugin-149"), "{screen}");
}

#[test]
fn catalog_manifest_and_errors_cannot_inject_controls_or_url_credentials() {
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["demo"]), Default::default());
    let entry = model.entries.get_mut("demo").unwrap();
    entry.description = Some("\x1b[31mhello\x1b[0m\x07".into());
    entry.source.url = "https://user:secret@example.invalid/demo?token=private#secret".into();
    let screen = snapshot(&model, 100, 20);
    assert!(screen.contains("hello"));
    for absent in ["\x1b", "\x07", "secret", "private", "user:"] {
        assert!(!screen.contains(absent));
    }
    model.phase = super::model::MarketPhase::InstallFailed;
    model.message = Some("failed https://u:secret@example.invalid/a?token=private\x1b[0m".into());
    let screen = snapshot(&model, 100, 20);
    assert!(!screen.contains("secret"));
    assert!(!screen.contains("private"));
}

#[derive(Default)]
struct FakeBackend {
    commits: std::sync::atomic::AtomicUsize,
    prepare_started: tokio_util::sync::CancellationToken,
    prepare_cancelled: tokio_util::sync::CancellationToken,
}

#[async_trait::async_trait]
impl super::worker::MarketBackend for FakeBackend {
    type Prepared = ();

    async fn load(
        &self,
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<(gray_pkg::index::Index, std::collections::BTreeSet<String>)> {
        Ok((index(&["demo"]), Default::default()))
    }

    async fn prepare(
        &self,
        _: gray_pkg::staged::ApprovedEntry,
        cancel: tokio_util::sync::CancellationToken,
        _: tokio::sync::mpsc::UnboundedSender<gray_pkg::staged::InstallStage>,
    ) -> anyhow::Result<((), super::ConsentView)> {
        self.prepare_started.cancel();
        cancel.cancelled().await;
        self.prepare_cancelled.cancel();
        anyhow::bail!("preparation cancelled")
    }

    async fn commit(&self, _: ()) -> anyhow::Result<crate::plugin_install::InstallResult> {
        self.commits
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(crate::plugin_install::InstallResult {
            name: "demo".into(),
            installed: true,
            warnings: Vec::new(),
        })
    }
}

fn approved_demo() -> gray_pkg::staged::ApprovedEntry {
    gray_pkg::staged::ApprovedEntry::new(
        "demo".into(),
        index(&["demo"]).plugins.remove("demo").unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn close_waits_for_preparation_cleanup_and_never_installs() {
    use super::worker::{MarketCommand, MarketEvent, TaggedCommand};
    let fake = std::sync::Arc::new(FakeBackend::default());
    let mut worker = super::worker::spawn_backend(fake.clone());
    worker
        .commands
        .send(TaggedCommand {
            operation: 1,
            command: MarketCommand::Prepare(Box::new(approved_demo())),
        })
        .unwrap();
    fake.prepare_started.cancelled().await;
    worker
        .commands
        .send(TaggedCommand {
            operation: 1,
            command: MarketCommand::Close,
        })
        .unwrap();
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), worker.events.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(event.event, MarketEvent::Closed) {
            break;
        }
        assert!(!matches!(
            event.event,
            MarketEvent::Ready(_) | MarketEvent::Committed(_)
        ));
    }
    assert_eq!(fake.commits.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(fake.prepare_cancelled.is_cancelled());
    worker.task.await.unwrap();
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Behavior {
    #[default]
    Ready,
    LateReady,
    LoadFailed,
    PrepareFailed,
    Panic,
    CommitFailed,
    Warning,
    AlreadyInstalled,
    RefreshFailed,
}

#[derive(Default)]
struct RaceBackend {
    behavior: Behavior,
    loads: std::sync::atomic::AtomicUsize,
    commits: std::sync::atomic::AtomicUsize,
    failures: std::sync::atomic::AtomicUsize,
    drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    prepare_started: tokio_util::sync::CancellationToken,
    commit_started: tokio_util::sync::CancellationToken,
    commit_release: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

struct FakePrepared(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Drop for FakePrepared {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl super::worker::MarketBackend for RaceBackend {
    type Prepared = FakePrepared;

    async fn load(
        &self,
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<(gray_pkg::index::Index, std::collections::BTreeSet<String>)> {
        self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        anyhow::ensure!(self.behavior != Behavior::LoadFailed, "catalog unavailable");
        Ok((index(&["demo"]), Default::default()))
    }

    async fn prepare(
        &self,
        _: gray_pkg::staged::ApprovedEntry,
        cancel: tokio_util::sync::CancellationToken,
        progress: tokio::sync::mpsc::UnboundedSender<gray_pkg::staged::InstallStage>,
    ) -> anyhow::Result<(FakePrepared, super::ConsentView)> {
        let package = FakePrepared(self.drops.clone());
        let _ = progress.send(gray_pkg::staged::InstallStage::ReadingManifest);
        self.prepare_started.cancel();
        if self.behavior == Behavior::LateReady {
            cancel.cancelled().await;
        }
        assert!(self.behavior != Behavior::Panic, "private panic output");
        anyhow::ensure!(
            self.behavior != Behavior::PrepareFailed,
            "preparation failed"
        );
        Ok((package, consent_fixture()))
    }

    async fn commit(
        &self,
        _: FakePrepared,
    ) -> anyhow::Result<crate::plugin_install::InstallResult> {
        self.commits
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let release = self.commit_release.lock().unwrap().take();
        self.commit_started.cancel();
        if let Some(release) = release {
            release.await.unwrap();
        }
        anyhow::ensure!(self.behavior != Behavior::CommitFailed, "commit failed");
        Ok(crate::plugin_install::InstallResult {
            name: "demo".into(),
            installed: self.behavior != Behavior::AlreadyInstalled,
            warnings: if self.behavior == Behavior::Warning {
                vec!["manifest adoption failed".into()]
            } else {
                Vec::new()
            },
        })
    }

    async fn installed(&self) -> anyhow::Result<Option<std::collections::BTreeSet<String>>> {
        anyhow::ensure!(self.behavior != Behavior::RefreshFailed, "registry corrupt");
        Ok(Some(["demo".into(), "disabled-local".into()].into()))
    }

    fn record_failure(&self, _: &str, _: &str) {
        self.failures
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

fn send(
    worker: &super::worker::WorkerHandle,
    operation: u64,
    command: super::worker::MarketCommand,
) {
    worker
        .commands
        .send(super::worker::TaggedCommand { operation, command })
        .unwrap();
}

async fn next(worker: &mut super::worker::WorkerHandle) -> super::worker::TaggedEvent {
    tokio::time::timeout(std::time::Duration::from_secs(2), worker.events.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn ready(worker: &mut super::worker::WorkerHandle, operation: u64) {
    send(
        worker,
        operation,
        super::worker::MarketCommand::Prepare(Box::new(approved_demo())),
    );
    loop {
        let event = next(worker).await;
        assert_eq!(event.operation, operation);
        if let super::worker::MarketEvent::Ready(view) = event.event {
            assert!(!view.capabilities.is_empty());
            return;
        }
    }
}

async fn closed(mut worker: super::worker::WorkerHandle) {
    send(&worker, 0, super::worker::MarketCommand::Close);
    while !matches!(
        next(&mut worker).await.event,
        super::worker::MarketEvent::Closed
    ) {}
    worker.task.await.unwrap();
}

#[tokio::test]
async fn each_load_fetches_fresh_and_stale_loads_cannot_replace_it() {
    use super::worker::{MarketCommand, MarketEvent};
    let fake = std::sync::Arc::new(RaceBackend::default());
    let mut worker = super::worker::spawn_backend(fake.clone());
    for operation in [1, 2] {
        send(&worker, operation, MarketCommand::Load);
        let event = next(&mut worker).await;
        assert_eq!(event.operation, operation);
        let MarketEvent::Loaded(index, installed) = event.event else {
            panic!("expected catalog");
        };
        assert!(index.plugins.contains_key("demo"));
        assert!(installed.is_empty());
    }
    send(&worker, 1, MarketCommand::Load);
    closed(worker).await;
    assert_eq!(fake.loads.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancel_and_late_ready_cannot_install_or_replace_a_new_attempt() {
    use super::worker::{MarketCommand, MarketEvent};
    let fake = std::sync::Arc::new(RaceBackend {
        behavior: Behavior::LateReady,
        ..Default::default()
    });
    let mut worker = super::worker::spawn_backend(fake.clone());
    send(
        &worker,
        1,
        MarketCommand::Prepare(Box::new(approved_demo())),
    );
    fake.prepare_started.cancelled().await;
    send(&worker, 1, MarketCommand::Cancel);
    send(&worker, 2, MarketCommand::Load);
    loop {
        let event = next(&mut worker).await;
        assert!(!matches!(event.event, MarketEvent::Ready(_)));
        if matches!(event.event, MarketEvent::Loaded(_, _)) {
            assert_eq!(event.operation, 2);
            break;
        }
    }
    send(&worker, 1, MarketCommand::Grant);
    closed(worker).await;
    assert_eq!(fake.commits.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(fake.drops.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(fake.failures.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn consent_cancel_drops_prepared_and_grant_cannot_resurrect_it() {
    use super::worker::{MarketCommand, MarketEvent};
    let fake = std::sync::Arc::new(RaceBackend::default());
    let mut worker = super::worker::spawn_backend(fake.clone());
    ready(&mut worker, 1).await;
    send(&worker, 1, MarketCommand::Cancel);
    assert!(matches!(
        next(&mut worker).await.event,
        MarketEvent::Cancelled
    ));
    send(&worker, 1, MarketCommand::Grant);
    closed(worker).await;
    assert_eq!(fake.commits.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(fake.drops.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn close_during_commit_waits_and_duplicate_grant_never_installs_twice() {
    use super::worker::{MarketCommand, MarketEvent};
    let (release, barrier) = tokio::sync::oneshot::channel();
    let fake = std::sync::Arc::new(RaceBackend {
        commit_release: std::sync::Mutex::new(Some(barrier)),
        ..Default::default()
    });
    let mut worker = super::worker::spawn_backend(fake.clone());
    ready(&mut worker, 1).await;
    send(&worker, 0, MarketCommand::Grant); // stale consent cannot commit
    send(&worker, 1, MarketCommand::Grant);
    fake.commit_started.cancelled().await;
    send(&worker, 1, MarketCommand::Grant);
    send(&worker, 1, MarketCommand::Cancel);
    send(&worker, 1, MarketCommand::Close);
    assert!(!worker.task.is_finished());
    release.send(()).unwrap();
    let mut committed = false;
    let mut refreshed = false;
    loop {
        match next(&mut worker).await.event {
            MarketEvent::Registry(installed) => {
                assert!(installed.contains("disabled-local"));
                refreshed = true;
            }
            MarketEvent::Committed(result) => {
                assert!(result.installed);
                committed = true;
            }
            MarketEvent::Closed => break,
            MarketEvent::Cancelled => panic!("commit cannot be cancelled"),
            _ => {}
        }
    }
    assert!(committed && refreshed);
    worker.task.await.unwrap();
    assert_eq!(fake.commits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failures_and_backend_panics_are_visible_and_allow_closing() {
    use super::worker::{MarketCommand, MarketEvent};
    for behavior in [
        Behavior::LoadFailed,
        Behavior::PrepareFailed,
        Behavior::Panic,
        Behavior::CommitFailed,
    ] {
        let fake = std::sync::Arc::new(RaceBackend {
            behavior,
            ..Default::default()
        });
        let mut worker = super::worker::spawn_backend(fake.clone());
        if behavior == Behavior::CommitFailed {
            ready(&mut worker, 1).await;
            send(&worker, 1, MarketCommand::Grant);
        } else {
            send(
                &worker,
                1,
                if behavior == Behavior::LoadFailed {
                    MarketCommand::Load
                } else {
                    MarketCommand::Prepare(Box::new(approved_demo()))
                },
            );
        }
        loop {
            if let MarketEvent::Failed(message) = next(&mut worker).await.event {
                assert!(!message.contains("private panic output"));
                break;
            }
        }
        closed(worker).await;
        assert_eq!(
            fake.failures.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(behavior != Behavior::LoadFailed)
        );
    }
}

#[tokio::test]
async fn committed_warnings_and_registry_failure_are_success_not_install_failure() {
    use super::worker::{MarketCommand, MarketEvent};
    for behavior in [
        Behavior::Warning,
        Behavior::AlreadyInstalled,
        Behavior::RefreshFailed,
    ] {
        let fake = std::sync::Arc::new(RaceBackend {
            behavior,
            ..Default::default()
        });
        let mut worker = super::worker::spawn_backend(fake.clone());
        ready(&mut worker, 1).await;
        send(&worker, 1, MarketCommand::Grant);
        loop {
            match next(&mut worker).await.event {
                MarketEvent::Committed(result) => {
                    assert_eq!(result.installed, behavior != Behavior::AlreadyInstalled);
                    assert_eq!(
                        result.warnings.is_empty(),
                        behavior == Behavior::AlreadyInstalled
                    );
                    break;
                }
                MarketEvent::Failed(_) => panic!("committed package cannot fail retroactively"),
                _ => {}
            }
        }
        closed(worker).await;
        assert_eq!(fake.failures.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn losing_command_channel_cancels_and_joins_preparation() {
    let fake = std::sync::Arc::new(FakeBackend::default());
    let worker = super::worker::spawn_backend(fake.clone());
    send(
        &worker,
        1,
        super::worker::MarketCommand::Prepare(Box::new(approved_demo())),
    );
    fake.prepare_started.cancelled().await;
    let super::worker::WorkerHandle {
        commands,
        events: mut events_rx,
        task,
    } = worker;
    drop(commands);
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events_rx.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(event.event, super::worker::MarketEvent::Closed) {
            break;
        }
    }
    task.await.unwrap();
    assert!(fake.prepare_cancelled.is_cancelled());
    assert_eq!(fake.commits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

async fn finish_market(mut market: super::Marketplace) -> crate::setup::PluginModalOutcome {
    use super::ManagerExtension;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !market.request_close() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    market.into_outcome()
}

#[tokio::test]
async fn modal_outcome_contains_only_new_commits_and_preserves_warnings() {
    use super::worker::{MarketEvent, TaggedEvent};
    use crate::plugin_install::InstallResult;
    let mut market = super::Marketplace::with_worker(super::worker::spawn_backend(
        std::sync::Arc::new(RaceBackend::default()),
    ));
    market.model.operation = 7;
    for (operation, name, installed) in [
        (6, "stale", true),
        (7, "existing", false),
        (7, "new", true),
        (7, "new", true),
    ] {
        market.event(TaggedEvent {
            operation,
            event: MarketEvent::Committed(InstallResult {
                name: name.into(),
                installed,
                warnings: if name == "new" {
                    vec!["adoption warning".into()]
                } else {
                    vec![]
                },
            }),
        });
    }
    assert!(market.model.installed.contains("existing"));
    assert!(market.model.installed.contains("new"));
    assert!(!market.model.installed.contains("stale"));
    let outcome = finish_market(market).await;
    assert!(outcome.changed);
    assert_eq!(outcome.installed, ["new"]);
    assert!(outcome.warnings.contains(&"adoption warning".to_string()));
}

#[tokio::test]
async fn late_ready_and_enter_repeat_never_grant_and_tabs_are_blocked_during_install() {
    use super::ManagerExtension;
    use super::model::MarketPhase;
    use super::worker::{MarketEvent, TaggedEvent};
    use crossterm::event::{KeyEvent, KeyEventKind, KeyModifiers};
    let fake = std::sync::Arc::new(RaceBackend::default());
    let mut market = super::Marketplace::with_worker(super::worker::spawn_backend(fake.clone()));
    market.model.operation = 2;
    market.model.phase = MarketPhase::Preparing;
    market.event(TaggedEvent {
        operation: 1,
        event: MarketEvent::Ready(consent_fixture()),
    });
    assert!(market.consent.is_none());
    market.event(TaggedEvent {
        operation: 2,
        event: MarketEvent::Ready(consent_fixture()),
    });
    assert_eq!(market.model.phase, MarketPhase::Consent);
    assert!(market.key(KeyEvent::new_with_kind(
        KeyCode::Enter,
        KeyModifiers::NONE,
        KeyEventKind::Repeat
    )));
    assert_eq!(market.model.phase, MarketPhase::Consent);
    assert!(market.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    assert!(market.model.grant_selected);
    market.model.phase = MarketPhase::Committing;
    assert!(market.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    assert!(market.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert_eq!(market.model.phase, MarketPhase::Committing);
    assert_eq!(fake.commits.load(std::sync::atomic::Ordering::SeqCst), 0);
    let outcome = finish_market(market).await;
    assert!(outcome.installed.is_empty());
}

#[tokio::test]
async fn search_keys_are_consumed_before_outer_navigation_and_load_error_goes_back() {
    use super::ManagerExtension;
    use super::model::MarketPhase;
    use crossterm::event::{KeyEvent, KeyModifiers};
    let mut market = super::Marketplace::with_worker(super::worker::spawn_backend(
        std::sync::Arc::new(RaceBackend::default()),
    ));
    market.model.loaded(index(&["jack"]), Default::default());
    for key in ['/', 'j', 'k'] {
        assert!(market.key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)));
    }
    assert_eq!(market.model.query, "jk");
    assert!(market.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    market.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!market.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    market.model.phase = MarketPhase::LoadError;
    assert!(market.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(market.take_back());
    assert!(!market.model.close_requested);
    finish_market(market).await;
}

#[tokio::test]
async fn modal_close_during_prepare_waits_and_ignores_a_late_ready() {
    use super::ManagerExtension;
    use super::model::MarketPhase;
    use crossterm::event::{KeyEvent, KeyModifiers};
    let fake = std::sync::Arc::new(RaceBackend {
        behavior: Behavior::LateReady,
        ..Default::default()
    });
    let mut market = super::Marketplace::with_worker(super::worker::spawn_backend(fake.clone()));
    market.model.loaded(index(&["demo"]), Default::default());
    market.model.confirm();
    assert!(market.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert_eq!(market.model.phase, MarketPhase::Preparing);
    fake.prepare_started.cancelled().await;
    let outcome = finish_market(market).await;
    assert!(outcome.installed.is_empty());
    assert_eq!(fake.drops.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(fake.commits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn finished_worker_results_are_drained_before_modal_outcome_is_returned() {
    use super::ManagerExtension;
    use super::worker::{MarketEvent, TaggedEvent, WorkerHandle};
    let (commands, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (events_tx, events) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        events_tx
            .send(TaggedEvent {
                operation: 1,
                event: MarketEvent::Committed(crate::plugin_install::InstallResult {
                    name: "demo".into(),
                    installed: true,
                    warnings: vec![],
                }),
            })
            .unwrap();
        events_tx
            .send(TaggedEvent {
                operation: 1,
                event: MarketEvent::Closed,
            })
            .unwrap();
    });
    let mut market = super::Marketplace::with_worker(WorkerHandle {
        commands,
        events,
        task,
    });
    market.model.operation = 1;
    market.model.close_requested = true;
    while !market.worker.task.is_finished() {
        tokio::task::yield_now().await;
    }
    market.tick();
    assert!(market.closed && market.joined);
    assert_eq!(market.model.phase, super::model::MarketPhase::Installed);
    assert_eq!(market.into_outcome().installed, ["demo"]);
}

#[tokio::test]
async fn registry_loading_counts_disabled_and_local_keys_and_fails_closed() {
    use super::worker::MarketBackend;
    let home = tempfile::tempdir().unwrap();
    let backend = super::worker::Backend {
        home: home.path().into(),
    };
    let path = gray_plugin::lock::lock_path(home.path());
    let mut lock = gray_plugin::lock::LockFile::load(&path).unwrap();
    lock.plugins.insert(
        "demo".into(),
        gray_plugin::lock::LockEntry {
            enabled: false,
            version: "0.0.1".into(),
            ..Default::default()
        },
    );
    lock.plugins.insert(
        "local".into(),
        gray_plugin::lock::LockEntry {
            ecosystem: "gray-cli".into(),
            argv: vec!["/local/path".into()],
            ..Default::default()
        },
    );
    lock.save(&path).unwrap();
    assert_eq!(
        backend.installed().await.unwrap().unwrap(),
        ["demo".into(), "local".into()].into()
    );
    std::fs::write(&path, b"corrupt registry").unwrap();
    assert!(backend.installed().await.is_err());
    std::fs::write(&path, br#"{"schema":999,"plugins":{}}"#).unwrap();
    assert!(backend.installed().await.is_err());
}

#[tokio::test]
async fn modal_close_during_commit_returns_real_install_outcome_before_finishing() {
    use super::ManagerExtension;
    use super::model::MarketPhase;
    use crossterm::event::{KeyEvent, KeyModifiers};
    let (release, barrier) = tokio::sync::oneshot::channel();
    let fake = std::sync::Arc::new(RaceBackend {
        behavior: Behavior::Warning,
        commit_release: std::sync::Mutex::new(Some(barrier)),
        ..Default::default()
    });
    let mut market = super::Marketplace::with_worker(super::worker::spawn_backend(fake.clone()));
    market.model.loaded(index(&["demo"]), Default::default());
    market.model.confirm();
    market.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    while market.model.phase != MarketPhase::Consent {
        let event = next(&mut market.worker).await;
        market.event(event);
    }
    market.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    fake.commit_started.cancelled().await;
    assert!(!market.request_close());
    release.send(()).unwrap();
    let outcome = finish_market(market).await;
    assert_eq!(outcome.installed, ["demo"]);
    assert_eq!(outcome.warnings, ["manifest adoption failed"]);
    assert!(outcome.changed);
    assert_eq!(fake.commits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn unsupported_catalog_packages_cannot_start_installation() {
    let mut model = super::model::MarketModel::default();
    let mut catalog = index(&["demo"]);
    catalog.plugins.get_mut("demo").unwrap().kind = "skill".into();
    model.loaded(catalog, Default::default());
    assert!(!model.can_install());
    model.input(KeyCode::Char('i'));
    assert_eq!(model.phase, super::model::MarketPhase::Browsing);
    let screen = compact(&snapshot(&model, 100, 20)).join(" ");
    assert!(screen.contains("not supported by the"), "{screen}");
}

#[test]
fn long_catalog_names_do_not_hide_trust_or_installed_badges() {
    let name = "long-plugin-name-".repeat(15);
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&[&name]), [name].into());
    for width in [80, 90, 110] {
        let screen = snapshot(&model, width, 12);
        let row = screen.lines().find(|l| l.contains('…')).unwrap();
        assert!(row.starts_with("> ○ long-plugin-name"), "{screen}");
        assert!(row.contains("… 1.0.0 ✓"), "{screen}");
    }
}

#[test]
fn details_wrap_at_word_boundaries_and_installed_mark_is_green() {
    let mut model = super::model::MarketModel::default();
    let mut catalog = index(&["demo"]);
    catalog.plugins.get_mut("demo").unwrap().description = Some(format!(
        "{}enrollment-code exchange, whoami, logout.",
        "Log in from the CLI: ".repeat(3)
    ));
    model.loaded(catalog, ["demo".into()].into());
    for width in [90, 100, 110] {
        let screen = snapshot(&model, width, 20);
        assert!(screen.contains("enrollment-code"), "{screen}");
    }
    model.loaded(index(&["alpha", "demo"]), ["demo".into()].into());
    let backend = ratatui::backend::TestBackend::new(110, 20);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| super::draw::render(frame, frame.area(), &model, None))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let mark = (0..20)
        .flat_map(|y| (0..110).map(move |x| (x, y)))
        .find(|&(x, y)| buffer[(x, y)].symbol() == "✓")
        .unwrap();
    assert_eq!(buffer[mark].fg, crate::theme::theme().success);
}

#[test]
fn enter_and_i_confirm_directly_but_installed_plugins_do_nothing() {
    use super::model::{MarketModel, MarketPhase};
    for key in [KeyCode::Enter, KeyCode::Char('i')] {
        let mut model = MarketModel::default();
        model.loaded(index(&["demo"]), Default::default());
        model.input(key);
        assert_eq!(model.phase, MarketPhase::Confirm);
        model.loaded(index(&["demo"]), ["demo".into()].into());
        model.input(key);
        assert_eq!(model.phase, MarketPhase::Browsing);
    }
}

#[test]
fn selected_details_are_always_visible_in_a_right_side_pane() {
    let mut model = super::model::MarketModel::default();
    let mut catalog = index(&["alpha", "beta"]);
    catalog.plugins.get_mut("alpha").unwrap().description = Some("Alpha description".into());
    catalog.plugins.get_mut("beta").unwrap().description = Some("Beta description".into());
    model.loaded(catalog, Default::default());
    for width in [90, 110] {
        let screen = snapshot(&model, width, 20);
        let row = screen
            .lines()
            .find(|line| line.contains("> ○ alpha"))
            .unwrap();
        assert_eq!(row.matches("alpha").count(), 2, "{screen}");
        assert!(screen.contains("Alpha description"), "{screen}");
    }
    model.input(KeyCode::PageDown);
    assert!(model.scroll > 0);
    model.input(KeyCode::Down);
    assert_eq!(model.scroll, 0);
    let screen = snapshot(&model, 110, 20);
    assert!(screen.contains("> ○ beta"), "{screen}");
    assert!(screen.contains("Beta description"), "{screen}");
    assert!(!screen.contains("Alpha description"), "{screen}");
    assert!(
        screen.contains("←→ tabs · / search · ↑↓/jk select · Enter install"),
        "{screen}"
    );
    assert!(!screen.contains("Enter details"));
}

#[test]
fn narrow_browsing_stacks_details_below_the_list() {
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["alpha", "beta"]), Default::default());
    for width in [40, 89] {
        let screen = snapshot(&model, width, 20);
        let list_y = screen
            .lines()
            .position(|line| line.contains("> ○ alpha"))
            .unwrap();
        let description_y = screen
            .lines()
            .position(|line| line.contains("alpha helper"))
            .unwrap();
        assert!(description_y > list_y, "{screen}");
        assert!(
            screen
                .lines()
                .nth(description_y)
                .unwrap()
                .starts_with("alpha helper"),
            "{screen}"
        );
    }
}

#[tokio::test]
async fn search_focus_does_not_capture_left_right_tab_keys() {
    use super::ManagerExtension;
    use crossterm::event::{KeyEvent, KeyModifiers};
    let mut market = super::Marketplace::with_worker(super::worker::spawn_backend(
        std::sync::Arc::new(RaceBackend::default()),
    ));
    market.model.loaded(index(&["demo"]), Default::default());
    market.model.search_focused = true;
    for key in [KeyCode::Left, KeyCode::Right] {
        assert!(!market.key(KeyEvent::new(key, KeyModifiers::NONE)));
        assert!(market.model.query.is_empty());
    }
    for phase in [
        super::model::MarketPhase::Confirm,
        super::model::MarketPhase::Preparing,
        super::model::MarketPhase::Committing,
    ] {
        market.model.phase = phase;
        for key in [KeyCode::Left, KeyCode::Right] {
            assert!(market.key(KeyEvent::new(key, KeyModifiers::NONE)));
            assert_eq!(market.model.phase, phase);
        }
    }
    market.model.phase = super::model::MarketPhase::Consent;
    market.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(market.model.grant_selected);
    finish_market(market).await;
}

#[tokio::test]
async fn stacked_list_uses_its_actual_height_to_keep_the_last_selection_visible() {
    use super::ManagerExtension;
    use ratatui::{Terminal, backend::TestBackend};
    let mut market = super::Marketplace::with_worker(super::worker::spawn_backend(
        std::sync::Arc::new(RaceBackend::default()),
    ));
    let names = (0..150)
        .map(|i| format!("plugin-{i:03}"))
        .collect::<Vec<_>>();
    market.model.loaded(
        index(&names.iter().map(String::as_str).collect::<Vec<_>>()),
        Default::default(),
    );
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| market.draw(frame, frame.area()))
        .unwrap();
    assert_eq!(market.model.viewport_rows, 5);
    for _ in 0..149 {
        market.model.input(KeyCode::Down);
    }
    assert_eq!(market.model.offset, 145);
    terminal
        .draw(|frame| market.draw(frame, frame.area()))
        .unwrap();
    let row = (0..80)
        .map(|x| terminal.backend().buffer()[(x, 7)].symbol())
        .collect::<String>();
    assert!(row.contains("> ○ plugin-149"), "{row}");
    finish_market(market).await;
}
