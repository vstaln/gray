//! gray.alignment.id account: `gray login`, `gray whoami`, `gray logout`.
//!
//! The site mints a one-time enrollment code from a Supabase session
//! (GitHub / Google / Discord). `gray login <code>` exchanges that code here
//! for a long-lived `gray_…` registry token, stored at
//! `~/.gray/registry-token.json` (mode 0600, same atomic write as
//! `auth.json`).
//!
//! Nothing in gray needs an account: a login only exists so an authenticated
//! registry call can name a caller. Every command here works with no provider
//! configured, so a fresh machine can enroll before it can run a turn.

use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// Production registry API. Override with [`REGISTRY_URL_ENV`] to point at a
/// local `pnpm backend:dev` instance.
pub const DEFAULT_REGISTRY_URL: &str = "https://gray.alignment.id/api";
/// Environment override for the registry base URL.
pub const REGISTRY_URL_ENV: &str = "GRAY_REGISTRY_URL";
/// Where a user signs in and mints an enrollment code.
pub const SITE_ACCOUNT_URL: &str = "https://gray.alignment.id/account";
/// How long the site says a code lives, for the prompt copy.
pub const CODE_TTL_MINUTES: u32 = 5;

/// Registry base URL: the env override when set, else production. Any trailing
/// slash is stripped so endpoint paths never double up.
pub fn base_url() -> String {
    normalize_base_url(&std::env::var(REGISTRY_URL_ENV).unwrap_or_default())
}

/// Absolute endpoint URL built from [`base_url`]. Fails with a message that
/// names the env var, so a typo in `GRAY_REGISTRY_URL` reads as a config
/// problem instead of "relative URL without a base".
pub fn endpoint(path: &str) -> anyhow::Result<String> {
    endpoint_with(&base_url(), path)
}

/// Pure seam for [`endpoint`]: joins `path` onto an already-normalized base.
pub fn endpoint_with(base: &str, path: &str) -> anyhow::Result<String> {
    // `Url::join` replaces the last segment unless the base ends in '/', and
    // `normalize_base_url` strips that slash — put it back for the join only.
    let base = reqwest::Url::parse(&format!("{base}/"))
        .map_err(|e| anyhow::anyhow!("{REGISTRY_URL_ENV} is not a valid absolute URL: {e}"))?;
    Ok(base.join(path.trim_start_matches('/'))?.to_string())
}

/// Pure seam for [`base_url`]: empty/blank input falls back to production.
pub fn normalize_base_url(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        DEFAULT_REGISTRY_URL.to_string()
    } else {
        raw.trim_end_matches('/').to_string()
    }
}

/// The stored credential. Debug is hand-redacted: it carries a bearer token,
/// so the derived impl would leak it into any log that formats this.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredToken {
    pub token: String,
}

impl std::fmt::Debug for StoredToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredToken").finish_non_exhaustive()
    }
}

/// Path of the registry token file inside `$GRAY_HOME`.
pub fn token_path() -> anyhow::Result<PathBuf> {
    Ok(crate::setup::gray_home()?.join("registry-token.json"))
}

pub fn load_token() -> Option<String> {
    token_path().ok().as_deref().and_then(load_token_at)
}

/// Explicit-path seam for [`load_token`] (tests). A missing, unreadable, or
/// shape-wrong file reads as "not logged in" — never an error.
pub fn load_token_at(path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<StoredToken>(&body)
        .ok()
        .map(|t| t.token)
        .filter(|t| !t.trim().is_empty())
}

pub fn save_token(token: &str) -> anyhow::Result<()> {
    save_token_at(&token_path()?, token)
}

/// Writes the token with the shared 0600 atomic writer so the file can never
/// be observed half-written or world-readable.
pub fn save_token_at(path: &Path, token: &str) -> anyhow::Result<()> {
    let stored = StoredToken {
        token: token.trim().to_string(),
    };
    crate::setup::catalog::save_private_json(
        path,
        &serde_json::to_value(&stored).map_err(|e| anyhow::anyhow!("{e}"))?,
    )
}

