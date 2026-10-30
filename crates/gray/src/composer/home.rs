use super::*;

impl Tui {
    pub(crate) fn start_fresh_conversation(&mut self) {
        crate::mascot::set_mascot_shown(false);
        self.history_entries = vec![TranscriptEntry::Welcome];
        self.left_home = false;
        self.reflow_on_resize(self.last_width);
    }

    pub fn set_model(&mut self, model: String) {
        self.model_name = model;
    }

    pub fn set_home_notice(&mut self, notice: Option<String>) {
        self.home_notice = notice;
    }

    pub fn set_plugin_stats(&mut self, stats: Option<String>) {
        self.plugin_stats = stats;
    }

    pub fn set_home_update(&mut self, notice: Option<String>) {
        self.home_update = notice;
    }

    /// The centered home layout holds only while the transcript is empty
    /// of conversation. Local slash commands retract their echo
    /// (`retract_slash_echo`) right after submit, so a bare trailing echo
    /// is still home. Once a slash echo has output after it — a plugin
    /// command like `/cloud` printing its receipt — the session has started
    /// talking: drop to the chat layout instead of stacking transcript rows
    /// above a still-centered home composer.
    pub(crate) fn is_home(&self) -> bool {
        if self.left_home {
            return false;
        }
        let entries = &self.history_entries;
        entries.iter().enumerate().all(|(i, e)| match e {
            TranscriptEntry::Welcome
            | TranscriptEntry::Gap(_)
            | TranscriptEntry::Mascot
            | TranscriptEntry::StyledLines { .. } => true,
            TranscriptEntry::UserPrompt(text, _) => {
                text.starts_with('/')
                    && entries[i + 1..]
                        .iter()
                        .all(|later| matches!(later, TranscriptEntry::Gap(_)))
            }
            TranscriptEntry::ToolBox { .. }
            | TranscriptEntry::ToolBoxLines { .. }
            | TranscriptEntry::ThinkingRun(_) => false,
        })
    }

    pub fn set_model_label(&mut self, label: Option<String>) {
        self.model_label = label;
    }

    pub fn set_model_label_parts(&mut self, parts: Option<Vec<(String, Option<usize>)>>) {
        self.model_label_parts = parts;
    }

    pub fn set_cwd(&mut self, cwd: String) {
        self.cwd = cwd;
    }

    pub fn set_thinking_effort(&mut self, effort: String) {
        self.thinking_effort = effort;
    }

    pub(crate) fn clear_draft(&mut self) {
        self.textarea.set_text("");
        self.matches.clear();
        self.muted_text = None;
        self.search = None;
        self.sel = 0;
        self.history_idx = None;
        self.draft.clear();
        self.attachments.clear();
        self.pending_pastes.clear();
    }

    pub(crate) fn set_hint(&mut self, text: &str) {
        self.hint = Some((Instant::now(), text.to_string()));
        self.hint_line = None;
    }

    /// On the home screen a one-line command reply is a transient toast
    /// above the input box rather than a transcript row that would scroll
    /// the welcome banner away. False (nothing shown) off home or when the
    /// line is too wide for the home column.
    pub(crate) fn home_toast(&mut self, line: Line<'static>) -> bool {
        if !self.is_home() || self.intro_started.is_some() {
            return false;
        }
        let (_, col_w) = home_column(self.last_width as usize);
        if line.width() + 1 > col_w {
            return false;
        }
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        self.hint = Some((Instant::now(), text));
        self.hint_line = Some(line);
        let _ = self.draw();
        true
    }

    /// The hint while it is still fresh; None once HINT_TTL has elapsed.
    pub(crate) fn hint_active(&self) -> Option<&str> {
        match &self.hint {
            Some((shown, text)) if shown.elapsed() < HINT_TTL => Some(text.as_str()),
            _ => None,
        }
    }

    /// A stored hint that has outlived HINT_TTL and should be dropped.
    pub(crate) fn hint_expired(&self) -> bool {
        self.hint.is_some() && self.hint_active().is_none()
    }
}
