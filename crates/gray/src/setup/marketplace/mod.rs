pub(crate) mod activation;
mod draw;
mod model;
mod worker;

use super::install_manager::{ManagerExtension, PluginModalOutcome};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::FutureExt;
use gray_pkg::staged::InstallStage;
use model::{Action, MarketModel, MarketPhase};
use worker::{MarketCommand, MarketEvent, TaggedCommand, TaggedEvent, WorkerHandle};

#[derive(Clone, Debug, Default)]
pub(crate) struct ConsentView {
    pub capabilities: Vec<crate::plugin_install::policy::CapabilityLine>,
    pub commands: Vec<String>,
    pub scan_text: String,
}

pub(super) struct Marketplace {
    model: MarketModel,
    consent: Option<ConsentView>,
    worker: WorkerHandle,
    joined: bool,
    closed: bool,
    back: bool,
    outcome: PluginModalOutcome,
}

impl Marketplace {
    pub fn new(home: std::path::PathBuf) -> Self {
        Self::with_worker(worker::spawn_backend(std::sync::Arc::new(
            worker::Backend { home },
        )))
    }

    fn with_worker(worker: WorkerHandle) -> Self {
        Self {
            model: MarketModel::default(),
            consent: None,
            worker,
            joined: false,
            closed: false,
            back: false,
            outcome: PluginModalOutcome::default(),
        }
    }

    fn send(&mut self, command: MarketCommand) {
        if self
            .worker
            .commands
            .send(TaggedCommand {
                operation: self.model.operation,
                command,
            })
            .is_err()
        {
            self.fail("Marketplace worker unavailable".into());
        }
    }

    fn load(&mut self) {
        self.model.begin(MarketPhase::Loading);
        self.model.loading_since = Some(std::time::Instant::now());
        self.model.search_focused = false;
        self.consent = None;
        self.send(MarketCommand::Load);
    }

    fn fail(&mut self, message: String) {
        self.model.phase = if self.model.phase == MarketPhase::Loading {
            MarketPhase::LoadError
        } else {
            MarketPhase::InstallFailed
        };
        self.model.message = Some(draw::safe_text(&message));
        self.model.scroll = 0;
        self.consent = None;
    }

    fn event(&mut self, tagged: TaggedEvent) {
        if matches!(tagged.event, MarketEvent::Closed) {
            self.closed = true;
            return;
        }
        if !self.model.accept_operation(tagged.operation) {
            return;
        }
        match tagged.event {
            MarketEvent::Loaded(index, installed) if !self.model.close_requested => {
                self.model.loaded(index, installed);
            }
            MarketEvent::Stage(stage) if !self.model.cancelling => self.model.stage = Some(stage),
            MarketEvent::Ready(view)
                if self.model.phase == MarketPhase::Preparing
                    && !self.model.cancelling
                    && !self.model.close_requested =>
            {
                self.consent = Some(view);
                self.model.phase = MarketPhase::Consent;
                self.model.grant_selected = false;
                self.model.scroll = 0;
            }
            MarketEvent::Registry(installed) => self.model.installed = installed,
            MarketEvent::Committed(result) => {
                // Keep the committed key disabled even if the registry refresh failed.
                self.model.installed.insert(result.name.clone());
                if result.installed && !self.outcome.installed.contains(&result.name) {
                    self.outcome.installed.push(result.name.clone());
                    self.outcome.changed = true;
                }
                let mut message = if result.installed {
                    format!("{} installed", result.name)
                } else {
                    format!("{} is already installed; no files replaced", result.name)
                };
                for warning in result.warnings {
                    let warning = draw::safe_text(&warning);
                    message.push_str(&format!("\nWarning: {warning}"));
                    self.outcome.warnings.push(warning);
                }
                self.model.phase = MarketPhase::Installed;
                self.model.message = Some(message);
                self.model.scroll = 0;
                self.consent = None;
            }
            MarketEvent::Failed(message) => self.fail(message),
            MarketEvent::Cancelled => {
                self.consent = None;
                self.model.back_to_browsing();
            }
            _ => {}
        }
    }

    pub fn shutdown(&mut self) {
        while !self.request_close() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    pub fn into_outcome(self) -> PluginModalOutcome {
        self.outcome
    }
}

impl ManagerExtension for Marketplace {
    fn enter(&mut self) {
        self.load();
    }

    fn tick(&mut self) {
        self.model.spinner = self.model.spinner.wrapping_add(1);
        while let Ok(event) = self.worker.events.try_recv() {
            self.event(event);
        }
        if !self.joined && self.worker.task.is_finished() {
            // Completion can race the first drain. Consume its final result
            // and Closed before deciding that the channel failed.
            while let Ok(event) = self.worker.events.try_recv() {
                self.event(event);
            }
            // Poll only an already-finished task. This joins without nesting
            // block_on on an executor thread or detaching a live install.
            if let Some(result) = (&mut self.worker.task).now_or_never() {
                self.joined = true;
                if result.is_err() || !self.closed {
                    self.fail("Marketplace worker stopped unexpectedly".into());
                    self.closed = true;
                }
            }
        }
    }

    fn draw(&mut self, frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect) {
        self.model.viewport_rows = usize::from(draw::browsing_panes(area).0.height).max(1);
        self.model.move_selection(0, self.model.viewport_rows);
        draw::render(frame, area, &self.model, self.consent.as_ref());
    }

    fn key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return true;
        }
        if matches!(key.code, KeyCode::Left | KeyCode::Right) && !self.model.blocks_tabs() {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('c') {
                return false;
            }
            if matches!(key.code, KeyCode::Char('n' | 'p')) {
                return self.model.blocks_tabs() || self.model.search_focused;
            }
            return true;
        }
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
            && !self.model.blocks_tabs()
            && !self.model.search_focused
        {
            return false;
        }
        match self.model.input(key.code) {
            Action::Load => self.load(),
            Action::Prepare => {
                if let Some(approved) = self.model.approved.clone() {
                    self.model.begin(MarketPhase::Preparing);
                    self.model.stage = Some(InstallStage::Downloading);
                    self.send(MarketCommand::Prepare(Box::new(approved)));
                }
            }
            Action::Grant => {
                self.model.phase = MarketPhase::Committing;
                self.model.scroll = 0;
                self.model.stage = Some(InstallStage::Committing);
                self.send(MarketCommand::Grant);
            }
            Action::Cancel => {
                self.model.cancelling = true;
                self.model.phase = MarketPhase::Preparing;
                self.send(MarketCommand::Cancel);
            }
            Action::BackTab => self.back = true,
            Action::Close => return false,
            Action::None => {}
        }
        true
    }

    fn request_close(&mut self) -> bool {
        if !self.model.close_requested {
            self.model.close_requested = true;
            self.send(MarketCommand::Close);
        }
        self.tick();
        self.closed && self.joined
    }

    fn take_back(&mut self) -> bool {
        std::mem::take(&mut self.back)
    }

    fn animating(&self) -> bool {
        self.model.phase == MarketPhase::Loading
    }
}

#[cfg(test)]
mod tests;