/// Drops the local token. Returns true when a file was actually removed.
pub fn clear_token_at(path: &Path) -> anyhow::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// An enrollment code as pasted by the user. `None` when there is nothing to
/// exchange, so callers can re-prompt instead of firing an empty request.
pub fn normalize_code(raw: &str) -> Option<String> {
    let code = raw.trim();
    (!code.is_empty()).then(|| code.to_string())
}

// ---- Wire types (mirror services/registry/routes/auth.mjs) ----------------

/// One plugin the caller owns, newest version resolved server-side.
#[derive(Debug, Clone, Deserialize)]
pub struct OwnedPlugin {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

/// The identity behind a token. Every field is optional because the two
/// endpoints that produce it disagree: `/auth/token` nests a `publicUser`
/// subset, `/auth/me` adds `id`, `created_at`, and owned plugins.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub plugins: Vec<OwnedPlugin>,
}

impl Account {
    /// Best human label: display name, then @handle, then provider, then
    /// "your account". Never invents a name the registry did not send.
    pub fn label(&self) -> String {
        self.display_name
            .clone()
            .or_else(|| self.username.clone().map(|u| format!("@{u}")))
            .or_else(|| self.provider.clone())
            .unwrap_or_else(|| "your account".to_string())
    }
}

#[derive(Deserialize)]
struct TokenExchangeResponse {
    token: String,
    #[serde(default)]
    user: Option<Account>,
}

// ---- HTTP -----------------------------------------------------------------

fn http_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?)
}

/// The registry reports failures as `{"error": "..."}`; surface that message
/// verbatim instead of a bare status code.
async fn error_message(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body)
        && let Some(msg) = parsed.get("error").and_then(|v| v.as_str())
        && !msg.trim().is_empty()
    {
        return msg.to_string();
    }
    format!("registry returned {status}")
}

/// Exchanges a one-time enrollment code for a long-lived `gray_…` token.
pub async fn login_with_code(code: &str) -> anyhow::Result<(String, Account)> {
    let url = endpoint("auth/token")?;
    let resp = http_client()?
        .post(&url)
        .json(&serde_json::json!({ "code": code }))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("could not reach {url}: {e}"))?;
    if !resp.status().is_success() {
        anyhow::bail!(error_message(resp).await);
    }
    let parsed: TokenExchangeResponse = resp.json().await?;
    Ok((parsed.token, parsed.user.unwrap_or_default()))
}

/// Who the stored token belongs to, plus the plugins it owns.
pub async fn whoami() -> anyhow::Result<Account> {
    let token = load_token().ok_or_else(|| {
        anyhow::anyhow!("not logged in — run `gray login` (or /login in the REPL)")
    })?;
    let url = endpoint("auth/me")?;
    let resp = http_client()?
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("could not reach {url}: {e}"))?;
    if !resp.status().is_success() {
        anyhow::bail!(error_message(resp).await);
    }
    Ok(resp.json().await?)
}

/// Best-effort server-side revoke of one token. Failures are logged and
/// swallowed: the caller's next step (saving a new token, clearing the local
/// file) must not depend on the registry being reachable.
async fn revoke_token(token: &str) -> bool {
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => {
            log::debug!(target: "gray_account", "revoke client build failed: {e}");
            return false;
        }
    };
    let url = match endpoint("auth/tokens") {
        Ok(u) => u,
        Err(e) => {
            log::debug!(target: "gray_account", "revoke endpoint failed: {e}");
            return false;
        }
    };
    let sent = client
        .delete(&url)
        .bearer_auth(token)
        .json(&serde_json::json!({ "token": token }))
        .send()
        .await;
    match sent {
        Ok(r) if r.status().is_success() => true,
        Ok(r) => {
            log::debug!(
                target: "gray_account",
                "revoke failed: {}",
                error_message(r).await
            );
            false
        }
        Err(e) => {
            log::debug!(target: "gray_account", "revoke request failed: {e}");
            false
        }
    }
}

