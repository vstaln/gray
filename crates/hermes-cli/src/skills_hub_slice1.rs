//! hermes-cli skills_hub — slice 1/3
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/skills_hub.py`
//! slice 1/3 — lines 1–900 of 2 127 (first 900 LOC).
//! Covers: module docstring + imports (`json`, `logging`, `re`, `shutil`,
//! `pathlib.Path`, `rich.console`/`panel`/`table`, `hermes_constants`),
//! `_display_source`, `_resolve_short_name`, `_print_tier1_advisory`,
//! `_format_extra_metadata_lines`, `_resolve_source_meta_and_bundle`,
//! `_derive_category_from_install_path`, interactive URL-install helpers
//! (`_VALID_NAME_RE`/`_VALID_CATEGORY_RE`, `_is_valid_installed_skill_name`,
//! `_existing_categories`, `_prompt_for_skill_name`, `_prompt_for_category`),
//! `do_search` (JSON + table branches), `do_browse` (trust-rank, per-source
//! limits, `parallel_search_sources` live-progress, provider filter,
//! dedup, sort, pagination, table + nav + source summary), `do_install`
//! (source pin, short-name resolve, `quarantine_bundle` → `scan_skill_cached`
//! → `should_allow_install` → `Tier1` advisory → `install_from_quarantine` →
//! blueprint suggestion → cache invalidation, including URL-name / category
//! prompts, official auto-category, rate-limit hint, and consent panels),
//! and `do_inspect` (metadata panel + `SKILL.md` 50-line preview, line 848–895).
//! Continued in `skills_hub_slice2.rs` (from `browse_skills`, line 898).
//!
//! T0701 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-11
// ---------------------------------------------------------------------------

/// Skills Hub CLI — Unified interface for the Hermes Skills Hub.
///
/// Powers both:
///   - `hermes skills <subcommand>` (CLI argparse entry point)
///   - `/skills <subcommand>` (slash command in the interactive chat)
///
/// All logic lives in shared `do_*` functions. The CLI entry point and slash
/// command handler are thin wrappers that parse args and delegate.
///
/// Mirrors `hermes_cli/skills_hub.py` lines 1-11.
pub const MODULE_DOC: &str = "skills_hub: unified Skills Hub — see skills_hub.py lines 1-11";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 13-28
// ---------------------------------------------------------------------------
// Python: json, logging, re, shutil, pathlib.Path, typing.Any/Dict/List/Optional,
//         rich.console.Console, rich.panel.Panel, rich.table.Table,
//         hermes_constants.display_hermes_home
//         Lazy: tools.skills_hub (unified_search, GitHubAuth, create_source_router,
//               parallel_search_sources, SKILLS_DIR, _category_skill_dirs,
//               quarantine_bundle, install_from_quarantine, HubLockFile,
//               ensure_hub_dirs, source_url_for_bundle, HUB_DIR, SKILLS_DIR, ...),
//               tools.skills_guard, tools.skillevaluator_scan,
//               hermes_cli.cli_output.line_input, agent.prompt_builder,
//               tools.blueprints, tools.skills_hub.TapsManager etc. (imported inside fns)
//
// Rust: std only (NEVER cargo). Rich console/panel/table, hermes_constants,
//       and all tools.* subsystems are stubbed for 1:1 traceability; real
//       wiring in later slices or via injected trait objects.

// ---------------------------------------------------------------------------
// Console / Rich stubs — mirrors `rich.console.Console`, `rich.panel.Panel`,
// `rich.table.Table`, and module-level `_console = Console()` (lines 21-28)
// ---------------------------------------------------------------------------

/// Minimal stub for `rich.console.Console`.
#[derive(Debug, Clone, Default)]
pub struct Console;

impl Console {
    pub fn new() -> Self {
        Self
    }
    /// Mirrors `console.print(...)`.
    pub fn print(&self, msg: &str) {
        println!("{msg}");
    }
    /// Mirrors `console.print(Panel(...))`.
    pub fn print_panel(&self, text: &str, title: &str, border_style: &str) {
        // Mirrors `Panel(text, title=..., border_style=...)`
        println!("[{border_style}][{title}] {text}");
    }
    /// Mirrors `with console.status("...") as status:`
    pub fn status(&self, msg: &str) -> StatusGuard {
        println!("{msg}");
        StatusGuard {
            msg: msg.to_string(),
        }
    }
}

/// Mirrors the `with c.status(...) as status:` context manager.
pub struct StatusGuard {
    msg: String,
}
impl StatusGuard {
    pub fn update(&self, msg: &str) {
        // Mirrors `status.update(...)` live progress tick
        println!("{msg}");
    }
}
impl Drop for StatusGuard {
    fn drop(&mut self) {
        let _ = &self.msg;
    }
}

/// Module-level `_console` singleton — mirrors ` _console = Console()` (line 28).
pub fn global_console() -> Console {
    Console::new()
}

// hermes_constants stub — mirrors `from hermes_constants import display_hermes_home` (26)
pub fn display_hermes_home() -> String {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            // Mirrors display_hermes_home(): "~/.hermes" or "~/.hermes/profiles/<name>"
            if let Ok(home) = std::env::var("HOME") {
                let h = PathBuf::from(home);
                let p = PathBuf::from(v.trim());
                if let Ok(rel) = p.strip_prefix(&h) {
                    return format!("~/{}", rel.display());
                }
                return p.display().to_string();
            }
            return v.trim().to_string();
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return format!("{}/.hermes", home.trim_end_matches('/'));
    }
    "~/.hermes".to_string()
}

pub fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".hermes"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/.hermes"))
}

fn log_debug(msg: &str) {
    if std::env::var("HERMES_DEBUG").is_ok() {
        eprintln!("[skills_hub] DEBUG: {msg}");
    }
}

// ---------------------------------------------------------------------------
// Domain types — mirrors `tools.skills_hub` result / meta / bundle shapes
// ---------------------------------------------------------------------------

/// Mirrors a single search/inspect result row (has `name`, `identifier`,
/// `source`, `trust_level`, `description`, `extra: Dict[str,Any]`, `tags`).
#[derive(Debug, Clone)]
pub struct SkillRow {
    pub name: String,
    pub identifier: String,
    pub source: String,
    pub trust_level: String,
    pub description: String,
    /// Mirrors `r.extra` (may contain `provider`, `repo_url`, `detail_url`, etc.)
    pub extra: HashMap<String, String>,
    pub tags: Vec<String>,
}

impl SkillRow {
    pub fn new(
        name: &str,
        identifier: &str,
        source: &str,
        trust_level: &str,
        description: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            identifier: identifier.to_string(),
            source: source.to_string(),
            trust_level: trust_level.to_string(),
            description: description.to_string(),
            extra: HashMap::new(),
            tags: Vec::new(),
        }
    }
}

/// Mirrors `SkillMeta` returned by `src.inspect(identifier)` (lines 168-194).
#[derive(Debug, Clone)]
pub struct SkillMeta {
    pub name: String,
    pub identifier: String,
    pub source: String,
    pub trust_level: String,
    pub description: String,
    pub tags: Vec<String>,
    pub extra: HashMap<String, String>,
    /// Mirrors `meta.path` (install-relative path)
    pub path: String,
}

/// Mirrors `SkillBundle` returned by `src.fetch(identifier)` (lines 168-194).
#[derive(Debug, Clone)]
pub struct SkillBundle {
    pub name: String,
    pub identifier: String,
    pub source: String,
    pub trust_level: String,
    pub metadata: HashMap<String, String>,
    /// Mirrors `bundle.files: Dict[str, bytes|str]` (key = filename)
    pub files: HashMap<String, Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Source router traits — mirrors `tools.skills_hub` source adapters
// ---------------------------------------------------------------------------

/// Minimal trait for a skills source adapter (official, skills-sh, github, etc.).
/// Mirrors the `src.inspect()` / `src.fetch()` duck interface used in
/// `_resolve_source_meta_and_bundle` (lines 161-194).
pub trait SkillSource {
    fn id(&self) -> &str;
    fn inspect(&self, identifier: &str) -> Option<SkillMeta>;
    fn fetch(&self, identifier: &str) -> Option<SkillBundle>;
    /// Mirrors `getattr(src, "is_rate_limited", False)` check (lines 595-596)
    fn is_rate_limited(&self) -> bool {
        false
    }
    /// Mirrors `getattr(src, "github", None).is_rate_limited` check
    fn github_rate_limited(&self) -> bool {
        false
    }
}

/// Mirrors `GitHubAuth` (lines 310, 380, 382, 569, 853, 916).
#[derive(Debug, Clone, Default)]
pub struct GitHubAuth {
    pub authenticated: bool,
}
impl GitHubAuth {
    pub fn new() -> Self {
        Self { authenticated: false }
    }
    pub fn is_authenticated(&self) -> bool {
        if std::env::var("GITHUB_TOKEN").ok().map(|v| !v.trim().is_empty()).unwrap_or(false) {
            return true;
        }
        self.authenticated
    }
}

// ---------------------------------------------------------------------------
// Helpers: trust rank + per-source limits — mirrors do_browse / browse_skills
// ---------------------------------------------------------------------------

fn trust_rank(trust: &str) -> i32 {
    // Mirrors `_TRUST_RANK = {"builtin": 3, "trusted": 2, "community": 1}` (385, 908)
    match trust {
        "builtin" => 3,
        "trusted" => 2,
        "community" => 1,
        _ => 0,
    }
}

fn per_source_limit_do_browse(source: &str) -> usize {
    // Mirrors `_PER_SOURCE_LIMIT` in do_browse (395-400)
    match source {
        "hermes-index" => 1_000_000,
        "official" => 200,
        "skills-sh" => 200,
        "well-known" => 50,
        "github" => 200,
        "clawhub" => 500,
        "lobehub" => 500,
        "browse-sh" => 500,
        _ => 200,
    }
}

// ---------------------------------------------------------------------------
// _display_source — mirrors lines 31-42
// ---------------------------------------------------------------------------

/// Human-facing source label for a result row.
///
/// GitHub-tap skills are stored under `source="github"`; surface their
/// per-tap provider label (NVIDIA / OpenAI / ...) when present so the table
/// reflects the real origin instead of the generic "github".
///
/// Mirrors `def _display_source(r) -> str:` (31-42).
pub fn display_source(r: &SkillRow) -> String {
    if r.source == "github" {
        if let Some(provider) = r.extra.get("provider") {
            if !provider.trim().is_empty() {
                return provider.clone();
            }
        }
    }
    r.source.clone()
}

// ---------------------------------------------------------------------------
// _resolve_short_name — mirrors lines 49-95
// ---------------------------------------------------------------------------

/// Resolve a short skill name (e.g. `"pptx"`) to a full identifier by
/// searching all sources. If exactly one match is found, returns its
/// identifier. If multiple matches exist, shows them and asks the user to
/// use the full identifier. Returns empty string if nothing found or
/// ambiguous.
///
/// Mirrors `def _resolve_short_name(name, sources, console)` (49-95).
pub fn resolve_short_name(
    name: &str,
    sources: &[Box<dyn SkillSource>],
    console: &Console,
) -> String {
    // Mirrors `from tools.skills_hub import unified_search` (56) — lazy import
    // In Rust the search is delegated to `unified_search_stub` (later slice).
    console.print(&format!("[dim]Resolving '{name}'...[/]"));

    let results = unified_search_stub(name, sources, "all", 20);

    // Filter to exact name matches (case-insensitive) — mirrors line 64
    let exact: Vec<&SkillRow> = results
        .iter()
        .filter(|r| r.name.to_lowercase() == name.to_lowercase())
        .collect();

    if exact.len() == 1 {
        console.print(&format!("[dim]Resolved to: {}[/]", exact[0].identifier));
        return exact[0].identifier.clone();
    }

    if exact.len() > 1 {
        // Mirrors table with Source | Trust | Identifier (lines 72-83)
        console.print(&format!("\n[yellow]Multiple skills named '{name}' found:[/]"));
        // Render as simple list (rich.table.Table stub — std only)
        for r in &exact {
            let trust_style = match r.trust_level.as_str() {
                "builtin" => "bright_cyan",
                "trusted" => "green",
                "community" => "yellow",
                _ => "dim",
            };
            let trust_label = if r.source == "official" {
                "official".to_string()
            } else {
                r.trust_level.clone()
            };
            console.print(&format!(
                "  [dim]{}[/] [{trust_style}]{trust_label}[/] [bold cyan]{}[/]",
                r.source, r.identifier
            ));
        }
        console.print("[bold]Use the full identifier to install a specific one.[/]\n");
        return String::new();
    }

    // No exact match — check if there are partial matches to suggest (lines 87-92)
    if !results.is_empty() {
        console.print(&format!("[yellow]No exact match for '{name}'. Did you mean one of these?[/]"));
        for r in results.iter().take(5) {
            console.print(&format!("  [cyan]{}[/] — {}", r.name, r.identifier));
        }
        console.print("");
        return String::new();
    }

    console.print(&format!("[bold red]Error:[/] No skill named '{name}' found in any source.\n"));
    String::new()
}

// unified_search stub — mirrors `tools.skills_hub.unified_search` (61, 315, 331)
// Real parallel search wired in do_browse / tools hub later slices.
fn unified_search_stub(
    _query: &str,
    _sources: &[Box<dyn SkillSource>],
    _source_filter: &str,
    _limit: usize,
) -> Vec<SkillRow> {
    // Without `tools.skills_hub` index / network, return empty in slice 1.
    // Preserves the "no results" branch for 1:1 audit; real search in later slice.
    Vec::new()
}

// ---------------------------------------------------------------------------
// _print_tier1_advisory — mirrors lines 98-130
// ---------------------------------------------------------------------------

/// Print the advisory SkillEvaluator Tier 1 report, if available.
///
/// Never raises and never blocks the install: scanner missing, disabled
/// via `skills.tier1_advisory: false`, or erroring all degrade to silence.
/// Secrets-class findings render red, the rest yellow.
///
/// Mirrors `def _print_tier1_advisory(skill_dir, console)` (98-130).
pub fn print_tier1_advisory(skill_dir: &Path, console: &Console) {
    // Mirrors `try: from tools.skillevaluator_scan import ... except Exception: debug` (129-130)
    // Slice 1: without `tools.skillevaluator_scan` (no cargo / no Python), degrade to no-op
    // but preserve the enabled-check shape for 1:1 traceability.
    let enabled = tier1_advisory_enabled_stub();
    if !enabled {
        return;
    }
    match run_tier1_scan_stub(skill_dir) {
        None => return, // mirrors `if not report.available: return` (112-113)
        Some(report) => {
            if !report.available {
                return;
            }
            let text = format_tier1_report_stub(&report);
            if report.findings.is_empty() {
                // Mirrors `console.print(f"[dim]{text}[/]")` (116)
                console.print(&format!("[dim]{text}[/]"));
                return;
            }
            let style = if report.secrets_findings { "red" } else { "yellow" };
            // Mirrors `console.print(Panel(text, title="SkillEvaluator Tier 1 (advisory)", border_style=style))` (119-123)
            console.print_panel(&text, "SkillEvaluator Tier 1 (advisory)", style);
            if report.secrets_findings {
                console.print(
                    "[bold red]Possible credentials detected above.[/] Review the flagged lines before using this skill.\n",
                );
            }
        }
    }
    // Any exception is swallowed — mirrors `except Exception as exc: logging.debug(...)` (129-130)
}

#[derive(Debug, Clone)]
struct Tier1ReportStub {
    available: bool,
    findings: Vec<String>,
    secrets_findings: bool,
}
fn tier1_advisory_enabled_stub() -> bool {
    // Mirrors `tier1_advisory_enabled()` — checks `skills.tier1_advisory` config.
    // In slice 1 without config, default to false (advisory off) to match "degrade to silence".
    // Real config read in later slice.
    // We check env var for test override; otherwise false.
    std::env::var("HERMES_TIER1_ADVISORY").map(|v| v == "1" || v.to_lowercase() == "true").unwrap_or(false)
}
fn run_tier1_scan_stub(_skill_dir: &Path) -> Option<Tier1ReportStub> {
    None
}
fn format_tier1_report_stub(_report: &Tier1ReportStub) -> String {
    String::new()
}

// ---------------------------------------------------------------------------
// _format_extra_metadata_lines — mirrors lines 133-158
// ---------------------------------------------------------------------------

/// Build extra metadata display lines from an `extra` dict.
///
/// Mirrors `def _format_extra_metadata_lines(extra: Dict[str, Any]) -> list[str]:` (133-158).
pub fn format_extra_metadata_lines(extra: &HashMap<String, String>) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if extra.is_empty() {
        return lines;
    }
    // Mirrors lines 138-151 — each key maps to a bold label line
    if let Some(v) = extra.get("repo_url") {
        if !v.is_empty() {
            lines.push(format!("[bold]Repo:[/] {v}"));
        }
    }
    if let Some(v) = extra.get("detail_url") {
        if !v.is_empty() {
            lines.push(format!("[bold]Detail Page:[/] {v}"));
        }
    }
    if let Some(v) = extra.get("index_url") {
        if !v.is_empty() {
            lines.push(format!("[bold]Index:[/] {v}"));
        }
    }
    if let Some(v) = extra.get("endpoint") {
        if !v.is_empty() {
            lines.push(format!("[bold]Endpoint:[/] {v}"));
        }
    }
    if let Some(v) = extra.get("install_command") {
        if !v.is_empty() {
            lines.push(format!("[bold]Install Command:[/] {v}"));
        }
    }
    if let Some(v) = extra.get("installs") {
        lines.push(format!("[bold]Installs:[/] {v}"));
    }
    if let Some(v) = extra.get("weekly_installs") {
        if !v.is_empty() {
            lines.push(format!("[bold]Weekly Installs:[/] {v}"));
        }
    }
    // Mirrors `security_audits` dict formatting (153-156)
    if let Some(v) = extra.get("security_audits") {
        // In Python `security_audits` is a dict; here it's serialized as "k=v, ..." string
        // We treat the stored string as already formatted if it contains '=', else build from prefix.
        // Preserve the `sorted(security.items())` ordering contract by sorting keys if we have a
        // serialized dict representation. For stub, just emit as-is if non-empty.
        if !v.is_empty() && v != "{}" {
            lines.push(format!("[bold]Security:[/] {v}"));
        }
    }
    lines
}

/// Typed variant that accepts a `security_audits` map separately (closer to Python's dict-of-dicts).
pub fn format_extra_metadata_lines_with_security(
    extra: &HashMap<String, String>,
    security_audits: Option<&HashMap<String, String>>,
) -> Vec<String> {
    let mut lines = format_extra_metadata_lines(extra);
    if let Some(sec) = security_audits {
        if !sec.is_empty() {
            let mut pairs: Vec<String> = sec.iter().map(|(k, v)| format!("{k}={v}")).collect();
            pairs.sort();
            lines.push(format!("[bold]Security:[/] {}", pairs.join(", ")));
        }
    }
    lines
}

// ---------------------------------------------------------------------------
// _resolve_source_meta_and_bundle — mirrors lines 161-194
// ---------------------------------------------------------------------------

/// Resolve metadata and bundle from a single source adapter.
///
/// Meta and bundle must come from the same adapter. Keeping catalog
/// metadata from `skills.sh` while taking a ClawHub zip of a same-named
/// skill is how `hermes skills inspect owner/repo/skills/foo` showed
/// the requested identifier and the wrong `SKILL.md`.
///
/// Mirrors `def _resolve_source_meta_and_bundle(identifier, sources):` (161-194).
pub fn resolve_source_meta_and_bundle(
    identifier: &str,
    sources: &[Box<dyn SkillSource>],
) -> (Option<SkillMeta>, Option<SkillBundle>, Option<String>) {
    let mut first_meta: Option<SkillMeta> = None;
    let mut first_meta_source: Option<String> = None;

    for src in sources {
        let mut meta: Option<SkillMeta> = None;
        let mut bundle: Option<SkillBundle> = None;

        // Mirrors `try: meta = src.inspect(identifier) except Exception: meta = None` (175-178)
        meta = src.inspect(identifier).or(meta);
        // Mirrors `try: bundle = src.fetch(identifier) except Exception: bundle = None` (179-182)
        bundle = src.fetch(identifier).or(bundle);

        if bundle.is_some() {
            // Mirrors `if meta is None: try: meta = src.inspect(identifier) except: meta=None` (184-189)
            if meta.is_none() {
                meta = src.inspect(identifier);
            }
            return (meta, bundle, Some(src.id().to_string()));
        }
        if first_meta.is_none() {
            if let Some(m) = meta {
                first_meta = Some(m);
                first_meta_source = Some(src.id().to_string());
            }
        }
    }

    (first_meta, None, first_meta_source)
}

// ---------------------------------------------------------------------------
// _derive_category_from_install_path — mirrors lines 197-200
// ---------------------------------------------------------------------------

/// Mirrors `def _derive_category_from_install_path(install_path: str) -> str:` (197-200).
pub fn derive_category_from_install_path(install_path: &str) -> String {
    let path = Path::new(install_path);
    let parent = path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    if parent == "." || parent.is_empty() {
        String::new()
    } else {
        parent
    }
}

// ---------------------------------------------------------------------------
// Interactive name/category resolution for URL-installed skills — lines 206-293
// ---------------------------------------------------------------------------

// Mirrors `_VALID_NAME_RE = re.compile(r"^[a-z][a-z0-9_-]*$")` (207)
/// Returns true if `name` matches `^[a-z][a-z0-9_-]*$` (mirrors `_VALID_NAME_RE`).
pub fn is_valid_name_re(name: &str) -> bool {
    // Manual without `regex` crate (NEVER cargo) — same as auth_slice1 profile check.
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {},
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return false;
        }
    }
    true
}

// Mirrors `_VALID_CATEGORY_RE = re.compile(r"^[a-z][a-z0-9_/-]*$")` (208)
pub fn is_valid_category_re(cat: &str) -> bool {
    if cat.is_empty() {
        return false;
    }
    let mut chars = cat.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {},
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '/' || c == '-') {
            return false;
        }
    }
    true
}

/// Accept identifier-shaped names, reject empty / sentinel-y values.
///
/// Mirrors `def _is_valid_installed_skill_name(name: str) -> bool:` (211-218).
pub fn is_valid_installed_skill_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let candidate = name.trim().to_lowercase();
    if candidate.is_empty() {
        return false;
    }
    if matches!(candidate.as_str(), "skill" | "readme" | "index" | "unnamed-skill") {
        return false;
    }
    is_valid_name_re(&candidate)
}

/// Return sorted subdirectory names under `~/.hermes/skills/` that look
/// like category buckets (contain at least one `SKILL.md` somewhere below).
///
/// Used to suggest reusable categories when interactively installing from a URL.
/// Hidden dirs (`.hub`, `.trash`) are skipped.
///
/// Mirrors `def _existing_categories() -> List[str]:` (221-239).
pub fn existing_categories() -> Vec<String> {
    // Mirrors `from tools.skills_hub import SKILLS_DIR, _category_skill_dirs` (228)
    let skills_dir = get_hermes_home().join("skills");
    if !skills_dir.is_dir() {
        return Vec::new();
    }
    // Try to enumerate category buckets — mirrors `_category_skill_dirs(SKILLS_DIR)` (233)
    // and `if not (SKILLS_DIR / name / "SKILL.md").exists()` (237).
    let entries = match std::fs::read_dir(&skills_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut cats: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // mirrors hidden dir skip
        }
        // Only children WITHOUT their own SKILL.md are category buckets (lines 233-236)
        if path.join("SKILL.md").exists() {
            continue;
        }
        // Check if any descendant contains SKILL.md — mirrors `_category_skill_dirs` semantics
        if has_skill_md_descendant(&path) {
            cats.push(name);
        } else {
            // Also include empty-ish category buckets for completeness (mirrors Python's
            // `_category_skill_dirs` which returns any child with active SKILL.md somewhere below)
            // We keep the strict check: only push if we found SKILL.md below.
        }
    }
    cats.sort();
    cats
}

fn has_skill_md_descendant(dir: &Path) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && p.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
            return true;
        }
        if p.is_dir() && !is_symlink(&p) {
            if has_skill_md_descendant(&p) {
                return true;
            }
        }
    }
    false
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Prompt interactively for a skill name. Returns None on cancel/EOF.
///
/// Mirrors `def _prompt_for_skill_name(c, url, default="") -> Optional[str]:` (242-266).
pub fn prompt_for_skill_name(c: &Console, url: &str, default: &str) -> Option<String> {
    c.print("");
    c.print(&format!(
        "[yellow]The SKILL.md at {url} doesn't declare a `name:` in its frontmatter,[/]\n[yellow]and the URL path doesn't produce a valid identifier either.[/]"
    ));
    let default_hint = if default.is_empty() { String::new() } else { format!(" [{default}]") };
    c.print(&format!(
        "[bold]Enter a skill name{default_hint}:[/] [dim](lowercase letters, digits, hyphens, underscores; starts with a letter)[/]"
    ));
    // Mirrors `from hermes_cli.cli_output import line_input` + `line_input("Name: ")` (255, 258)
    let answer = line_input_stub("Name: ")?;
    let mut answer = answer.trim().to_string();
    if answer.is_empty() && !default.is_empty() {
        answer = default.to_string();
    }
    if !is_valid_installed_skill_name(&answer) {
        c.print(&format!("[bold red]Invalid name:[/] {answer:?}. Aborting install.\n"));
        return None;
    }
    Some(answer)
}

/// Prompt interactively for a category. Empty input means flat install.
///
/// Mirrors `def _prompt_for_category(c, existing)` (269-293).
pub fn prompt_for_category(c: &Console, existing: &[String]) -> String {
    c.print("");
    if !existing.is_empty() {
        c.print(
            "[bold]Pick a category[/] [dim](reuse an existing bucket, type a new one, or press Enter to install flat)[/]",
        );
        c.print(&format!("[dim]Existing: {}[/]", existing.join(", ")));
    } else {
        c.print(
            "[bold]Category[/] [dim](optional — press Enter to install flat at ~/.hermes/skills/<name>/)[/]",
        );
    }
    // Mirrors `from hermes_cli.cli_output import line_input` + `line_input("Category: ")` (282, 285)
    let answer = match line_input_stub("Category: ") {
        Some(s) => s.trim().to_string(),
        None => return String::new(),
    };
    if answer.is_empty() {
        return String::new();
    }
    if !is_valid_category_re(&answer) {
        c.print(&format!("[dim]Invalid category {answer:?} — installing flat.[/]"));
        return String::new();
    }
    answer
}

// cli_output.line_input stub — mirrors `hermes_cli.cli_output.line_input` (255, 282)
fn line_input_stub(prompt: &str) -> Option<String> {
    use std::io::{self, Write};
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut buf = String::new();
    match io::stdin().read_line(&mut buf) {
        Ok(0) => None, // EOF
        Ok(_) => Some(buf),
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// do_search — mirrors lines 296-362
// ---------------------------------------------------------------------------

/// Search registries and display results as a Rich table.
///
/// When `as_json=True` writes a JSON array of result records to stdout
/// (one object per skill: `name`, `identifier`, `source`,
/// `trust_level`, `description`) and skips the table render. This is
/// the scripting / copy-paste handle: the full identifier is always
/// intact, even for browse-sh slugs that the table would otherwise wrap.
///
/// Mirrors `def do_search(query, source="all", limit=10, console=None, as_json=False)` (296-362).
pub fn do_search(
    query: &str,
    source: &str,
    limit: usize,
    console: Option<&Console>,
    as_json: bool,
) {
    // Mirrors `from tools.skills_hub import GitHubAuth, create_source_router, unified_search` (306)
    let c_owned;
    let c = match console {
        Some(v) => v,
        None => {
            c_owned = global_console();
            &c_owned
        }
    };

    let auth = GitHubAuth::new();
    let sources = create_source_router_stub(&auth);

    if as_json {
        // Avoid Rich status spinner contaminating stdout — JSON consumers expect clean parseable stream.
        // Mirrors lines 312-327
        let results = unified_search_stub(query, &sources, source, limit);
        let mut out = String::from("[\n");
        for (i, r) in results.iter().enumerate() {
            // Manual JSON (no serde — NEVER cargo). Mirrors `json.dumps(payload, indent=2)`.
            let item = format!(
                "  {{\"name\": {}, \"identifier\": {}, \"source\": {}, \"trust_level\": {}, \"description\": {}}}",
                json_escape(&r.name),
                json_escape(&r.identifier),
                json_escape(&r.source),
                json_escape(&r.trust_level),
                json_escape(&r.description),
            );
            out.push_str(&item);
            if i + 1 < results.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("]\n");
        print!("{out}");
        return;
    }

    c.print(&format!("\n[bold]Searching for:[/] {query}"));
    // Mirrors `with c.status("[bold]Searching registries..."):` (330)
    let _status = c.status("[bold]Searching registries...[/]");
    let results = unified_search_stub(query, &sources, source, limit);

    if results.is_empty() {
        c.print("[dim]No skills found matching your query.[/]\n");
        return;
    }

    // Build table — mirrors lines 337-357
    // Table(title=..., columns: Name, Description, Source, Trust, Identifier)
    c.print(&format!("Skills Hub — {} result(s)", results.len()));
    // Header
    c.print("Name | Description | Source | Trust | Identifier");
    c.print(&"-".repeat(80));
    for r in &results {
        let trust_style = match r.trust_level.as_str() {
            "builtin" => "bright_cyan",
            "trusted" => "green",
            "community" => "yellow",
            _ => "dim",
        };
        let trust_label = if r.source == "official" {
            "official".to_string()
        } else {
            r.trust_level.clone()
        };
        let desc = if r.description.len() > 60 {
            format!("{}...", &r.description[..60])
        } else {
            r.description.clone()
        };
        c.print(&format!(
            "{} | {} | {} | [{trust_style}]{trust_label}[/] | {}",
            r.name,
            desc,
            display_source(r),
            r.identifier
        ));
    }
    c.print(
        "[dim]Use: hermes skills inspect <identifier> to preview, hermes skills install <identifier> to install (--json for scripting)[/]\n",
    );
}

fn json_escape(s: &str) -> String {
    let mut out = String::from('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn create_source_router_stub(_auth: &GitHubAuth) -> Vec<Box<dyn SkillSource>> {
    // Mirrors `create_source_router(auth)` — builds adapters for official, skills-sh, github, etc.
    // In slice 1 without `tools.skills_hub` network/index, return empty vec.
    Vec::new()
}

// ---------------------------------------------------------------------------
// do_browse — mirrors lines 365-533
// ---------------------------------------------------------------------------

/// Browse all available skills across registries, paginated.
///
/// Official skills are always shown first, regardless of source filter.
///
/// Mirrors `def do_browse(page=1, page_size=20, source="all", console=None)` (365-533).
pub fn do_browse(page: usize, page_size: usize, source: &str, console: Option<&Console>) {
    // Mirrors `from tools.skills_hub import GitHubAuth, create_source_router, parallel_search_sources` (371-373)

    // Clamp page_size to safe range — mirrors `page_size = max(1, min(page_size, 100))` (376)
    let page_size = page_size.clamp(1, 100);

    let c_owned;
    let c = match console {
        Some(v) => v,
        None => {
            c_owned = global_console();
            &c_owned
        }
    };

    let auth = GitHubAuth::new();
    let sources = create_source_router_stub(&auth);

    // Per-source limits — mirrors `_PER_SOURCE_LIMIT` (395-400)
    // "hermes-index" must carry high limit to avoid truncating catalog (389-394 note).
    let per_source_limits: HashMap<&str, usize> = [
        ("hermes-index", 1_000_000usize),
        ("official", 200),
        ("skills-sh", 200),
        ("well-known", 50),
        ("github", 200),
        ("clawhub", 500),
        ("lobehub", 500),
        ("browse-sh", 500),
    ]
    .into_iter()
    .collect();
    let _ = per_source_limits;
    let _ = per_source_limit_do_browse; // retained for audit

    // Collect results with live progress — mirrors lines 402-425
    // `parallel_search_sources` is called with `on_source_done` callback that ticks
    // off each source as it resolves; page renders once after merged/sorted set.
    let mut done: Vec<String> = Vec::new();
    let status = c.status("[bold]Fetching skills from registries...[/]");

    // Callback shape: `def _on_source_done(sid, count): _done.append(...)` (411-416)
    let on_source_done = |sid: &str, count: usize| {
        done.push(format!("{sid} ({count})"));
        status.update(&format!(
            "[bold]Fetching skills from registries...[/]  [dim]done: {}[/]",
            done.join(", ")
        ));
    };
    let _ = on_source_done;

    let (mut all_results, source_counts, timed_out) = parallel_search_sources_stub(
        &sources,
        "",
        &per_source_limits,
        source,
        30,
    );

    if all_results.is_empty() {
        c.print("[dim]No skills found in the Skills Hub.[/]\n");
        return;
    }

    // Provider filter — mirrors lines 431-439
    // When source is a provider name (nvidia/openai/...), narrow github-tap skills by extra.provider
    if is_provider_filter_value(source) {
        all_results = filter_results_by_provider_stub(all_results, source);
        if all_results.is_empty() {
            c.print(&format!("[dim]No skills found for provider '{source}'.[/]\n"));
            return;
        }
    }

    // Deduplicate by identifier, preferring higher trust — mirrors lines 441-449
    let mut seen: HashMap<String, SkillRow> = HashMap::new();
    for r in all_results {
        let rank = trust_rank(&r.trust_level);
        let keep = match seen.get(&r.identifier) {
            None => true,
            Some(existing) => rank > trust_rank(&existing.trust_level),
        };
        if keep {
            seen.insert(r.identifier.clone(), r);
        }
    }
    let mut deduped: Vec<SkillRow> = seen.into_values().collect();

    // Sort: official first, then by trust desc, then alphabetically — mirrors lines 451-456
    deduped.sort_by(|a, b| {
        let ra = trust_rank(&a.trust_level);
        let rb = trust_rank(&b.trust_level);
        rb.cmp(&ra)
            .then_with(|| (a.source != "official").cmp(&(b.source != "official")))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    // Paginate — mirrors lines 458-464
    let total = deduped.len();
    let total_pages = std::cmp::max(1, (total + page_size - 1) / page_size);
    let page = page.clamp(1, total_pages);
    let start = (page - 1) * page_size;
    let end = std::cmp::min(start + page_size, total);
    let page_items = &deduped[start..end];

    let official_count = deduped.iter().filter(|r| r.source == "official").count();

    // Build header — mirrors lines 469-478
    let source_label = if source != "all" {
        format!("— {source}")
    } else {
        "— all sources".to_string()
    };
    let mut loaded_label = format!("{total} skills loaded");
    if !timed_out.is_empty() {
        loaded_label.push_str(&format!(", {} source(s) still loading", timed_out.len()));
    }
    c.print(&format!(
        "\n[bold]Skills Hub — Browse {source_label}[/]  [dim]({loaded_label}, page {page}/{total_pages})[/]"
    ));
    if official_count > 0 && page == 1 {
        c.print(&format!("[bright_cyan]★ {official_count} official optional skill(s) from Nous Research[/]"));
    }
    c.print("");

    // Build table — mirrors lines 481-510
    c.print("# | Name | Description | Source | Trust | Identifier");
    c.print(&"-".repeat(90));
    for (idx, r) in page_items.iter().enumerate() {
        let trust_style = match r.trust_level.as_str() {
            "builtin" => "bright_cyan",
            "trusted" => "green",
            "community" => "yellow",
            _ => "dim",
        };
        let trust_label = if r.source == "official" {
            "★ official".to_string()
        } else {
            r.trust_level.clone()
        };
        let desc = if r.description.len() > 44 {
            format!("{}...", &r.description[..44])
        } else {
            r.description.clone()
        };
        let num = start + idx + 1;
        c.print(&format!(
            "{num} | {} | {} | {} | [{trust_style}]{trust_label}[/] | {}",
            r.name,
            desc,
            display_source(r),
            r.identifier
        ));
    }

    // Navigation hints — mirrors lines 512-520
    let mut nav_parts: Vec<String> = Vec::new();
    if page > 1 {
        nav_parts.push(format!("[cyan]--page {}[/] ← prev", page - 1));
    }
    if page < total_pages {
        nav_parts.push(format!("[cyan]--page {}[/] → next", page + 1));
    }
    if !nav_parts.is_empty() {
        c.print(&format!("  {}", nav_parts.join(" | ")));
    }

    // Source summary — mirrors lines 522-525
    if source == "all" && !source_counts.is_empty() {
        let mut parts: Vec<String> = source_counts.iter().map(|(sid, ct)| format!("{sid}: {ct}")).collect();
        parts.sort();
        c.print(&format!("  [dim]Sources: {}[/]", parts.join(", ")));
    }

    if !timed_out.is_empty() {
        c.print(&format!(
            "  [yellow]⚡ Slow sources skipped: {} — run again for cached results[/]",
            timed_out.join(", ")
        ));
    }

    c.print(
        "[dim]Tip: 'hermes skills inspect <identifier>' to preview, 'hermes skills install <identifier>' to install, 'hermes skills search <query>' to search deeper[/]\n",
    );
}

fn parallel_search_sources_stub(
    _sources: &[Box<dyn SkillSource>],
    _query: &str,
    _per_source_limits: &HashMap<&str, usize>,
    _source_filter: &str,
    _overall_timeout: u64,
) -> (Vec<SkillRow>, HashMap<String, usize>, Vec<String>) {
    // Mirrors `parallel_search_sources(sources, query="", per_source_limits=..., source_filter=..., overall_timeout=30, on_source_done=...)`
    // Real parallel fetch across hermes-index / official / github / etc. in later slices.
    (Vec::new(), HashMap::new(), Vec::new())
}

fn is_provider_filter_value(source: &str) -> bool {
    // Mirrors `if source.strip().lower() in _PROVIDER_FILTER_VALUES` (435)
    // Provider values are per-tap provider labels (nvidia/openai/anthropic/...).
    // Stub set covers commonly seen providers; real set from `_PROVIDER_FILTER_VALUES` in later slice.
    matches!(
        source.trim().to_lowercase().as_str(),
        "nvidia" | "openai" | "anthropic" | "huggingface"
    )
}

fn filter_results_by_provider_stub(results: Vec<SkillRow>, provider: &str) -> Vec<SkillRow> {
    // Mirrors `_filter_results_by_provider(all_results, source)` (436)
    results
        .into_iter()
        .filter(|r| {
            if let Some(p) = r.extra.get("provider") {
                p.to_lowercase() == provider.trim().to_lowercase()
            } else {
                false
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// do_install — mirrors lines 536-846
// ---------------------------------------------------------------------------

/// Fetch, quarantine, scan, confirm, and install a skill.
///
/// `name_override` lets non-interactive callers (slash commands, gateway,
/// scripts) supply a skill name when the upstream `SKILL.md` lacks a valid
/// `name:` frontmatter field. On interactive TTY surfaces, a missing name
/// triggers a prompt instead; `skip_confirm=True` means "non-interactive"
/// (so pair it with `name_override` when installing from a URL that has
/// no frontmatter).
///
/// `source_id` pins resolution to a single source adapter (e.g. `clawhub`).
/// Callers that already know a skill's provenance — notably `do_update`,
/// which reads it from the lockfile — should pass it so a bare, slash-less
/// identifier cannot be fuzzy-resolved to a same-named skill in a different
/// registry. Skill names are not namespaced across registries, so an
/// unconstrained resolve can silently change a skill's provenance.
///
/// Mirrors `def do_install(identifier, category="", force=False, console=None, skip_confirm=False, invalidate_cache=True, name_override="", source_id=None)` (536-846).
#[allow(clippy::too_many_arguments)]
pub fn do_install(
    identifier: &str,
    category: &str,
    force: bool,
    console: Option<&Console>,
    skip_confirm: bool,
    invalidate_cache: bool,
    name_override: &str,
    source_id: Option<&str>,
) {
    // Mirrors imports: `from tools.skills_hub import GitHubAuth, create_source_router, ensure_hub_dirs, quarantine_bundle, ...` (557-561)
    // and `from tools.skills_guard import scan_skill_cached, should_allow_install, format_scan_report` (562)
    let c_owned;
    let c = match console {
        Some(v) => v,
        None => {
            c_owned = global_console();
            &c_owned
        }
    };

    ensure_hub_dirs_stub();

    let auth = GitHubAuth::new();
    let mut sources = create_source_router_stub(&auth);

    // Source pinning — mirrors lines 570-581
    if let Some(sid) = source_id {
        if !sid.is_empty() {
            let pinned: Vec<Box<dyn SkillSource>> = sources
                .into_iter()
                .filter(|s| source_matches_stub(s.as_ref(), sid))
                .collect();
            if !pinned.is_empty() {
                sources = pinned;
            } else {
                c.print(&format!(
                    "[bold red]Error:[/] no source adapter for '{sid}'. Refusing to resolve '{identifier}' against other registries (that would change the skill's provenance).\n"
                ));
                return;
            }
        } else {
            // empty source_id treated as None — fall through with all sources
            let _ = &sources;
        }
    }

    // Short-name resolve — mirrors `if "/" not in identifier: identifier = _resolve_short_name(...)` (583-587)
    let mut identifier_owned = identifier.to_string();
    if !identifier_owned.contains('/') {
        let resolved = resolve_short_name(&identifier_owned, &sources, c);
        if resolved.is_empty() {
            return;
        }
        identifier_owned = resolved;
    }

    c.print(&format!("\n[bold]Fetching:[/] {identifier_owned}"));

    // Mirrors `meta, bundle, _matched_source = _resolve_source_meta_and_bundle(identifier, sources)` (591)
    let (mut meta, mut bundle, _matched_source) =
        resolve_source_meta_and_bundle(&identifier_owned, &sources);

    if bundle.is_none() {
        // Check rate limiting — mirrors lines 594-611
        let rate_limited = sources.iter().any(|src| src.is_rate_limited() || src.github_rate_limited());
        c.print(&format!("[bold red]Error:[/] Could not fetch '{identifier_owned}' from any source."));
        if rate_limited {
            c.print(
                "[yellow]Hint:[/] GitHub API rate limit exhausted (unauthenticated: 60 requests/hour).\nSet [bold]GITHUB_TOKEN[/] in your .env or install the [bold]gh[/] CLI and run [bold]gh auth login[/] to raise the limit to 5,000/hr.\n",
            );
        } else {
            c.print("");
        }
        return;
    }

    // Unwrap bundle (now known Some) — mirrors lines 617-705
    let mut bundle_val = bundle.take().unwrap();

    // URL-sourced skills may arrive with empty name — mirrors lines 617-657
    let bundle_meta_awaiting = bundle_val
        .metadata
        .get("awaiting_name")
        .map(|v| v == "true")
        .unwrap_or(false);
    let bundle_name_empty = bundle_val.name.trim().is_empty();
    if bundle_val.source == "url" && (bundle_name_empty || bundle_meta_awaiting) {
        if !name_override.is_empty() && is_valid_installed_skill_name(name_override) {
            bundle_val.name = name_override.trim().to_string();
            bundle_val.metadata.insert("awaiting_name".to_string(), "false".to_string());
        } else if !name_override.is_empty() {
            c.print(&format!(
                "[bold red]Invalid --name:[/] {name_override:?}. Must be a lowercase identifier (letters, digits, hyphens, underscores; starts with a letter).\n"
            ));
            return;
        } else if skip_confirm {
            // Non-interactive — emit actionable error (lines 630-643)
            let url = bundle_val
                .metadata
                .get("url")
                .cloned()
                .unwrap_or_else(|| identifier_owned.clone());
            c.print(&format!(
                "[bold red]Cannot install from URL:[/] {url}\n[yellow]The SKILL.md has no `name:` in its frontmatter, and the URL path doesn't produce a valid identifier.[/]\n\nRetry with an explicit name:\n  [bold]/skills install {url} --name <your-name>[/]\n  [bold]hermes skills install {url} --name <your-name>[/]\n\n[dim]Or ask the SKILL.md's author to add a `name:` field to its YAML frontmatter.[/]\n"
            ));
            return;
        } else {
            // Interactive TTY — prompt (lines 644-657)
            let url = bundle_val
                .metadata
                .get("url")
                .cloned()
                .unwrap_or_else(|| identifier_owned.clone());
            match prompt_for_skill_name(c, &url, "") {
                None => {
                    c.print("[dim]Installation cancelled.[/]\n");
                    return;
                }
                Some(chosen) => {
                    bundle_val.name = chosen;
                    bundle_val.metadata.insert("awaiting_name".to_string(), "false".to_string());
                }
            }
        }
        // Keep SkillMeta in sync so downstream "already installed" checks, audit logs, and display see final name.
        // Mirrors lines 654-657
        if let Some(ref mut m) = meta {
            m.name = bundle_val.name.clone();
            m.path = bundle_val.name.clone();
        }
    }

    // URL category prompt — mirrors lines 662-663
    let mut category_owned = category.to_string();
    if bundle_val.source == "url" && category_owned.is_empty() && !skip_confirm {
        category_owned = prompt_for_category(c, &existing_categories());
    }

    // Official auto-category — mirrors lines 668-671
    if bundle_val.source == "official" && category_owned.is_empty() {
        let id_parts: Vec<&str> = bundle_val.identifier.split('/').collect();
        if id_parts.len() >= 3 {
            category_owned = id_parts[1..id_parts.len() - 1].join("/");
        }
    }

    // Already-installed check — mirrors lines 673-681
    let lock = HubLockFileStub::new();
    if let Some(existing) = lock.get_installed(&bundle_val.name) {
        let install_path = existing.get("install_path").cloned().unwrap_or_default();
        c.print(&format!(
            "[yellow]Warning:[/] '{}' is already installed at {install_path}",
            bundle_val.name
        ));
        if !force {
            c.print("Use --force to reinstall.\n");
            return;
        }
    }

    // Merge extra metadata — mirrors lines 683-684
    let mut extra_metadata: HashMap<String, String> = HashMap::new();
    if let Some(ref m) = meta {
        extra_metadata.extend(m.extra.clone());
    }
    extra_metadata.extend(bundle_val.metadata.clone());

    // Quarantine — mirrors lines 686-694
    let q_path = match quarantine_bundle_stub(&bundle_val) {
        Ok(p) => p,
        Err(exc) => {
            c.print(&format!("[bold red]Installation blocked:[/] {exc}\n"));
            append_audit_log_stub("BLOCKED", &bundle_val.name, &bundle_val.source, &bundle_val.trust_level, "invalid_path", &exc);
            return;
        }
    };
    // Mirrors `c.print(f"[dim]Quarantined to {q_path.relative_to(...)}[/]")` (694)
    // Compute relative to parent.parent.parent if possible; fallback to display
    let q_display = q_path.display().to_string();
    c.print(&format!("[dim]Quarantined to {q_display}[/]"));

    // Scan — mirrors lines 696-723
    c.print("[bold]Running security scan...[/]");
    let scan_source = if bundle_val.source == "official" {
        "official".to_string()
    } else {
        // mirrors `bundle.identifier or meta.identifier or identifier` (700-704)
        if !bundle_val.identifier.is_empty() {
            bundle_val.identifier.clone()
        } else if let Some(ref m) = meta {
            if !m.identifier.is_empty() {
                m.identifier.clone()
            } else {
                identifier_owned.clone()
            }
        } else {
            identifier_owned.clone()
        }
    };
    let _hub_dir = get_hermes_home().join("hub"); // mirrors `HUB_DIR` stub
    let source_url = source_url_for_bundle_stub(&bundle_val);
    let (result, scan_provenance) = scan_skill_cached_stub(&q_path, &scan_source, &source_url);

    c.print(&format_scan_report_stub(&result));
    let freshness = if scan_provenance.fresh { "fresh" } else { "cached" };
    c.print(&format!(
        "[dim]Scan provenance: {freshness}; scanner {}; hash {}[/]",
        scan_provenance.scanner_version, scan_provenance.bundle_hash
    ));
    let rules = if scan_provenance.rules.is_empty() {
        "none".to_string()
    } else {
        scan_provenance.rules.join(", ")
    };
    c.print(&format!(
        "[dim]Source: {}; scanned {}; rules: {rules}[/]",
        scan_provenance.source_url, scan_provenance.scanned_at
    ));

    // Install policy — mirrors lines 725-735
    let (allowed, reason) = should_allow_install_stub(&result, force);
    if !allowed {
        c.print(&format!("\n[bold red]Installation blocked:[/] {reason}"));
        let _ = std::fs::remove_dir_all(&q_path);
        append_audit_log_stub(
            "BLOCKED",
            &bundle_val.name,
            &bundle_val.source,
            &bundle_val.trust_level,
            &result.verdict,
            &format!("{}_findings", result.findings.len()),
        );
        return;
    }

    // Tier1 advisory — mirrors lines 737-741
    print_tier1_advisory(&q_path, c);

    // Upstream metadata panel — mirrors lines 743-746
    if !extra_metadata.is_empty() {
        let metadata_lines = format_extra_metadata_lines(&extra_metadata);
        if !metadata_lines.is_empty() {
            // Mirrors `c.print(Panel("\n".join(metadata_lines), title="Upstream Metadata", border_style="blue"))`
            c.print_panel(&metadata_lines.join("\n"), "Upstream Metadata", "blue");
        }
    }

    // Confirm with user — mirrors lines 748-779
    if !force && !skip_confirm {
        c.print("");
        if bundle_val.source == "official" {
            // Mirrors bright_cyan "Official Skill" panel (752-760)
            let cat_prefix = if category_owned.is_empty() {
                String::new()
            } else {
                format!("{category_owned}/")
            };
            c.print_panel(
                &format!(
                    "[bold bright_cyan]This is an official optional skill maintained by Nous Research.[/]\n\nIt ships with hermes-agent but is not activated by default.\nInstalling will copy it to your skills directory where the agent can use it.\n\nFiles will be at: [cyan]{}/skills/{}{}/[/]",
                    display_hermes_home(),
                    cat_prefix,
                    bundle_val.name
                ),
                "Official Skill",
                "bright_cyan",
            );
        } else {
            let cat_prefix = if category_owned.is_empty() {
                String::new()
            } else {
                format!("{category_owned}/")
            };
            c.print_panel(
                &format!(
                    "[bold yellow]You are installing a third-party skill at your own risk.[/]\n\nExternal skills can contain instructions that influence agent behavior,\n\
                     shell commands, and scripts. Even after automated scanning, you should\n\
                     review the installed files before use.\n\nFiles will be at: [cyan]{}/skills/{}{}/[/]",
                    display_hermes_home(),
                    cat_prefix,
                    bundle_val.name
                ),
                "Disclaimer",
                "yellow",
            );
        }
        c.print(&format!("[bold]Install '{}'?[/]", bundle_val.name));
        // Mirrors `answer = input("Confirm [y/N]: ").strip().lower()` (773)
        let answer = line_input_stub("Confirm [y/N]: ")
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        if !matches!(answer.as_str(), "y" | "yes") {
            c.print("[dim]Installation cancelled.[/]\n");
            let _ = std::fs::remove_dir_all(&q_path);
            return;
        }
    }

    // Install — mirrors lines 781-793
    let install_dir = match install_from_quarantine_stub(&q_path, &bundle_val.name, &category_owned, &bundle_val, &result) {
        Ok(p) => p,
        Err(exc) => {
            c.print(&format!("[bold red]Installation blocked:[/] {exc}\n"));
            let _ = std::fs::remove_dir_all(&q_path);
            append_audit_log_stub("BLOCKED", &bundle_val.name, &bundle_val.source, &bundle_val.trust_level, "invalid_path", &exc);
            return;
        }
    };
    let skills_dir = get_hermes_home().join("skills");
    let rel = install_dir
        .strip_prefix(&skills_dir)
        .unwrap_or(&install_dir)
        .display()
        .to_string()
        .replace('\\', "/");
    c.print(&format!("[bold green]Installed:[/] {rel}"));
    let file_list = bundle_val.files.keys().cloned().collect::<Vec<_>>().join(", ");
    c.print(&format!("[dim]Files: {file_list}[/]\n"));

    // Blueprint detection — mirrors lines 795-834
    // Never blocks install; best-effort suggestion registration.
    match blueprint_spec_for_installed_stub(&bundle_val.name) {
        Err(rec_err) if is_blueprint_present(&bundle_val) => {
            c.print(&format!("[yellow]Blueprint block present but invalid:[/] {rec_err}\n"));
        }
        Ok(Some(spec)) => {
            match register_blueprint_suggestion_stub(&spec) {
                Some(_) => {
                    c.print(&format!(
                        "[bold cyan]Blueprint:[/] '{}' is an automation (schedule [bold]{}[/]).",
                        bundle_val.name, spec.schedule
                    ));
                    c.print(
                        "[dim]Added to your suggestions — run[/] [bold]/suggestions[/] [dim]to schedule or dismiss it.[/]\n",
                    );
                }
                None => {
                    c.print(&format!(
                        "[bold cyan]Blueprint:[/] '{}' is an automation (schedule [bold]{}[/]), but it wasn't added to your suggestions (already offered/dismissed, or the pending list is full — run [bold]/suggestions[/] to review).",
                        bundle_val.name, spec.schedule
                    ));
                    c.print(
                        "[dim]You can still schedule it any time by asking the agent or via[/] [bold]hermes cron add[/][dim].[/]\n",
                    );
                }
            }
        }
        _ => {}
    }

    // Cache invalidation — mirrors lines 836-845
    if invalidate_cache {
        let _ = clear_skills_system_prompt_cache_stub(true);
    } else {
        c.print("[dim]Skill will be available in your next session.[/]");
        c.print(
            "[dim]Use /reset to start a new session now, or --now to activate immediately (invalidates prompt cache).[/]\n",
        );
    }
}

// Stub helpers for do_install — mirrors `tools.skills_hub` / `tools.skills_guard` / `agent.prompt_builder` / `tools.blueprints`

fn ensure_hub_dirs_stub() {
    let _ = std::fs::create_dir_all(get_hermes_home().join("skills"));
}

fn source_matches_stub(src: &dyn SkillSource, source_id: &str) -> bool {
    // Mirrors `tools.skills_hub._source_matches(src, source_id)` — checks id/name match
    src.id().to_lowercase() == source_id.to_lowercase()
}

#[derive(Debug, Clone, Default)]
struct HubLockFileStub {
    // Mirrors `tools.skills_hub.HubLockFile` — installed map
    installed: HashMap<String, HashMap<String, String>>,
}
impl HubLockFileStub {
    fn new() -> Self {
        Self { installed: HashMap::new() }
    }
    fn get_installed(&self, name: &str) -> Option<HashMap<String, String>> {
        self.installed.get(name).cloned()
    }
}

fn quarantine_bundle_stub(bundle: &SkillBundle) -> Result<PathBuf, String> {
    // Mirrors `quarantine_bundle(bundle)` (687) — validates paths, copies to quarantine
    // In slice 1 we stub the quarantine directory under `~/.hermes/.hub/quarantine/<name>`
    // and validate that `bundle.files` keys don't escape the bundle root (path traversal guard).
    for rel in bundle.files.keys() {
        let p = Path::new(rel);
        if p.is_absolute() || rel.contains("..") {
            return Err(format!("invalid path in bundle: {rel}"));
        }
    }
    let q_path = get_hermes_home().join(".hub").join("quarantine").join(&bundle.name);
    Ok(q_path)
}

fn source_url_for_bundle_stub(bundle: &SkillBundle) -> String {
    // Mirrors `source_url_for_bundle(bundle)` — builds URL for scan provenance
    bundle.metadata.get("url").cloned()
        .or_else(|| bundle.metadata.get("repo_url").cloned())
        .unwrap_or_else(|| bundle.identifier.clone())
}

#[derive(Debug, Clone)]
struct ScanResultStub {
    verdict: String,
    findings: Vec<String>,
}
#[derive(Debug, Clone)]
struct ScanProvenanceStub {
    fresh: bool,
    scanner_version: String,
    bundle_hash: String,
    source_url: String,
    scanned_at: String,
    rules: Vec<String>,
}

fn scan_skill_cached_stub(
    _q_path: &Path,
    _source: &str,
    source_url: &str,
) -> (ScanResultStub, ScanProvenanceStub) {
    // Mirrors `scan_skill_cached(q_path, source=..., source_url=..., cache_dir=HUB_DIR/"scan-cache")`
    // Slice 1 stub: return clean verdict
    (
        ScanResultStub { verdict: "safe".to_string(), findings: Vec::new() },
        ScanProvenanceStub {
            fresh: true,
            scanner_version: "stub".to_string(),
            bundle_hash: "00000000".to_string(),
            source_url: source_url.to_string(),
            scanned_at: "now".to_string(),
            rules: Vec::new(),
        },
    )
}

fn format_scan_report_stub(result: &ScanResultStub) -> String {
    format!("[scan] verdict={} findings={}", result.verdict, result.findings.len())
}

fn should_allow_install_stub(result: &ScanResultStub, force: bool) -> (bool, String) {
    // Mirrors `should_allow_install(result, force=force)` — blocks dangerous unless force
    if result.verdict == "dangerous" && !force {
        return (false, "scan verdict is dangerous".to_string());
    }
    (true, String::new())
}

fn append_audit_log_stub(action: &str, name: &str, source: &str, trust: &str, verdict: &str, details: &str) {
    log_debug(&format!("audit {action} {name} {source} {trust} {verdict} {details}"));
}

fn install_from_quarantine_stub(
    q_path: &Path,
    name: &str,
    category: &str,
    _bundle: &SkillBundle,
    _result: &ScanResultStub,
) -> Result<PathBuf, String> {
    // Mirrors `install_from_quarantine(q_path, bundle.name, category, bundle, result)` (783)
    // Validates category doesn't escape, then returns the install dir.
    if category.contains("..") || category.contains("//") {
        return Err(format!("invalid category: {category}"));
    }
    let skills_dir = get_hermes_home().join("skills");
    let install_dir = if category.is_empty() {
        skills_dir.join(name)
    } else {
        skills_dir.join(category).join(name)
    };
    // Ensure quarantine path looks plausible (stub — don't actually copy)
    let _ = q_path;
    Ok(install_dir)
}

#[derive(Debug, Clone)]
struct BlueprintSpecStub {
    schedule: String,
}
fn is_blueprint_present(_bundle: &SkillBundle) -> bool {
    false
}
fn blueprint_spec_for_installed_stub(_name: &str) -> Result<Option<BlueprintSpecStub>, String> {
    // Mirrors `blueprint_spec_for_installed(bundle.name)` (804) — returns spec if skill has blueprint block
    Ok(None)
}
fn register_blueprint_suggestion_stub(_spec: &BlueprintSpecStub) -> Option<String> {
    // Mirrors `register_blueprint_suggestion(spec)` — returns Some(id) if registered
    None
}
fn clear_skills_system_prompt_cache_stub(_clear_snapshot: bool) -> Result<(), String> {
    // Mirrors `clear_skills_system_prompt_cache(clear_snapshot=True)` (839)
    Ok(())
}

// ---------------------------------------------------------------------------
// do_inspect — mirrors lines 848-895
// ---------------------------------------------------------------------------

/// Preview a skill's `SKILL.md` content without installing.
///
/// Mirrors `def do_inspect(identifier, console=None)` (848-895).
pub fn do_inspect(identifier: &str, console: Option<&Console>) {
    // Mirrors `from tools.skills_hub import GitHubAuth, create_source_router` (850)
    let c_owned;
    let c = match console {
        Some(v) => v,
        None => {
            c_owned = global_console();
            &c_owned
        }
    };

    let auth = GitHubAuth::new();
    let sources = create_source_router_stub(&auth);

    // Short-name resolve — mirrors lines 856-859
    let mut ident = identifier.to_string();
    if !ident.contains('/') {
        let resolved = resolve_short_name(&ident, &sources, c);
        if resolved.is_empty() {
            return;
        }
        ident = resolved;
    }

    let (meta, bundle, _matched) = resolve_source_meta_and_bundle(&ident, &sources);

    let meta_val = match meta {
        Some(m) => m,
        None => {
            c.print(&format!("[bold red]Error:[/] Could not find '{ident}' in any source.\n"));
            return;
        }
    };

    c.print("");
    let trust_style = match meta_val.trust_level.as_str() {
        "builtin" => "bright_cyan",
        "trusted" => "green",
        "community" => "yellow",
        _ => "dim",
    };
    let trust_label = if meta_val.source == "official" {
        "official".to_string()
    } else {
        meta_val.trust_level.clone()
    };

    // Build info lines — mirrors lines 871-882
    let mut info_lines: Vec<String> = vec![
        format!("[bold]Name:[/] {}", meta_val.name),
        format!("[bold]Description:[/] {}", meta_val.description),
        format!("[bold]Source:[/] {}", meta_val.source),
        format!("[bold]Trust:[/] [{trust_style}]{trust_label}[/]"),
        format!("[bold]Identifier:[/] {}", meta_val.identifier),
    ];
    if !meta_val.tags.is_empty() {
        info_lines.push(format!("[bold]Tags:[/] {}", meta_val.tags.join(", ")));
    }
    info_lines.extend(format_extra_metadata_lines(&meta_val.extra));

    c.print_panel(&info_lines.join("\n"), &format!("Skill: {}", meta_val.name), "default");

    // SKILL.md preview — mirrors lines 884-893
    if let Some(b) = bundle {
        if let Some(content_bytes) = b.files.get("SKILL.md") {
            let content = String::from_utf8_lossy(content_bytes).to_string();
            let lines: Vec<&str> = content.split('\n').collect();
            let preview = if lines.len() > 50 {
                let head = lines[..50].join("\n");
                format!("{head}\n\n... ({} more lines)", lines.len() - 50)
            } else {
                lines.join("\n")
            };
            c.print_panel(&preview, "SKILL.md Preview", "default");
            // Mirrors `subtitle="hermes skills install <id> to install"` — shown as hint line
            c.print("[dim]hermes skills install <id> to install[/]");
        }
    }

    c.print("");
}

// ---------------------------------------------------------------------------
// Re-export helpers for do_inspect / do_install callers (used by slice2)
// ---------------------------------------------------------------------------

/// Mirrors `trust_style` mapping used across do_search/do_browse/do_inspect tables.
/// Exported for slice2 reuse without duplication.
pub fn trust_style_for(trust: &str) -> &'static str {
    match trust {
        "builtin" => "bright_cyan",
        "trusted" => "green",
        "community" => "yellow",
        _ => "dim",
    }
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `skills_hub.py` lines 898-2127 (`browse_skills`, `inspect_skill`,
// `do_list`, `do_check`, `do_update`, `do_audit`, `do_uninstall`,
// `do_reset`, `do_list_modified`, `do_diff`, `do_opt_out`, `do_opt_in`,
// `do_repair_official`, `do_tap`, `do_publish`, `_github_publish`,
// `do_snapshot_export`, `do_snapshot_import`, `skills_command`,
// `handle_skills_slash`, `_print_skills_help`) continue in
// `skills_hub_slice2.rs` (from `browse_skills`, line 898) and
// `skills_hub_slice3.rs`.
// This file intentionally stops at the 900-line boundary (inside
// `do_inspect`'s preview rendering, line 895 — the next def starts at
// 898) so that `cargo` is never invoked and the 3-slice decomposition
// stays clean.
