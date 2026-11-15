use std::collections::{BTreeSet, HashMap};

use crate::compact::SUMMARY_ENVELOPE_PREFIX;
use crate::message::{ContentBlock, Message, Role};

pub const KEEP_TURNS: usize = 4;

const VERBATIM_BYTES: usize = 1500;

const RECORDS_MAX_BYTES: usize = 24 * 1024;

const ARGS_BYTES: usize = 100;

pub struct Turn {
    pub id: usize,
    pub user: String,
    pub reply: String,
    pub tools: Vec<usize>,
    pub handle: Option<String>,
}

pub struct ToolCall {
    pub id: usize,
    pub name: String,
    pub args: String,
    pub failed: bool,
    pub bytes: usize,
    pub turn: usize,
}

pub struct Earlier {
    pub id: usize,
    pub bytes: usize,
    pub handle: Option<String>,
}

#[derive(Default)]
pub struct Records {
    pub turns: Vec<Turn>,
    pub tools: Vec<ToolCall>,
    pub earlier: Vec<Earlier>,
    pub known: BTreeSet<String>,
    pub corpus: String,
}

pub fn is_real_user(m: &Message) -> bool {
    m.role == Role::User
        && !m.injected
        && m.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if !text.trim().is_empty()))
        && !m
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

fn text_of(m: &Message) -> String {
    m.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_summary(m: &Message) -> bool {
    m.role == Role::User && m.injected && text_of(m).starts_with(SUMMARY_ENVELOPE_PREFIX)
}

fn transcript(m: &Message, out: &mut String) {
    for b in &m.content {
        match b {
            ContentBlock::Text { text } => {
                out.push_str(if m.role == Role::User {
                    "user: "
                } else {
                    "assistant: "
                });
                out.push_str(text);
                out.push('\n');
            }
            ContentBlock::ToolUse { name, args, .. } => {
                out.push_str(&format!("tool call {name} {args}\n"));
            }
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                out.push_str(if *is_error {
                    "tool error: "
                } else {
                    "tool result: "
                });
                out.push_str(content);
                out.push('\n');
            }
            _ => {}
        }
    }
}

fn cut(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    format!("{}…", &s[..s.floor_char_boundary(n)])
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn prior_max(text: &str, kind: char) -> usize {
    text.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let rest = l.strip_prefix(kind)?;
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<usize>().ok()
        })
        .max()
        .unwrap_or(0)
}

fn failed_result(content: &str, is_error: bool) -> bool {
    is_error
        || content
            .lines()
            .next()
            .is_some_and(|l| l.starts_with("exit ") && !l.starts_with("exit 0"))
}