/// Revokes the stored token server-side, then drops it locally. A token the
/// registry already forgot is still cleared locally: the goal is "this machine
/// holds no credential", not "the server agreed".
pub async fn logout() -> anyhow::Result<bool> {
    let Some(token) = load_token() else {
        return Ok(false);
    };
    let revoked = revoke_token(&token).await;
    clear_token_at(&token_path()?)?;
    Ok(revoked)
}

// ---- Prompts --------------------------------------------------------------

/// Reads one line from stdin, trimmed. `None` on EOF (piped empty stdin) so a
/// scripted `gray login` fails loudly instead of hanging.
pub fn prompt_for_code() -> Option<String> {
    use std::io::Write as _;
    print!("code: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => normalize_code(&line),
    }
}

/// The instructions printed before the prompt. Kept as data so the REPL and
/// the CLI print the same words.
pub fn login_instructions() -> String {
    format!(
        "1. Open {SITE_ACCOUNT_URL} and sign in (GitHub, Google, or Discord).\n\
         2. Choose \"Generate CLI login code\" — it expires in {CODE_TTL_MINUTES} minutes.\n\
         3. Paste it here."
    )
}

// ---- Command entry points -------------------------------------------------

/// `gray login [CODE]`: prints the walkthrough when no code is given,
/// exchanges the code, stores the token, and reports the identity.
pub async fn run_login(code: Option<&str>) -> anyhow::Result<()> {
    let code = match code.map(|c| c.trim()).filter(|c| !c.is_empty()) {
        Some(c) => c.to_string(),
        None => {
            println!("Log in to gray.alignment.id from this machine.\n");
            println!("{}\n", login_instructions());
            prompt_for_code().ok_or_else(|| {
                anyhow::anyhow!("no enrollment code given — run `gray login <code>`")
            })?
        }
    };
    let (token, account) = login_with_code(&code).await?;
    // A second login would otherwise orphan the previous token: it stays
    // valid server-side while this machine forgets it ever existed. Exchange
    // first, so a bad code never costs the user their current session.
    if let Some(previous) = load_token()
        && previous != token
    {
        revoke_token(&previous).await;
    }
    save_token(&token)?;
    println!("logged in as {}", account.label());
    if account.plugins.is_empty() {
        println!("no plugins published yet");
    } else {
        println!("plugins:");
        for p in &account.plugins {
            match &p.version {
                Some(v) => println!("  {} {v}", p.name),
                None => println!("  {}", p.name),
            }
        }
    }
    println!(
        "\nWhat this unlocks today: nothing you didn't already have. The token only names you on registry calls."
    );
    Ok(())
}

/// `gray whoami`: reports the stored token's identity.
pub async fn run_whoami() -> anyhow::Result<()> {
    let account = whoami().await?;
    println!("{}", account.label());
    if let Some(provider) = &account.provider {
        println!("signed in with {provider}");
    }
    if !account.plugins.is_empty() {
        println!("plugins:");
        for p in &account.plugins {
            match &p.version {
                Some(v) => println!("  {} {v}", p.name),
                None => println!("  {}", p.name),
            }
        }
    }
    Ok(())
}

