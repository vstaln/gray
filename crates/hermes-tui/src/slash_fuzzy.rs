//! Description-aware fuzzy scoring for slash-menu completions.
//!
//! 1:1 port of `tui_gateway/slash_fuzzy.py` (91 lines).
//!
//! Candidates are scored in tiers — exact match on the command token (0),
//! prefix (1), substring (2) — and the DESCRIPTION text is tokenized and
//! matched at a +3 offset (exact word 3, word prefix 4, word substring 5).
//! Typing `/summary` thus surfaces a command whose description mentions
//! summaries even though no command name starts with it. Lower score wins;
//! `f64::INFINITY` means no match.
//!
//! Ported from `superagent-ai/grok-cli` `src/ui/slash-menu.ts` (mirrored on
//! the TUI client in `ui-tui/src/app/slash/fuzzyScore.ts`).
//!
//! ```python
//! # Python — tui_gateway/slash_fuzzy.py
//! _TOKEN_SPLIT = re.compile(r"[^a-z0-9]+")
//! def tokenize_search_text(value: str) -> list[str]: ...
//! def normalize_slash_search_query(query: str) -> str: ...
//! def _score_fields(fields: list[str], query: str, offset: int) -> float: ...
//! def score_slash_completion_item(item: dict, query: str) -> float: ...
//! def fuzzy_rank_slash_items(items, catalog, query): ...
//! ```

use std::collections::{HashMap, HashSet};

/// Completion item shape mirrored from Python's `{"text": ..., "meta": ...}` dict.
///
/// Python uses ad-hoc dicts with keys `text` (replacement token, may carry a
/// leading slash or trailing space), `display`, `meta` (human description),
/// and `kind`. Scoring only reads `text` + `meta`; `display`/`kind` are
/// preserved by the caller but are not scored here. This struct keeps the two
/// scored fields; extra display data can be carried alongside by the caller
/// when needed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SlashCompletionItem {
    /// Replacement token, e.g. `"/recaps"` or `"/recaps "` (may include slash).
    pub text: String,
    /// Human description, e.g. `"Turn session recaps on/off"`.
    pub meta: String,
}

impl SlashCompletionItem {
    /// Create an item from `text` + `meta`.
    pub fn new(text: impl Into<String>, meta: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            meta: meta.into(),
        }
    }
}

/// Lowercase `value` and return it alongside its alphanumeric words.
///
/// Mirrors `tokenize_search_text`:
///
/// ```python
/// _TOKEN_SPLIT = re.compile(r"[^a-z0-9]+")
/// def tokenize_search_text(value: str) -> list[str]:
///     normalized = value.lower()
///     return [normalized, *[t for t in _TOKEN_SPLIT.split(normalized) if t]]
/// ```
pub fn tokenize_search_text(value: &str) -> Vec<String> {
    let normalized = value.to_lowercase();
    let mut words = Vec::new();
    let mut cur = String::new();
    for ch in normalized.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            cur.push(ch);
        } else if !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let mut out = Vec::with_capacity(1 + words.len());
    out.push(normalized);
    out.extend(words);
    out
}

/// Trim, drop leading slashes, lowercase — `/Model` and `model` alike.
///
/// Mirrors `normalize_slash_search_query`:
///
/// ```python
/// def normalize_slash_search_query(query: str) -> str:
///     return query.strip().lstrip("/").lower()
/// ```
pub fn normalize_slash_search_query(query: &str) -> String {
    query.trim().trim_start_matches('/').to_lowercase()
}

fn score_fields(fields: &[String], query: &str, offset: f64) -> f64 {
    for field in fields {
        if field == query || format!("/{}", field) == query {
            return offset;
        }
    }
    for field in fields {
        if field.starts_with(query) || format!("/{}", field).starts_with(query) {
            return offset + 1.0;
        }
    }
    for field in fields {
        if field.contains(query) {
            return offset + 2.0;
        }
    }
    f64::INFINITY
}