pub fn build(dropped: &[Message], kept: &[Message]) -> Records {
    let mut r = Records::default();
    let prior: String = dropped
        .iter()
        .filter(|m| is_summary(m))
        .map(text_of)
        .collect::<Vec<_>>()
        .join("\n");
    let (mut next_m, mut next_t, mut next_l) = (
        prior_max(&prior, 'M'),
        prior_max(&prior, 'T'),
        prior_max(&prior, 'L'),
    );
    for kind in ['M', 'T', 'L'] {
        for n in 1..=prior_max(&prior, kind) {
            r.known.insert(format!("{kind}{n}"));
        }
    }
    let mut corpus = String::new();
    for m in dropped.iter().chain(kept) {
        transcript(m, &mut corpus);
    }
    r.corpus = corpus;

    let mut pending: HashMap<String, usize> = HashMap::new();
    let mut current: Option<(Turn, String)> = None;
    let finish = |turn: Option<(Turn, String)>, r: &mut Records| {
        if let Some((mut t, full)) = turn {
            t.handle = crate::spill::store(&full);
            r.known.insert(format!("M{}", t.id));
            r.turns.push(t);
        }
    };
    for m in dropped {
        if is_summary(m) {
            next_l += 1;
            let text = text_of(m);
            r.known.insert(format!("L{next_l}"));
            r.earlier.push(Earlier {
                id: next_l,
                bytes: text.len(),
                handle: crate::spill::store(&text),
            });
            continue;
        }
        if is_real_user(m) {
            finish(current.take(), &mut r);
            next_m += 1;
            current = Some((
                Turn {
                    id: next_m,
                    user: text_of(m),
                    reply: String::new(),
                    tools: Vec::new(),
                    handle: None,
                },
                String::new(),
            ));
        }
        let Some((turn, full)) = current.as_mut() else {
            continue;
        };
        transcript(m, full);
        for b in &m.content {
            match b {
                ContentBlock::ToolUse { id, name, args, .. } => {
                    next_t += 1;
                    turn.tools.push(next_t);
                    pending.insert(id.clone(), r.tools.len());
                    r.known.insert(format!("T{next_t}"));
                    r.tools.push(ToolCall {
                        id: next_t,
                        name: name.clone(),
                        args: cut(&one_line(&args.to_string()), ARGS_BYTES),
                        failed: false,
                        bytes: 0,
                        turn: turn.id,
                    });
                }
                ContentBlock::ToolResult {
                    id,
                    content,
                    is_error,
                } => {
                    if let Some(&i) = pending.get(id) {
                        r.tools[i].failed = failed_result(content, *is_error);
                        r.tools[i].bytes = content.len();
                    }
                }
                ContentBlock::Text { text }
                    if m.role == Role::Assistant && !text.trim().is_empty() =>
                {
                    turn.reply = text.clone();
                }
                _ => {}
            }
        }
    }
    finish(current.take(), &mut r);
    r
}

