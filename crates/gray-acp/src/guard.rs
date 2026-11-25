pub const RECORDS: &[&str] = &[
    "User: ",
    "Assistant: ",
    "System: ",
    "System instructions:",
    "Assistant called tool ",
    "Tool result for ",
];

pub const LABEL: &str = "Assistant: ";

pub const ONE_TURN_RULE: &str = "You write exactly ONE assistant turn. Never write `User:`, \
    `Tool result for …:` or any other transcript record yourself; tool results only come from \
    the harness. Never claim a command ran unless its `Tool result` is in the transcript.";

pub const CUT_NOTE: &str = "[Harness note] Your previous reply was cut where you started writing \
    a transcript record yourself; everything from that point on was discarded, never delivered, \
    and none of it ran. Write exactly one assistant turn.";

#[derive(Clone, Debug)]
pub struct TurnGuard {
    records: &'static [&'static str],
    label: Option<&'static str>,
    scanned: usize,
    seen_text: bool,
    cut: Option<&'static str>,
}

impl Default for TurnGuard {
    fn default() -> Self {
        Self::new(RECORDS, Some(LABEL))
    }
}

impl TurnGuard {
    pub fn new(records: &'static [&'static str], label: Option<&'static str>) -> Self {
        Self {
            records,
            label,
            scanned: 0,
            seen_text: false,
            cut: None,
        }
    }

    pub fn cut(&self) -> Option<&'static str> {
        self.cut
    }

    pub fn new_segment(&mut self) {
        self.scanned = 0;
    }

    pub fn feed(&mut self, text: &mut String) -> Option<&'static str> {
        if self.cut.is_some() {
            return None;
        }
        self.scanned = self.scanned.min(text.len());
        loop {
            let rest = &text[self.scanned..];
            let (line, complete) = match rest.find('\n') {
                Some(i) => (&rest[..i], true),
                None => (rest, false),
            };
            let head = line.trim_start_matches([' ', '\t']);
            if let Some(label) = self.label
                && head.starts_with(label)
                && !self.seen_text
                && text[..self.scanned].trim().is_empty()
            {
                let at = self.scanned + (line.len() - head.len());
                text.replace_range(at..at + label.len(), "");
                continue;
            }
            if let Some(m) = self.records.iter().find(|m| head.starts_with(**m)) {
                text.truncate(self.scanned);
                self.cut = Some(*m);
                return self.cut;
            }
            if !complete {
                return None;
            }
            if !head.trim().is_empty() {
                self.seen_text = true;
            }
            self.scanned += line.len() + 1;
        }
    }
}

pub fn cut_transcript(
    text: &str,
    records: &'static [&'static str],
    label: Option<&'static str>,
) -> (String, Option<&'static str>) {
    let mut out = text.to_string();
    let cut = TurnGuard::new(records, label).feed(&mut out);
    (out, cut)
}

pub fn strip_reasoning(text: &str) -> String {
    let mut out = text.to_string();
    for (open, close) in [("<think>", "</think>"), ("<thinking>", "</thinking>")] {
        while let Some(a) = out.find(open) {
            let Some(rel) = out[a + open.len()..].find(close) else {
                break;
            };
            let b = a + open.len() + rel + close.len();
            out.replace_range(a..b, "");
        }
    }
    for tag in [
        "<think>",
        "</think>",
        "<thinking>",
        "</thinking>",
        "<response>",
        "</response>",
    ] {
        out = out.replace(tag, "");
    }
    out.replace("thinkresponse", "\n\n")
}

pub fn clean_items(items: &mut Vec<crate::chat::OutItem>) {
    for item in items.iter_mut() {
        if let crate::chat::OutItem::Message { text } = item {
            *text = strip_reasoning(text).trim().to_string();
        }
    }
    items.retain(|i| !matches!(i, crate::chat::OutItem::Message { text } if text.is_empty()));
}

pub fn marker_name(m: &str) -> String {
    m.trim_matches(|c: char| c == '[' || c == ']' || c == ':' || c == '#' || c.is_whitespace())
        .replace(' ', "_")
}