/// Score one completion item against `query`.
///
/// `query` should already be normalized via [`normalize_slash_search_query`] —
/// the scorer is case-sensitive and expects lowercased input, matching the
/// Python contract where callers normalize once before looping.
///
/// ```python
/// def score_slash_completion_item(item: dict, query: str) -> float:
///     name = str(item.get("text", "")).strip().lstrip("/")
///     command_fields = tokenize_search_text(name)
///     description_fields = tokenize_search_text(str(item.get("meta", "")))
///     return min(
///         _score_fields(command_fields, query, 0),
///         _score_fields(description_fields, query, 3),
///     )
/// ```
///
/// Lower is better; `f64::INFINITY` means no match.
pub fn score_slash_completion_item(item: &SlashCompletionItem, query: &str) -> f64 {
    let name = item.text.trim().trim_start_matches('/').to_string();
    let command_fields = tokenize_search_text(&name);
    let description_fields = tokenize_search_text(&item.meta);
    let a = score_fields(&command_fields, query, 0.0);
    let b = score_fields(&description_fields, query, 3.0);
    a.min(b)
}

/// Merge description/substring matches into `items` and sort by score.
///
/// `items` are the completer's own (prefix-filtered) rows and keep their
/// identity; `catalog` is the full command/skill universe, from which any
/// entry the prefix filter missed but the fuzzy scorer matches is appended.
/// Returns the score-sorted rows (stable within a tier) plus a `score_of`
/// lookup for downstream rankers to use as a leading sort key.
///
/// ```python
/// def fuzzy_rank_slash_items(items, catalog, query):
///     seen = {str(item.get("text", "")).strip() for item in items}
///     merged = list(items)
///     for item in catalog:
///         if str(item.get("text", "")).strip() in seen:
///             continue
///         if not math.isinf(score_slash_completion_item(item, query)):
///             merged.append(item)
///     scores: dict[int, float] = {}
///     scored: list[tuple[float, int, dict]] = []
///     for index, item in enumerate(merged):
///         score = score_slash_completion_item(item, query)
///         if math.isinf(score):
///             continue
///         scores[id(item)] = score
///         scored.append((score, index, item))
///     scored.sort(key=lambda entry: (entry[0], entry[1]))
///     ranked = [item for _, _, item in scored]
///     return ranked, lambda item: scores.get(id(item), math.inf)
/// ```
///
/// # Rust notes
/// * Python keys `scores` by `id(item)` (object identity). Rust has no stable
///   dict identity after clone, so `score_of` is keyed by **value equality**
///   (`text` + `meta`). Passing a freshly constructed item with the same
///   content as a ranked row therefore returns its score (Python would return
///   `INF` for a distinct object). In practice callers always query with rows
///   from `ranked` or with new items that should be `INF` anyway, so the
///   observable behaviour matches.
/// * `items`/`catalog` deduplication uses `text.trim()` (whitespace only),
///   mirroring `str(item.get("text","")).strip()` — no slash stripping.
pub fn fuzzy_rank_slash_items(
    items: &[SlashCompletionItem],
    catalog: &[SlashCompletionItem],
    query: &str,
) -> (Vec<SlashCompletionItem>, Box<dyn Fn(&SlashCompletionItem) -> f64>) {
    let mut seen: HashSet<String> = HashSet::new();
    for item in items {
        seen.insert(item.text.trim().to_string());
    }

    let mut merged: Vec<SlashCompletionItem> = items.to_vec();
    for item in catalog {
        let key = item.text.trim().to_string();
        if seen.contains(&key) {
            continue;
        }
        if !score_slash_completion_item(item, query).is_infinite() {
            merged.push(item.clone());
        }
    }

    let mut scores: HashMap<SlashCompletionItem, f64> = HashMap::new();
    let mut scored: Vec<(f64, usize, SlashCompletionItem)> = Vec::new();

    for (index, item) in merged.iter().enumerate() {
        let score = score_slash_completion_item(item, query);
        if score.is_infinite() {
            continue;
        }
        scores.insert(item.clone(), score);
        scored.push((score, index, item.clone()));
    }

    scored.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });

    let ranked: Vec<SlashCompletionItem> = scored.into_iter().map(|(_, _, item)| item).collect();

    // Clone map into closure — lookup by value equality (see Rust notes).
    let score_of = Box::new(move |item: &SlashCompletionItem| {
        scores.get(item).copied().unwrap_or(f64::INFINITY)
    });

    (ranked, score_of)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, meta: &str) -> SlashCompletionItem {
        SlashCompletionItem::new(text, meta)
    }

    #[test]
    fn normalize_query() {
        assert_eq!(normalize_slash_search_query(" /Model "), "model");
        assert_eq!(normalize_slash_search_query("//help"), "help");
        assert_eq!(normalize_slash_search_query("plain"), "plain");
        assert_eq!(normalize_slash_search_query("  ///Foo  "), "foo");
    }

    #[test]
    fn tokenize_includes_full_and_words() {
        assert_eq!(
            tokenize_search_text("Commit & Push"),
            vec!["commit & push", "commit", "push"]
        );
        assert_eq!(tokenize_search_text(""), vec![""]);
        assert_eq!(tokenize_search_text("///"), vec!["///"]);
        assert_eq!(
            tokenize_search_text("a_b-c.d"),
            vec!["a_b-c.d", "a", "b", "c", "d"]
        );
    }

    #[test]
    fn score_tiers_name_before_description() {
        let it = item("/recaps ", "Turn session recaps on/off");
        assert_eq!(score_slash_completion_item(&it, "recaps"), 0.0);
        assert_eq!(score_slash_completion_item(&it, "rec"), 1.0);
        assert_eq!(score_slash_completion_item(&it, "caps"), 2.0);
        assert_eq!(score_slash_completion_item(&it, "session"), 3.0);
        assert_eq!(score_slash_completion_item(&it, "sess"), 4.0);
        assert_eq!(score_slash_completion_item(&it, "essio"), 5.0);
        assert!(score_slash_completion_item(&it, "zzz").is_infinite());
    }

    #[test]
    fn name_match_beats_description_match() {
        let it = item("/recap", "Turn session recaps on/off");
        assert_eq!(score_slash_completion_item(&it, "recap"), 0.0);
    }

    #[test]
    fn fuzzy_rank_merges_description_matches() {
        let prefix_hits = vec![item("/summon", "")];
        let catalog = vec![
            item("/summon", ""),
            item("/recaps", "Show a summary of the session"),
            item("/help", "Show available commands"),
        ];
        let (ranked, score_of) = fuzzy_rank_slash_items(&prefix_hits, &catalog, "summ");
        let texts: Vec<&str> = ranked.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, vec!["/summon", "/recaps"]);
        assert_eq!(score_of(&ranked[0]), 1.0);
        assert_eq!(score_of(&ranked[1]), 4.0);
        assert!(score_of(&item("/help", "Show available commands")).is_infinite());
    }

    #[test]
    fn fuzzy_rank_is_stable_within_tier() {
        let items = vec![item("/mod-b", ""), item("/mod-a", "")];
        let (ranked, _) = fuzzy_rank_slash_items(&items, &[], "mod");
        let texts: Vec<&str> = ranked.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, vec!["/mod-b", "/mod-a"]);
    }

    #[test]
    fn fuzzy_rank_drops_non_matching() {
        let (ranked, _) = fuzzy_rank_slash_items(&[item("/other", "")], &[], "model");
        assert!(ranked.is_empty());
    }

    #[test]
    fn slash_prefix_in_query_handled() {
        // score_fields handles leading slash in query via "/field" check.
        // With offset 0, exact "/recaps" should score 0 even though field is "recaps".
        let fields = tokenize_search_text("recaps");
        // field "recaps", query "/recaps" => "/recaps" == "/recaps"
        assert_eq!(score_fields(&fields, "/recaps", 0.0), 0.0);
        assert_eq!(score_fields(&fields, "/rec", 0.0), 1.0);
    }
}