impl Records {
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty() && self.earlier.is_empty()
    }

    fn tool_range(t: &Turn) -> String {
        match (t.tools.first(), t.tools.last()) {
            (Some(a), Some(b)) if a == b => format!(" · T{a}"),
            (Some(a), Some(b)) => format!(" · T{a}–T{b}"),
            _ => String::new(),
        }
    }

    pub fn index(&self) -> String {
        let mut out = String::new();
        for l in &self.earlier {
            out.push_str(&format!("L{}: earlier checkpoint\n", l.id));
        }
        for t in &self.turns {
            out.push_str(&format!(
                "M{}: {}{}\n",
                t.id,
                cut(&one_line(&t.user), 80),
                Self::tool_range(t)
            ));
        }
        for c in &self.tools {
            out.push_str(&format!(
                "T{} {} {} → {}\n",
                c.id,
                c.name,
                cut(&c.args, 60),
                if c.failed { "failed" } else { "ok" }
            ));
        }
        out
    }

    pub fn render(&self) -> String {
        let mut head = String::from(
            "Dropped turns (M#), tool calls (T#) and earlier checkpoints (L#). User messages and final replies are word for word up to the shown cut. Read a whole record with `gray spill head <spill>` (or tail / grep). A summary line tagged [check: …] could not be confirmed against these records: confirm it before relying on it.\n",
        );
        for l in &self.earlier {
            head.push_str(&format!(
                "L{} earlier checkpoint, {}{}\n",
                l.id,
                crate::spill::fmt_bytes(l.bytes),
                l.handle
                    .as_deref()
                    .map(|h| format!(" · spill {h}"))
                    .unwrap_or_default()
            ));
        }
        for t in &self.turns {
            head.push_str(&format!(
                "M{}{}{}\n  user: {}\n",
                t.id,
                Self::tool_range(t),
                t.handle
                    .as_deref()
                    .map(|h| format!(" · spill {h}"))
                    .unwrap_or_default(),
                cut(t.user.trim(), VERBATIM_BYTES).replace('\n', "\n  ")
            ));
            if !t.reply.trim().is_empty() {
                head.push_str(&format!(
                    "  reply: {}\n",
                    cut(t.reply.trim(), VERBATIM_BYTES).replace('\n', "\n  ")
                ));
            }
        }
        let mut tools = String::new();
        let mut omitted = 0;
        for c in &self.tools {
            let line = format!(
                "T{} {} {} → {} {} (M{})\n",
                c.id,
                c.name,
                c.args,
                if c.failed { "failed" } else { "ok" },
                crate::spill::fmt_bytes(c.bytes),
                c.turn
            );
            if head.len() + tools.len() + line.len() > RECORDS_MAX_BYTES {
                omitted += 1;
                continue;
            }
            tools.push_str(&line);
        }
        if omitted > 0 {
            tools.push_str(&format!(
                "({omitted} more tool calls: see their turn's spill)\n"
            ));
        }
        head + &tools
    }

    pub fn lint(&self, summary: &str) -> String {
        let failed: BTreeSet<String> = self
            .tools
            .iter()
            .filter(|t| t.failed)
            .map(|t| format!("T{}", t.id))
            .collect();
        summary
            .lines()
            .map(|line| {
                let notes = self.check_line(line, &failed);
                if notes.is_empty() {
                    line.to_string()
                } else {
                    format!("{line} [check: {}]", notes.join("; "))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn check_line(&self, line: &str, failed: &BTreeSet<String>) -> Vec<String> {
        let mut notes: Vec<String> = Vec::new();
        let mut cited: Vec<String> = Vec::new();
        for raw in line.split(|c: char| c.is_whitespace() || "–—,;()[]".contains(c)) {
            let tok = raw.trim_matches(|c: char| ".:!?\"'*`".contains(c));
            let mut chars = tok.chars();
            if let Some('M' | 'T' | 'L') = chars.next()
                && tok.len() > 1
                && chars.all(|c| c.is_ascii_digit())
            {
                if self.known.contains(tok) {
                    cited.push(tok.to_string());
                } else {
                    notes.push(format!("{tok} is not a record"));
                }
            }
        }
        for span in line.split('`').skip(1).step_by(2) {
            let span = span.trim();
            if span.len() >= 2 && span.len() <= 200 && !self.corpus.contains(span) {
                notes.push(format!("`{}` not in records", cut(span, 60)));
            }
        }
        for raw in line.split_whitespace() {
            if raw.contains('`') {
                continue;
            }
            let tok = raw.trim_matches(|c: char| "()[]{}<>,;:!?\"'*".contains(c));
            let tok = tok.trim_end_matches('.');
            if tok.len() < 4 || tok.starts_with("http") {
                continue;
            }
            let pathish = tok.contains('/') && tok.chars().any(char::is_alphanumeric);
            let fileish = tok.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty()
                    && (1..=5).contains(&ext.len())
                    && ext.chars().all(|c| c.is_ascii_lowercase())
                    && stem
                        .chars()
                        .all(|c| c.is_alphanumeric() || "_-./".contains(c))
            });
            let numberish = tok.chars().filter(char::is_ascii_digit).count() >= 5
                && tok
                    .chars()
                    .all(|c| c.is_ascii_digit() || ".,-:".contains(c));
            if (pathish || fileish || numberish) && !self.corpus.contains(tok) {
                notes.push(format!("{} not in records", cut(tok, 60)));
            }
        }
        let lower = line.to_lowercase();
        if [
            "succeed", "success", "passed", "passes", "works", "fixed", "green",
        ]
        .iter()
        .any(|w| lower.contains(w))
        {
            for id in cited.iter().filter(|id| failed.contains(*id)) {
                notes.push(format!("{id} failed"));
            }
        }
        notes.dedup();
        notes
    }
}

pub fn keep_start(messages: &[Message]) -> Option<usize> {
    messages
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, m)| is_real_user(m))
        .nth(KEEP_TURNS - 1)
        .map(|(i, _)| i)
}

pub fn cut_for_tail(messages: &[Message], tail: &mut Vec<Message>) -> usize {
    let mut cut = tail
        .iter()
        .position(is_real_user)
        .and_then(|p| messages.iter().rposition(|m| m == &tail[p]))
        .unwrap_or_else(|| messages.len().saturating_sub(tail.len()));
    if let Some(k) = keep_start(messages)
        && k > cut
        && let Some(p) = tail.iter().position(|m| m == &messages[k])
    {
        tail.drain(..p);
        cut = k;
    }
    cut
}
