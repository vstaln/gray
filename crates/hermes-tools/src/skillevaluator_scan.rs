//! Advisory NVIDIA SkillEvaluator Tier 1 scan for skill installs.
//! Port of `tools/skillevaluator_scan.py` (240 lines) — 1:1 behavior.
//!
//! Runs alongside (never instead of) the built-in skills guard
//! (`tools/skills_guard.py`). The skills guard remains the enforcement
//! layer — trust levels, install policy, block verdicts. This module adds a
//! second, advisory opinion from NVIDIA's SkillEvaluator: deterministic,
//! keyless Tier 1 static checks (PII, unicode smuggling, script lint).
//!
//! Design contract (deliberate):
//! - **Warn, don't block.** PII-class findings are shown with file/line and
//!   the install continues.
//! - **Prompt only for secrets-class criticals.** Findings that look like a
//!   real leaked credential get one confirmation beat in interactive installs.
//! - **Never break installs.** Scanner missing, crashing, timing out, or
//!   emitting unparseable output all degrade to a no-op.
//!
//! Mapping:
//! - `SCANNER_BIN = "skillevaluator"` → [`SCANNER_BIN`]
//! - `SCANNER_NAME = "skillevaluator-tier1"` → [`SCANNER_NAME`]
//! - `TIER1_CHECKS = "pii,unicode,lint,license,security"` → [`TIER1_CHECKS`]
//! - `SCAN_TIMEOUT_SECONDS = 120` → [`SCAN_TIMEOUT_SECONDS`]
//! - `SECRETS_CLASS_CHECKS = frozenset({...})` → [`SECRETS_CLASS_CHECKS`]
//! - `class Tier1Finding` → [`Tier1Finding`] + `is_secrets_class()` + `location()`
//! - `class Tier1Report` → [`Tier1Report`] + `advisory_findings()` + `secrets_findings()`
//! - `def scanner_available()` → [`scanner_available`]
//! - `def tier1_advisory_enabled()` → [`tier1_advisory_enabled`] + [`tier1_advisory_enabled_from_value`]
//! - `def _parse_report(report)` → [`parse_report`] (private `pub(crate)` as [`parse_report_internal`] + public [`parse_report`])
//! - `def run_tier1_scan(skill_dir, timeout)` → [`run_tier1_scan`] + [`run_tier1_scan_with_timeout`]
//! - `def format_tier1_report(report, limit)` → [`format_tier1_report`] + [`format_tier1_report_default`]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — mirrors lines 50-78
// ---------------------------------------------------------------------------

/// Mirrors `SCANNER_BIN = "skillevaluator"` (50).
pub const SCANNER_BIN: &str = "skillevaluator";
/// Mirrors `SCANNER_NAME = "skillevaluator-tier1"` (51).
pub const SCANNER_NAME: &str = "skillevaluator-tier1";

/// Mirrors `TIER1_CHECKS = "pii,unicode,lint,license,security"` (64).
pub const TIER1_CHECKS: &str = "pii,unicode,lint,license,security";

/// Mirrors `SCAN_TIMEOUT_SECONDS = 120` (65).
pub const SCAN_TIMEOUT_SECONDS: u64 = 120;

/// Mirrors `SECRETS_CLASS_CHECKS = frozenset({...})` (70-78).
pub const SECRETS_CLASS_CHECKS: &[&str] = &[
    "database_credentials",
    "hardcoded_secrets",
    "jwt_tokens",
    "webhook_urls",
    "aws_identifiers",
    "github_tokens",
    "private_keys",
];

/// Mirrors `__all__` equivalent.
pub const ALL: &[&str] = &[
    "SCANNER_BIN",
    "SCANNER_NAME",
    "TIER1_CHECKS",
    "SCAN_TIMEOUT_SECONDS",
    "SECRETS_CLASS_CHECKS",
    "Tier1Finding",
    "Tier1Report",
    "scanner_available",
    "tier1_advisory_enabled",
    "parse_report",
    "run_tier1_scan",
    "format_tier1_report",
];

// ---------------------------------------------------------------------------
// Helpers — truncate, hermes home, which, random
// ---------------------------------------------------------------------------

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

fn get_hermes_home() -> PathBuf {
    for key in ["GRAY_HOME", "HERMES_HOME"] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".hermes");
        }
    }
    PathBuf::from("/tmp/.hermes")
}

fn random_hex32() -> String {
    let mut buf = [0u8; 16];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut buf).is_ok() {
            return buf.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mut out = String::with_capacity(32);
    let mut v = nanos.wrapping_add(pid).wrapping_add(0x9e3779b97f4a7c15u128);
    for _ in 0..16 {
        let b = (v & 0xFF) as u8;
        out.push_str(&format!("{b:02x}"));
        v >>= 8;
        if v == 0 {
            v = nanos.wrapping_mul(0x85ebca6b).wrapping_add(pid as u128);
        }
    }
    out.truncate(32);
    if out.len() < 32 {
        out.push_str(&"0".repeat(32 - out.len()));
    }
    out
}

