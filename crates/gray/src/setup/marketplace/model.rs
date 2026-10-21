use std::collections::{BTreeMap, BTreeSet};

use crossterm::event::KeyCode;
use gray_pkg::index::{Entry, Index};
use gray_pkg::staged::{ApprovedEntry, InstallStage};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarketPhase {
    #[default]
    Loading,
    LoadError,
    Browsing,
    Confirm,
    Preparing,
    Consent,
    Committing,
    Installed,
    InstallFailed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    None,
    Load,
    Prepare,
    Grant,
    Cancel,
    BackTab,
    Close,
}

#[derive(Default)]
pub struct MarketModel {
    pub phase: MarketPhase,
    pub entries: BTreeMap<String, Entry>,
    pub installed: BTreeSet<String>,
    pub query: String,
    pub search_focused: bool,
    pub selected: usize,
    pub offset: usize,
    pub operation: u64,
    pub message: Option<String>,
    pub close_requested: bool,
    pub approved: Option<ApprovedEntry>,
    pub stage: Option<InstallStage>,
    pub scroll: u16,
    pub viewport_rows: usize,
    pub spinner: usize,
    pub loading_since: Option<std::time::Instant>,
    pub grant_selected: bool,
    pub cancelling: bool,
}

impl MarketModel {
    pub fn loaded(&mut self, index: Index, installed: BTreeSet<String>) {
        self.entries = index.plugins;
        self.installed = installed;
        self.phase = MarketPhase::Browsing;
        self.message = None;
        self.reset_selection();
    }

    pub fn visible_names(&self) -> Vec<&str> {
        let query = self.query.to_lowercase();
        self.entries
            .iter()
            .filter(|(name, entry)| {
                name.to_lowercase().contains(&query)
                    || entry
                        .description
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
            })
            .map(|(name, _)| name.as_str())
            .collect()
    }

    pub fn selected_name(&self) -> Option<&str> {
        self.visible_names().get(self.selected).copied()
    }

    pub fn can_install(&self) -> bool {
        self.phase == MarketPhase::Browsing
            && self.selected_name().is_some_and(|name| {
                !self.installed.contains(name)
                    && ApprovedEntry::new(name.into(), self.entries[name].clone()).is_ok()
            })
    }

    pub fn move_selection(&mut self, delta: isize, rows: usize) {
        let len = self.visible_names().len();
        let previous = self.selected;
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
        if self.selected != previous {
            self.scroll = 0;
        }
        self.offset = self.offset.min(self.selected);
        if self.selected >= self.offset.saturating_add(rows.max(1)) {
            self.offset = self.selected + 1 - rows.max(1);
        }
    }

    pub fn begin(&mut self, phase: MarketPhase) -> u64 {
        self.operation = self.operation.wrapping_add(1);
        self.phase = phase;
        self.message = None;
        self.scroll = 0;
        self.cancelling = false;
        self.operation
    }

    pub fn accept_operation(&self, operation: u64) -> bool {
        self.operation == operation
    }

    pub fn blocks_tabs(&self) -> bool {
        matches!(
            self.phase,
            MarketPhase::Confirm
                | MarketPhase::Preparing
                | MarketPhase::Consent
                | MarketPhase::Committing
        )
    }

    fn reset_selection(&mut self) {
        self.selected = 0;
        self.offset = 0;
        self.scroll = 0;
    }

    pub fn confirm(&mut self) {
        if !self.can_install() {
            return;
        }
        if let Some(name) = self.selected_name()
            && let Ok(approved) = ApprovedEntry::new(name.into(), self.entries[name].clone())
        {
            self.approved = Some(approved);
            self.phase = MarketPhase::Confirm;
            self.scroll = 0;
        }
    }

    pub fn input(&mut self, code: KeyCode) -> Action {
        if self.close_requested {
            return Action::None;
        }
        if self.phase == MarketPhase::Browsing && self.search_focused {
            match code {
                KeyCode::Char(c) if !c.is_control() => self.query.push(c),
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Esc | KeyCode::Enter => {
                    self.search_focused = false;
                    return Action::None;
                }
                KeyCode::Tab | KeyCode::BackTab => return Action::None,
                _ => return Action::None,
            }
            self.reset_selection();
            return Action::None;
        }
        match code {
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(8);
                return Action::None;
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(8);
                return Action::None;
            }
            _ => {}
        }
        // Up/Down select catalog rows while browsing and scroll install screens.
        if self.phase != MarketPhase::Browsing {
            match code {
                KeyCode::Down => {
                    self.scroll = self.scroll.saturating_add(1);
                    return Action::None;
                }
                KeyCode::Up => {
                    self.scroll = self.scroll.saturating_sub(1);
                    return Action::None;
                }
                _ => {}
            }
        }
        match self.phase {
            MarketPhase::Browsing => match code {
                KeyCode::Char('/') => self.search_focused = true,
                KeyCode::Down | KeyCode::Char('j') => self.move_selection(1, self.viewport_rows),
                KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1, self.viewport_rows),
                KeyCode::Enter | KeyCode::Char('i') => self.confirm(),
                KeyCode::Esc => return Action::Close,
                _ => {}
            },
            MarketPhase::LoadError => match code {
                KeyCode::Char('r') | KeyCode::Enter => return Action::Load,
                KeyCode::Esc | KeyCode::Char('b') => return Action::BackTab,
                _ => {}
            },
            MarketPhase::Confirm => match code {
                KeyCode::Enter | KeyCode::Char('a') => return Action::Prepare,
                KeyCode::Esc | KeyCode::Char('b') => self.back_to_browsing(),
                _ => {}
            },
            MarketPhase::Preparing => {
                if code == KeyCode::Esc && !self.cancelling {
                    self.cancelling = true;
                    return Action::Cancel;
                }
            }
            MarketPhase::Consent => match code {
                KeyCode::Char('g') => return Action::Grant,
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                    self.grant_selected = !self.grant_selected;
                }
                KeyCode::Enter if self.grant_selected => return Action::Grant,
                KeyCode::Esc | KeyCode::Char('c') | KeyCode::Enter => return Action::Cancel,
                _ => {}
            },
            MarketPhase::Installed | MarketPhase::InstallFailed => match code {
                KeyCode::Char('r') if self.phase == MarketPhase::InstallFailed => {
                    self.phase = MarketPhase::Confirm;
                    self.scroll = 0;
                }
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('b') => self.back_to_browsing(),
                _ => {}
            },
            MarketPhase::Loading => {
                if code == KeyCode::Esc {
                    return Action::Close;
                }
            }
            MarketPhase::Committing => {}
        }
        Action::None
    }

    pub fn back_to_browsing(&mut self) {
        self.phase = MarketPhase::Browsing;
        self.scroll = 0;
        self.message = None;
        self.stage = None;
        self.cancelling = false;
    }
}