/// `gray logout`: revokes and forgets the token.
pub async fn run_logout() -> anyhow::Result<()> {
    if logout().await? {
        println!("logged out — registry token revoked");
    } else {
        println!("logged out — no registry token was stored");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, body).expect("write fixture");
    }

    #[test]
    fn base_url_falls_back_to_production_when_env_is_blank() {
        assert_eq!(normalize_base_url(""), DEFAULT_REGISTRY_URL);
        assert_eq!(normalize_base_url("   "), DEFAULT_REGISTRY_URL);
    }

    #[test]
    fn base_url_strips_one_trailing_slash_only() {
        assert_eq!(
            normalize_base_url("http://127.0.0.1:4000/api/"),
            "http://127.0.0.1:4000/api"
        );
        // Every trailing slash goes: an endpoint join must never see "//".
        assert_eq!(
            normalize_base_url("https://example.test/api//"),
            "https://example.test/api"
        );
    }

    /// Mutates process env: hold `ACCOUNT_ENV_SERIAL` for the whole test.
    struct EnvGuard {
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(value: Option<&str>) -> Self {
            let prev = std::env::var(REGISTRY_URL_ENV).ok();
            match value {
                Some(v) => unsafe { std::env::set_var(REGISTRY_URL_ENV, v) },
                None => unsafe { std::env::remove_var(REGISTRY_URL_ENV) },
            }
            Self { prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(REGISTRY_URL_ENV, v) },
                None => unsafe { std::env::remove_var(REGISTRY_URL_ENV) },
            }
        }
    }

    #[test]
    fn endpoint_joins_onto_the_api_prefix() {
        assert_eq!(
            endpoint_with("http://127.0.0.1:4000/api", "auth/token").expect("endpoint"),
            "http://127.0.0.1:4000/api/auth/token"
        );
        // A leading slash must not collapse the /api prefix.
        assert_eq!(
            endpoint_with("http://127.0.0.1:4000/api", "/auth/token").expect("endpoint"),
            "http://127.0.0.1:4000/api/auth/token"
        );
    }

    #[test]
    fn endpoint_names_the_env_var_when_the_url_is_junk() {
        let err = endpoint_with("not a url", "auth/token").expect_err("junk url must fail");
        assert!(err.to_string().contains(REGISTRY_URL_ENV), "{err}");
    }

    /// Process env is global: these two readers must not interleave.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn base_url_reads_the_env_override() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::set(Some("http://127.0.0.1:4000/api/"));
        assert_eq!(base_url(), "http://127.0.0.1:4000/api");
    }

    #[test]
    fn base_url_ignores_an_empty_env_override() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::set(Some("  "));
        assert_eq!(base_url(), DEFAULT_REGISTRY_URL);
    }

    #[test]
    fn empty_code_normalizes_to_none() {
        assert_eq!(normalize_code(""), None);
        assert_eq!(normalize_code("   \n"), None);
        assert_eq!(normalize_code("  abc123 \n"), Some("abc123".to_string()));
    }

    #[test]
    fn token_round_trips_through_the_private_writer() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("registry-token.json");
        save_token_at(&path, "gray_secret").expect("save");
        assert_eq!(load_token_at(&path), Some("gray_secret".to_string()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "token file must be 0600");
        }
    }

    #[test]
    fn missing_or_junk_token_file_reads_as_logged_out() {
        let dir = tempfile::tempdir().expect("tmp");
        let missing = dir.path().join("nope.json");
        assert_eq!(load_token_at(&missing), None);
        let junk = dir.path().join("junk.json");
        write(&junk, "not json at all");
        assert_eq!(load_token_at(&junk), None);
        let empty = dir.path().join("empty.json");
        write(&empty, r#"{"token": "  "}"#);
        assert_eq!(load_token_at(&empty), None);
    }

    #[test]
    fn clear_token_reports_whether_a_file_was_there() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("registry-token.json");
        assert!(!clear_token_at(&path).expect("clear missing"));
        save_token_at(&path, "gray_secret").expect("save");
        assert!(clear_token_at(&path).expect("clear present"));
        assert!(!clear_token_at(&path).expect("clear again"));
    }

    #[test]
    fn account_label_prefers_the_display_name() {
        let account = Account {
            display_name: Some("Vstalin Grady".to_string()),
            username: Some("vstaln".to_string()),
            ..Default::default()
        };
        assert_eq!(account.label(), "Vstalin Grady");
        let handle_only = Account {
            username: Some("vstaln".to_string()),
            ..Default::default()
        };
        assert_eq!(handle_only.label(), "@vstaln");
        assert_eq!(Account::default().label(), "your account");
    }
}