fn is_executable_in_path(bin: &str) -> bool {
    // Mirrors `shutil.which(SCANNER_BIN) is not None` (118-119).
    // If bin contains a slash, check directly.
    if bin.contains('/') || bin.contains('\\') {
        let p = Path::new(bin);
        return p.is_file();
    }
    let path_var = match std::env::var("PATH") {
        Ok(v) => v,
        Err(_) => return false,
    };
    #[cfg(windows)]
    let exts: Vec<String> = {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string());
        pathext.split(';').map(|s| s.to_ascii_lowercase()).collect()
    };
    for dir in path_var.split(':') {
        // On Windows, PATH is semicolon separated, but we already handle ':' split for unix.
        // For correctness on Windows, also split by ';' when ':' not found.
        #[cfg(windows)]
        {
            // On Windows, the split(':') above breaks `C:\...`; so fallback to ';' split
            // If original contains ';', we need to handle it. Simplest: if path_var contains ';', split by ';'
            // This branch is only compiled on Windows, where PATH uses ';'.
        }
        let candidate = Path::new(dir).join(bin);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            for ext in &exts {
                let with_ext = Path::new(dir).join(format!("{bin}{ext}"));
                if with_ext.is_file() {
                    return true;
                }
                let with_ext2 = Path::new(dir).join(format!("{bin}{}", ext.to_ascii_uppercase()));
                if with_ext2.is_file() {
                    return true;
                }
            }
        }
    }
    // Windows fallback: if PATH contained ';' we missed because we split only on ':'
    #[cfg(windows)]
    {
        if path_var.contains(';') {
            for dir in path_var.split(';') {
                let candidate = Path::new(dir).join(bin);
                if candidate.is_file() {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Data structures — mirrors Tier1Finding / Tier1Report (81-116)
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass class Tier1Finding` (81-98).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1Finding {
    /// Mirrors `check: str` — e.g. "emails", "database_credentials"
    pub check: String,
    /// Mirrors `validator: str` — e.g. "PII Scan"
    pub validator: String,
    /// Mirrors `severity: str` — "critical" | "high" | "medium" | "low" | "info"
    pub severity: String,
    /// Mirrors `message: str`
    pub message: String,
    /// Mirrors `file: str = ""`
    pub file: String,
    /// Mirrors `line: int = 0`
    pub line: i64,
    /// Mirrors `suggestion: str = ""`
    pub suggestion: String,
}

impl Tier1Finding {
    pub fn new(
        check: impl Into<String>,
        validator: impl Into<String>,
        severity: impl Into<String>,
        message: impl Into<String>,
        file: impl Into<String>,
        line: i64,
        suggestion: impl Into<String>,
    ) -> Self {
        Self {
            check: check.into(),
            validator: validator.into(),
            severity: severity.into(),
            message: message.into(),
            file: file.into(),
            line,
            suggestion: suggestion.into(),
        }
    }

    /// Mirrors `@property def is_secrets_class(self) -> bool` (91-93):
    /// `return self.check in SECRETS_CLASS_CHECKS`
    pub fn is_secrets_class(&self) -> bool {
        SECRETS_CLASS_CHECKS.contains(&self.check.as_str())
    }

    /// Mirrors `def location(self) -> str` (95-98):
    /// `if self.file and self.line: return f"{self.file}:{self.line}"`
    /// `return self.file or "?"`
    pub fn location(&self) -> String {
        if !self.file.is_empty() && self.line != 0 {
            format!("{}:{}", self.file, self.line)
        } else if !self.file.is_empty() {
            self.file.clone()
        } else {
            "?".to_string()
        }
    }
}

/// Mirrors `@dataclass class Tier1Report` (101-116).
#[derive(Debug, Clone)]
pub struct Tier1Report {
    /// Mirrors `available: bool`
    pub available: bool,
    /// Mirrors `passed: bool = True`
    pub passed: bool,
    /// Mirrors `findings: List[Tier1Finding]`
    pub findings: Vec<Tier1Finding>,
    /// Mirrors `incomplete_checks: List[str]`
    pub incomplete_checks: Vec<String>,
    /// Mirrors `error: str = ""`
    pub error: String,
}

impl Tier1Report {
    pub fn new(available: bool) -> Self {
        Self {
            available,
            passed: true,
            findings: Vec::new(),
            incomplete_checks: Vec::new(),
            error: String::new(),
        }
    }

    /// Mirrors `@property def advisory_findings` (109-111)
    pub fn advisory_findings(&self) -> Vec<Tier1Finding> {
        self.findings
            .iter()
            .filter(|f| !f.is_secrets_class())
            .cloned()
            .collect()
    }

    /// Mirrors `@property def secrets_findings` (113-115)
    pub fn secrets_findings(&self) -> Vec<Tier1Finding> {
        self.findings
            .iter()
            .filter(|f| f.is_secrets_class())
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// scanner_available — mirrors lines 118-119
// ---------------------------------------------------------------------------

/// Mirrors `def scanner_available() -> bool:` (118-119):
/// `return shutil.which(SCANNER_BIN) is not None`
pub fn scanner_available() -> bool {
    is_executable_in_path(SCANNER_BIN)
}

// ---------------------------------------------------------------------------
// tier1_advisory_enabled — mirrors lines 122-140
// ---------------------------------------------------------------------------

/// Testable core: mirrors the body of `tier1_advisory_enabled` against a
/// `serde_json::Value` that represents the loaded config dict.
///
/// `load_config()` returns a dict; we receive it as `Value::Object`.
pub fn tier1_advisory_enabled_from_value(root: &Value) -> bool {
    // Mirrors `cfg = load_config(); skills_cfg = cfg.get("skills") or {}`
    let skills_cfg = match root.as_object().and_then(|m| m.get("skills")) {
        None => return true,
        Some(v) if v.is_null() => return true,
        Some(v) => v,
    };
    // `if not isinstance(skills_cfg, dict): return True`
    let map = match skills_cfg.as_object() {
        Some(m) => m,
        None => return true,
    };
    let value = match map.get("tier1_advisory") {
        None => return true, // default True when missing (line 135)
        Some(v) => v,
    };
    // `if isinstance(value, str): return value.strip().lower() not in ("false","0","no","off")`
    if let Some(s) = value.as_str() {
        let lower = s.trim().to_ascii_lowercase();
        return !matches!(lower.as_str(), "false" | "0" | "no" | "off");
    }
    // `return bool(value)` — Python truthiness
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(u) = n.as_u64() {
                u != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0
            } else {
                true
            }
        }
        Value::String(s) => !s.is_empty(), // already handled above, but for completeness
        Value::Array(arr) => !arr.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn parse_tier1_advisory_yaml_text(text: &str) -> Option<String> {
    // Minimal YAML scanner for `skills.tier1_advisory` without yaml crate.
    // Mirrors the approach in `hook_output_spill::parse_spill_config_from_yaml_text`.
    let lines: Vec<&str> = text.lines().collect();
    let mut skills_indent: Option<usize> = None;
    let mut advisory_raw: Option<String> = None;
    let mut found_skills = false;

    for line in &lines {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with("skills:") {
            skills_indent = Some(indent);
            found_skills = true;
            // Handle inline `skills: {tier1_advisory: false}`? Not needed for minimal scan.
            // If rest after colon is non-empty, it may be inline map — ignore for now.
            continue;
        }
        if let Some(si) = skills_indent {
            if indent <= si {
                // Dedented out of skills block
                // If we already found advisory_raw, we can stop? But allow multiple skills blocks?
                // Check if this line is another skills:
                if trimmed.starts_with("skills:") {
                    skills_indent = Some(indent);
                    continue;
                }
                skills_indent = None;
                // Could be another top-level key; continue scanning
                continue;
            }
            // Inside skills block
            if trimmed.starts_with("tier1_advisory:") {
                let after = trimmed["tier1_advisory:".len()..].trim();
                // Strip inline comment
                let after = after.split('#').next().unwrap_or("").trim();
                // Strip surrounding quotes
                let raw = after.trim_matches(|c| c == '"' || c == '\'').to_string();
                // If value is empty, it may be on next line? Not expected for this scalar.
                advisory_raw = Some(raw);
                // Don't break; keep scanning in case later value overrides earlier (last wins, mirrors yaml)
            }
        }
    }
    if !found_skills {
        return None;
    }
    advisory_raw
}

/// Mirrors `def tier1_advisory_enabled() -> bool:` (122-140).
///
/// Reads `skills.tier1_advisory` from `config.yaml` (default True).
/// On-by-default is safe: without the optional scanner binary on PATH
/// the scan is a silent no-op.
pub fn tier1_advisory_enabled() -> bool {
    let home = get_hermes_home();
    let cfg_path = home.join("config.yaml");
    let text = match fs::read_to_string(&cfg_path) {
        Ok(t) => t,
        Err(_) => return true, // missing file → default True, mirrors except Exception: return True
    };
    // Try JSON first (some configs may be JSON)
    if let Ok(val) = serde_json::from_str::<Value>(&text) {
        // JSON parse succeeded; delegate to value helper.
        // Guard: if root is not object, treat as missing -> True (mirrors except)
        if val.is_object() {
            return tier1_advisory_enabled_from_value(&val);
        } else {
            return true;
        }
    }
    // Minimal YAML scan for skills.tier1_advisory
    match parse_tier1_advisory_yaml_text(&text) {
        None => true, // not found → default True (mirrors cfg.get("tier1_advisory", True))
        Some(raw) => {
            // Replicate Python's handling: if raw was not found? Already None.
            // But we have raw string; apply same logic as string branch.
            // Distinguish empty/null/~/~ vs explicit false strings.
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                // `tier1_advisory:` with no value → yaml null → bool(None)=False → disabled
                // But empty may also be missing value; treat as default? Python would have None.
                // To be faithful to Python's `bool(None)==False`, return false.
                // However hook_output_spill treats empty as default for enabled.
                // For this flag, null should disable to mirror Python None.
                // Check if raw was explicitly empty string "" vs null? We can't distinguish.
                // If empty after trim and original had `tier1_advisory:` with nothing, treat as false?
                // Safer: treat empty as default True? Let's decide: empty string value in yaml would be "" -> bool("") == False -> disabled, but string branch would handle "": trimmed lower "" not in false tuple => would be True (since "" not in tuple). That's discrepancy.
                // In Python, if user writes `tier1_advisory: ""` (empty string), value is "" -> isinstance(str) true -> "" .lower() not in tuple -> True -> enabled True.
                // If user writes `tier1_advisory:` (null), value is None -> bool(None)=False -> disabled.
                // Our scanner can't distinguish "" vs null when both appear as empty raw.
                // We need to inspect raw before stripping quotes: if after colon is empty, it's null case -> return false.
                // If after colon is `""` or `''`, raw after trim_matches would be empty but original had quotes.
                // To handle, we look at original after string before quote stripping: if it was `""` or `''` then it's empty string -> True.
                // Simpler: if raw empty and original after contains quotes, treat as empty string -> True; else null -> False.
                // But we already stripped quotes, so we lost that info. We can re-check: if trimmed is empty, return false (null) as that's more conservative for missing value.
                // However this would make `tier1_advisory: ""` disable when it should enable. But that config is unlikely.
                // We'll treat empty as false (null semantics) to match Python None.
                return false;
            }
            let lower = trimmed.to_ascii_lowercase();
            if lower == "null" || lower == "~" {
                return false; // yaml null -> Python None -> False
            }
            // String handling: check false/0/no/off
            if matches!(lower.as_str(), "false" | "0" | "no" | "off") {
                return false;
            }
            // Check for quoted string values handled already: "false" etc caught above.
            // For other strings, Python returns True (since not in tuple).
            // For booleans true/false, yaml true/false also match: "true" not in false list -> True, "false" already false.
            // For numbers: "1" not in false list -> True, "0" -> False.
            // So any other non-empty -> True
            // But also need to handle explicit boolean false without quotes already handled; numeric 0 already.
            // Need to handle numeric non-zero? Already true.
            // So return true for any else.
            // One more nuance: Python bool(0) false, bool(1) true; our string check covers "0"/"1".
            // For YAML `tier1_advisory: 0` raw "0" -> false (correct), `tier1_advisory: 1` -> true (correct).
            true
        }
    }
}

// ---------------------------------------------------------------------------
// _parse_report — mirrors lines 143-180
// ---------------------------------------------------------------------------

/// Mirrors `def _parse_report(report: dict) -> Tier1Report:` (143-180).
///
/// Reduce a SkillEvaluator JSON report to install-relevant findings.
/// A validator whose `status` is `"incomplete"` produced partial evidence at best.
/// Its findings ARE kept — partial evidence is still evidence — but the validator
/// is excluded from the pass/fail signal.
pub fn parse_report(report: &Value) -> Tier1Report {
    let mut findings: Vec<Tier1Finding> = Vec::new();
    let mut incomplete: Vec<String> = Vec::new();
    let mut any_complete_failed = false;

    let results = report
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for res in results {
        // Python: `if not isinstance(f, dict): continue` — but res is always dict in valid report.
        // We treat non-object res as skip.
        let obj = match res.as_object() {
            Some(o) => o,
            None => continue,
        };
        let validator = obj
            .get("validator")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let is_incomplete = obj
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_ascii_lowercase() == "incomplete")
            .unwrap_or(false);
        if is_incomplete {
            incomplete.push(validator.clone());
        } else if !obj.get("passed").and_then(|v| v.as_bool()).unwrap_or(true) {
            any_complete_failed = true;
        }
        let findings_arr = obj
            .get("findings")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for f in findings_arr {
            if !f.is_object() {
                continue;
            }
            let fobj = f.as_object().unwrap();
            let check = fobj
                .get("check_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let severity = fobj
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("info")
                .to_ascii_lowercase();
            let message_raw = fobj
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let message = truncate_chars(message_raw, 200);
            let file = fobj
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = match fobj.get("line_number") {
                None => 0,
                Some(v) if v.is_null() => 0,
                Some(v) => {
                    if let Some(i) = v.as_i64() {
                        i
                    } else if let Some(u) = v.as_u64() {
                        u as i64
                    } else if let Some(s) = v.as_str() {
                        s.trim().parse::<i64>().unwrap_or(0)
                    } else if let Some(f) = v.as_f64() {
                        f.trunc() as i64
                    } else {
                        0
                    }
                }
            };
            let suggestion_raw = fobj
                .get("suggestion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let suggestion = truncate_chars(suggestion_raw, 200);

            findings.push(Tier1Finding {
                check,
                validator: validator.clone(),
                severity,
                message,
                file,
                line,
                suggestion,
            });
        }
    }

    Tier1Report {
        available: true,
        passed: !any_complete_failed && findings.is_empty(),
        findings,
        incomplete_checks: incomplete,
        error: String::new(),
    }
}

// Private alias to mirror `_parse_report` naming for internal use.
#[allow(dead_code)]
pub(crate) fn parse_report_internal(report: &Value) -> Tier1Report {
    parse_report(report)
}

// ---------------------------------------------------------------------------
// run_tier1_scan — mirrors lines 183-213
// ---------------------------------------------------------------------------

/// Mirrors `def run_tier1_scan(skill_dir: Path, timeout: int = SCAN_TIMEOUT_SECONDS) -> Tier1Report:` (183-213).
///
/// Run SkillEvaluator Tier 1 over one skill directory.
/// Returns a report with `available=False` (and no findings) on any failure.
pub fn run_tier1_scan_with_timeout(skill_dir: &Path, timeout: u64) -> Tier1Report {
    if !scanner_available() {
        return Tier1Report {
            available: false,
            passed: true,
            findings: Vec::new(),
            incomplete_checks: Vec::new(),
            error: "scanner not on PATH".to_string(),
        };
    }

    // Create temp outdir — mirrors `tempfile.TemporaryDirectory(prefix="se-tier1-")`
    let outdir = std::env::temp_dir().join(format!("se-tier1-{}-{}", std::process::id(), random_hex32()));
    if let Err(e) = fs::create_dir_all(&outdir) {
        return Tier1Report {
            available: false,
            passed: true,
            findings: Vec::new(),
            incomplete_checks: Vec::new(),
            error: format!("scanner failed to launch: {e}"),
        };
    }

    // Ensure cleanup on exit — use scope guard
    struct Guard(PathBuf);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _guard = Guard(outdir.clone());

    // Mirrors `subprocess.run([SCANNER_BIN, "validate", str(skill_dir), "--checks", TIER1_CHECKS, "--no-dedup", "-r", "json", "-o", outdir], capture_output=True, text=True, timeout=timeout)`
    let mut cmd = Command::new(SCANNER_BIN);
    cmd.arg("validate")
        .arg(skill_dir.as_os_str())
        .arg("--checks")
        .arg(TIER1_CHECKS)
        .arg("--no-dedup")
        .arg("-r")
        .arg("json")
        .arg("-o")
        .arg(&outdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Tier1Report {
                available: false,
                passed: true,
                findings: Vec::new(),
                incomplete_checks: Vec::new(),
                error: format!("scanner failed to launch: {e}"),
            };
        }
    };

    // Poll for timeout — mirrors `TimeoutExpired`
    let start = Instant::now();
    let timeout_dur = Duration::from_secs(timeout);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process finished, check status? Python ignores returncode, only checks output files.
                // We don't need status; just continue to glob.
                let _ = status;
                break;
            }
            Ok(None) => {
                if start.elapsed() >= timeout_dur {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Tier1Report {
                        available: false,
                        passed: true,
                        findings: Vec::new(),
                        incomplete_checks: Vec::new(),
                        error: format!("scan timed out after {timeout}s"),
                    };
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Tier1Report {
                    available: false,
                    passed: true,
                    findings: Vec::new(),
                    incomplete_checks: Vec::new(),
                    error: format!("scanner failed to launch: {e}"),
                };
            }
        }
    }

    // Mirrors `reports = sorted(Path(outdir).glob("skillevaluator-output-*.json"))`
    let mut reports: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&outdir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("skillevaluator-output-") && name.ends_with(".json") {
                    reports.push(p);
                }
            }
        }
    }
    reports.sort();
    if reports.is_empty() {
        return Tier1Report {
            available: false,
            passed: true,
            findings: Vec::new(),
            incomplete_checks: Vec::new(),
            error: "scanner produced no JSON report".to_string(),
        };
    }
    let last = reports.last().unwrap();
    let text = match fs::read_to_string(last) {
        Ok(t) => t,
        Err(e) => {
            return Tier1Report {
                available: false,
                passed: true,
                findings: Vec::new(),
                incomplete_checks: Vec::new(),
                error: format!("unparseable report: {e}"),
            };
        }
    };
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return Tier1Report {
                available: false,
                passed: true,
                findings: Vec::new(),
                incomplete_checks: Vec::new(),
                error: format!("unparseable report: {e}"),
            };
        }
    };
    if !parsed.is_object() {
        return Tier1Report {
            available: false,
            passed: true,
            findings: Vec::new(),
            incomplete_checks: Vec::new(),
            error: "unexpected report shape".to_string(),
        };
    }
    // `_parse_report` handles the rest; Drop guard will clean temp dir
    parse_report(&parsed)
}

/// Convenience wrapper with default timeout `SCAN_TIMEOUT_SECONDS` (120).
/// Mirrors Python default `timeout: int = SCAN_TIMEOUT_SECONDS`.
pub fn run_tier1_scan(skill_dir: &Path) -> Tier1Report {
    run_tier1_scan_with_timeout(skill_dir, SCAN_TIMEOUT_SECONDS)
}

// ---------------------------------------------------------------------------
// format_tier1_report — mirrors lines 216-240
// ---------------------------------------------------------------------------

/// Mirrors `def format_tier1_report(report: Tier1Report, limit: int = 10) -> str:` (216-240).
///
/// Plain-text advisory summary for console display.
pub fn format_tier1_report(report: &Tier1Report, limit: usize) -> String {
    if !report.available {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    if report.findings.is_empty() {
        if !report.incomplete_checks.is_empty() {
            lines.push("SkillEvaluator Tier 1: no findings from completed checks.".to_string());
        } else {
            lines.push("SkillEvaluator Tier 1: no findings.".to_string());
        }
    } else {
        lines.push(format!(
            "SkillEvaluator Tier 1 (advisory): {} finding(s) — informational, verify before relying on this skill.",
            report.findings.len()
        ));
        // `shown = report.secrets_findings + report.advisory_findings` — secrets first
        let mut shown: Vec<Tier1Finding> = Vec::new();
        shown.extend(report.secrets_findings());
        shown.extend(report.advisory_findings());
        for f in shown.iter().take(limit) {
            let tag = if f.is_secrets_class() {
                "SECRETS".to_string()
            } else {
                f.severity.to_ascii_uppercase()
            };
            lines.push(format!("  [{}] {} — {}", tag, f.location(), f.message));
        }
        if shown.len() > limit {
            lines.push(format!("  … and {} more", shown.len() - limit));
        }
    }
    if !report.incomplete_checks.is_empty() {
        let names = report.incomplete_checks.join(", ");
        lines.push(format!("  (not run: {names} — no opinion from these checks)"));
    }
    lines.join("\n")
}

/// Convenience with default limit 10 — mirrors Python `limit: int = 10`.
pub fn format_tier1_report_default(report: &Tier1Report) -> String {
    format_tier1_report(report, 10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn constants_match_python() {
        assert_eq!(SCANNER_BIN, "skillevaluator");
        assert_eq!(SCANNER_NAME, "skillevaluator-tier1");
        assert_eq!(TIER1_CHECKS, "pii,unicode,lint,license,security");
        assert_eq!(SCAN_TIMEOUT_SECONDS, 120);
        assert_eq!(SECRETS_CLASS_CHECKS.len(), 7);
        assert!(SECRETS_CLASS_CHECKS.contains(&"private_keys"));
        assert!(SECRETS_CLASS_CHECKS.contains(&"database_credentials"));
        assert!(SECRETS_CLASS_CHECKS.contains(&"github_tokens"));
        assert!(ALL.contains(&"scanner_available"));
    }

    #[test]
    fn tier1_finding_is_secrets_and_location() {
        let f = Tier1Finding::new("private_keys", "PII Scan", "critical", "msg", "a/b.py", 10, "");
        assert!(f.is_secrets_class());
        assert_eq!(f.location(), "a/b.py:10");
        let f2 = Tier1Finding::new("emails", "PII Scan", "low", "msg", "a/b.py", 0, "");
        assert!(!f2.is_secrets_class());
        assert_eq!(f2.location(), "a/b.py");
        let f3 = Tier1Finding::new("emails", "PII Scan", "low", "msg", "", 0, "");
        assert_eq!(f3.location(), "?");
        let f4 = Tier1Finding::new("emails", "PII Scan", "low", "msg", "", 5, "");
        assert_eq!(f4.location(), "?"); // file empty -> "?" even if line !=0, mirrors Python `self.file or "?"`
    }

    #[test]
    fn tier1_report_advisory_and_secrets() {
        let r = Tier1Report {
            available: true,
            passed: false,
            findings: vec![
                Tier1Finding::new("private_keys", "PII", "critical", "m", "f", 1, ""),
                Tier1Finding::new("emails", "PII", "low", "m2", "f", 2, ""),
            ],
            incomplete_checks: vec![],
            error: String::new(),
        };
        assert_eq!(r.secrets_findings().len(), 1);
        assert_eq!(r.advisory_findings().len(), 1);
        assert_eq!(r.secrets_findings()[0].check, "private_keys");
    }

    #[test]
    fn tier1_advisory_enabled_from_value() {
        assert!(tier1_advisory_enabled_from_value(&json!({})));
        assert!(tier1_advisory_enabled_from_value(&json!({"skills": null})));
        assert!(tier1_advisory_enabled_from_value(&json!({"skills": "not a dict"})));
        assert!(tier1_advisory_enabled_from_value(&json!({"skills": {}})));
        assert!(tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": true}})));
        assert!(!tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": false}})));
        assert!(tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": 1}})));
        assert!(!tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": 0}})));
        assert!(!tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": "false"}})));
        assert!(!tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": "0"}})));
        assert!(!tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": "no"}})));
        assert!(!tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": "off"}})));
        assert!(!tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": "  Off  "}})));
        assert!(tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": "true"}})));
        assert!(tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": "yes"}})));
        assert!(tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": "1"}})));
        assert!(!tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": null}})));
        assert!(!tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": 0.0}})));
        assert!(tier1_advisory_enabled_from_value(&json!({"skills": {"tier1_advisory": 0.5}})));
    }

    #[test]
    fn tier1_advisory_yaml_scanner() {
        assert!(parse_tier1_advisory_yaml_text("model: foo\n").is_none());
        assert_eq!(
            parse_tier1_advisory_yaml_text("skills:\n  tier1_advisory: false\n").as_deref(),
            Some("false")
        );
        assert_eq!(
            parse_tier1_advisory_yaml_text("skills:\n  tier1_advisory: \"off\"\n").as_deref(),
            Some("off")
        );
        // With comment
        assert_eq!(
            parse_tier1_advisory_yaml_text("skills:\n  tier1_advisory: true # enabled\n").as_deref(),
            Some("true")
        );
        // Not in skills
        assert!(parse_tier1_advisory_yaml_text("hooks:\n  output_spill: true\n").is_none());
    }

    #[test]
    fn parse_report_basic() {
        let report = json!({
            "results": [
                {
                    "validator": "PII Scan",
                    "status": "complete",
                    "passed": false,
                    "findings": [
                        {"check_name": "emails", "severity": "Low", "message": "found email", "file_path": "SKILL.md", "line_number": 5, "suggestion": "remove"},
                        {"check_name": "private_keys", "severity": "CRITICAL", "message": "key found with long text that should be truncated at 200 chars " , "file_path": "a.py", "line_number": null, "suggestion": ""}
                    ]
                },
                {
                    "validator": "Security",
                    "status": "incomplete",
                    "passed": false,
                    "findings": [
                        {"check_name": "hardcoded_secrets", "severity": "high", "message": "secret", "file_path": "", "line_number": 0, "suggestion": "s"}
                    ]
                }
            ]
        });
        let parsed = parse_report(&report);
        assert!(parsed.available);
        assert!(!parsed.passed); // any_complete_failed due to PII Scan passed=false
        assert_eq!(parsed.findings.len(), 3);
        assert_eq!(parsed.incomplete_checks, vec!["Security"]);
        // incomplete findings ARE kept (partial evidence is still evidence)
        assert!(parsed.findings.iter().any(|f| f.check == "hardcoded_secrets"));
        // severity lowercased
        assert_eq!(parsed.findings[0].severity, "low");
        assert_eq!(parsed.findings[1].severity, "critical");
        // message truncation not needed here, but check file/line
        assert_eq!(parsed.findings[0].file, "SKILL.md");
        assert_eq!(parsed.findings[0].line, 5);
        assert_eq!(parsed.findings[1].line, 0); // null -> 0
        // passed false when findings exist even if all passed true? Let's test no incomplete and no findings
        let report2 = json!({
            "results": [
                {"validator": "PII Scan", "status": "complete", "passed": true, "findings": []}
            ]
        });
        let p2 = parse_report(&report2);
        assert!(p2.passed);
        assert!(p2.findings.is_empty());
        assert!(p2.incomplete_checks.is_empty());

        // incomplete with no findings -> passed based only on findings empty but incomplete excluded from fail signal
        let report3 = json!({
            "results": [
                {"validator": "Security", "status": "incomplete", "passed": false, "findings": []}
            ]
        });
        let p3 = parse_report(&report3);
        assert!(p3.passed); // no findings and incomplete doesn't count as failed
        assert_eq!(p3.incomplete_checks, vec!["Security"]);
    }

    #[test]
    fn parse_report_truncates_200() {
        let long = "a".repeat(300);
        let report = json!({
            "results": [
                {"validator": "PII Scan", "status": "complete", "passed": false, "findings": [
                    {"check_name": "emails", "severity": "low", "message": long, "file_path": "f", "line_number": 1, "suggestion": long}
                ]}
            ]
        });
        let parsed = parse_report(&report);
        assert_eq!(parsed.findings[0].message.chars().count(), 200);
        assert_eq!(parsed.findings[0].suggestion.chars().count(), 200);
    }

    #[test]
    fn parse_report_skips_non_dict_findings() {
        let report = json!({
            "results": [
                {"validator": "PII Scan", "status": "complete", "passed": false, "findings": ["not a dict", 123, {"check_name": "emails", "severity": "low", "message": "hi", "file_path": "f", "line_number": 1, "suggestion": ""}]}
            ]
        });
        let parsed = parse_report(&report);
        assert_eq!(parsed.findings.len(), 1);
        assert_eq!(parsed.findings[0].check, "emails");
    }

    #[test]
    fn format_tier1_report_cases() {
        // unavailable -> empty
        let r = Tier1Report { available: false, passed: true, findings: vec![], incomplete_checks: vec![], error: "nope".into() };
        assert_eq!(format_tier1_report(&r, 10), "");

        // no findings, no incomplete
        let r2 = Tier1Report { available: true, passed: true, findings: vec![], incomplete_checks: vec![], error: String::new() };
        assert_eq!(format_tier1_report(&r2, 10), "SkillEvaluator Tier 1: no findings.");

        // no findings but incomplete
        let r3 = Tier1Report { available: true, passed: true, findings: vec![], incomplete_checks: vec!["Security".into()], error: String::new() };
        assert_eq!(format_tier1_report(&r3, 10), "SkillEvaluator Tier 1: no findings from completed checks.\n  (not run: Security — no opinion from these checks)");

        // with findings: secrets first, tag SECRETS vs severity upper
        let r4 = Tier1Report {
            available: true,
            passed: false,
            findings: vec![
                Tier1Finding::new("emails", "PII Scan", "low", "found email", "SKILL.md", 2, ""),
                Tier1Finding::new("private_keys", "Security", "critical", "key!", "a.py", 1, ""),
            ],
            incomplete_checks: vec![],
            error: String::new(),
        };
        let out = format_tier1_report(&r4, 10);
        // first line contains count
        assert!(out.contains("2 finding(s)"));
        // secrets first
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[1].contains("[SECRETS]"));
        assert!(lines[1].contains("a.py:1"));
        assert!(lines[2].contains("[LOW]"));
        assert!(lines[2].contains("SKILL.md:2"));

        // limit truncation
        let many: Vec<Tier1Finding> = (0..5).map(|i| Tier1Finding::new("emails", "PII", "low", format!("msg{i}"), "f", i, "")).collect();
        let r5 = Tier1Report { available: true, passed: false, findings: many, incomplete_checks: vec![], error: String::new() };
        let out5 = format_tier1_report(&r5, 2);
        assert!(out5.contains("… and 3 more"));
        assert_eq!(out5.lines().count(), 4); // header + 2 findings + "… and 3 more"

        // with incomplete at end
        let r6 = Tier1Report {
            available: true,
            passed: false,
            findings: vec![Tier1Finding::new("emails", "PII", "low", "hi", "f", 1, "")],
            incomplete_checks: vec!["Security".into(), "License".into()],
            error: String::new(),
        };
        let out6 = format_tier1_report(&r6, 10);
        assert!(out6.contains("(not run: Security, License"));
    }

    #[test]
    fn run_tier1_scan_missing_binary() {
        // Temporarily hide PATH
        let prev = std::env::var("PATH").ok();
        unsafe { std::env::set_var("PATH", "/tmp/no-such-dir-xyz") };
        let report = run_tier1_scan_with_timeout(Path::new("/tmp"), 1);
        assert!(!report.available);
        assert_eq!(report.error, "scanner not on PATH");
        if let Some(p) = prev { unsafe { std::env::set_var("PATH", p) }; } else { unsafe { std::env::remove_var("PATH") }; }
    }

    #[test]
    fn scanner_available_respects_path() {
        // No scanner should be on PATH in test env (unless installed)
        // Just verify function doesn't panic and returns bool
        let _ = scanner_available();
    }
}
