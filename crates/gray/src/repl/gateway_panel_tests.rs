use super::*;

fn row(name: &str, on: bool) -> crate::plugin_cli::ManagedRow {
    crate::plugin_cli::ManagedRow {
        name: name.to_string(),
        version: "0.2.0".to_string(),
        scope: "user".to_string(),
        ecosystem: "gray-native".to_string(),
        on,
        cli: true,
    }
}

#[test]
fn app_row_shows_plugin_shape_and_toggle_state() {
    let items = app_rows_with(&[row("discord", true)], None);
    assert_eq!(items.len(), 1);
    assert!(
        items[0].row.starts_with("\u{2713} discord 0.2.0 (user)"),
        "{}",
        items[0].row
    );
    assert!(items[0].enabled);
    assert!(items[0].lit);
    assert!(!items[0].read_only);

    let off = app_rows_with(&[row("discord", false)], None);
    assert!(off[0].row.starts_with("\u{25cb}"), "{}", off[0].row);
    assert!(off[0].row.contains("[disabled]"), "{}", off[0].row);
    assert!(!off[0].enabled);
}

#[test]
fn missing_config_marks_the_app_as_needing_setup() {
    let home = tempfile::tempdir().unwrap();
    let items = app_rows_with(&[row("discord", true)], Some(home.path()));
    assert!(items[0].row.contains("needs setup"), "{}", items[0].row);
    // The flag is what routes Enter to the setup flow.
    assert!(items[0].needs_setup);

    // The required keys present means configured; contents are never read.
    let cfg = home.path().join(".config/gray-discord");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.json"),
        r#"{"token": "sk-x", "channel_id": "1", "owner_id": "2"}"#,
    )
    .unwrap();
    let items = app_rows_with(&[row("discord", true)], Some(home.path()));
    // Configured but no live daemon reads as "stopped", which is startable:
    // Enter routes to the setup flow to bring it back, so the flag is set.
    assert!(!items[0].row.contains("needs setup"), "{}", items[0].row);
    assert!(items[0].row.contains("stopped"), "{}", items[0].row);
    assert!(items[0].needs_setup);

    // A config missing one required key is still a setup candidate, and the
    // row says nothing about which key or any value.
    std::fs::write(cfg.join("config.json"), r#"{"token": "sk-x"}"#).unwrap();
    let items = app_rows_with(&[row("discord", true)], Some(home.path()));
    assert!(items[0].row.contains("needs setup"), "{}", items[0].row);
    assert!(!items[0].row.contains("channel_id"), "{}", items[0].row);
    assert!(!items[0].row.contains("sk-x"), "{}", items[0].row);
}

#[test]
fn setup_probe_reads_the_user_home_not_the_gray_home() {
    // App configs live under the *user's* home (`~/.config/<app>`, the
    // plugin's own default path); gray's home holds only gray's registries.
    // Probing the gray home would report "needs setup" forever even after
    // a successful `gray gateway setup discord`.
    let user = crate::setup::user_home().unwrap();
    // Windows has no HOME; the native profile (USERPROFILE) is what
    // `user_home()` resolves there. Prefer it first so a Git Bash HOME
    // never shadows the native profile in the comparison.
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .unwrap();
    assert_eq!(user, std::path::PathBuf::from(home));
    assert_ne!(user, crate::plugin_cli::home().unwrap());
}

#[test]
fn unknown_apps_get_no_setup_probe_and_no_invented_commands() {
    let home = tempfile::tempdir().unwrap();
    let items = app_rows_with(&[row("slack", true)], Some(home.path()));
    assert_eq!(items[0].row, "\u{2713} slack 0.2.0 (user) [Gray Index]");
}

#[test]
fn the_panel_is_apps_only() {
    // The gateway picker lists the apps and nothing else: daemon/cron/memory
    // own their commands (`gray gateway status`, `/cron`, `/memory`) and this
    // panel narrates none of them — no rule, no pointer lines.
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".config/gray-discord")).unwrap();
    std::fs::write(home.path().join(".config/gray-discord/config.json"), "{}").unwrap();
    let rows = [row("discord", true)];
    let items = app_rows_with(&rows, Some(home.path()));
    assert_eq!(items.len(), 1);
    assert!(items[0].row.contains("discord"));

    // With no apps there are no rows either; the hint is the spec's only
    // fallback, and nothing decorative rides alongside it.
    assert!(app_rows_with(&[], Some(home.path())).is_empty());
    assert!(GATEWAY_SPEC.empty_hint.contains("no apps installed"));
}

#[test]
fn spec_is_a_toggle_listing_without_removal_or_errors() {
    assert_eq!(GATEWAY_SPEC.title, "Connections");
    const {
        assert!(GATEWAY_SPEC.supports_toggle);
    }
    // Removal belongs to /plugin; package errors belong to /plugin too.
    const {
        assert!(!GATEWAY_SPEC.supports_remove);
    }
    const {
        assert!(!GATEWAY_SPEC.errors_tab);
    }
}

/// The row reads three files per app: the app's config (existence),
/// the daemon pidfile (liveness), the identity the daemon writes on connect.
/// All three live under a temp home here.
fn write_app_files(home: &std::path::Path, config: bool, daemon: Option<u64>, bot: Option<&str>) {
    let dir = home.join(".config/gray-discord");
    std::fs::create_dir_all(&dir).unwrap();
    if config {
        // Required keys only — grey never reads the value back.
        std::fs::write(
            dir.join("config.json"),
            b"{\"token\":\"T\",\"channel_id\":\"1\"}",
        )
        .unwrap();
    }
    if let Some(pid) = daemon {
        std::fs::write(
            dir.join("daemon.json"),
            format!("{{\"pid\":{pid},\"started_at\":\"0\"}}"),
        )
        .unwrap();
    }
    if let Some(bot) = bot {
        std::fs::write(dir.join("state.json"), format!("{{\"bot\":\"{bot}\"}}")).unwrap();
    }
}

#[test]
fn a_running_daemon_with_its_identity_shows_as_connected() {
    let home = tempfile::tempdir().unwrap();
    write_app_files(
        home.path(),
        true,
        Some(std::process::id().into()),
        Some("graytest#0148"),
    );
    let items = app_rows_with(&[row("discord", true)], Some(home.path()));
    assert!(
        items[0].row.contains("connected as graytest#0148"),
        "{}",
        items[0].row
    );
    // A running, configured app is not a setup candidate.
    assert!(!items[0].needs_setup);
}

#[test]
fn a_running_daemon_without_an_identity_shows_as_connected() {
    let home = tempfile::tempdir().unwrap();
    write_app_files(home.path(), true, Some(std::process::id().into()), None);
    let items = app_rows_with(&[row("discord", true)], Some(home.path()));
    assert!(items[0].row.contains("connected"), "{}", items[0].row);
    assert!(!items[0].row.contains("connected as"), "{}", items[0].row);
}

#[test]
fn a_dead_pid_reports_stopped_even_with_a_stale_identity_file() {
    let home = tempfile::tempdir().unwrap();
    // PID 999_999_999 cannot exist; zombies stay out of the way too.
    write_app_files(home.path(), true, Some(999_999_999), Some("graytest#0148"));
    let items = app_rows_with(&[row("discord", true)], Some(home.path()));
    assert!(items[0].row.contains("stopped"), "{}", items[0].row);
    assert!(!items[0].row.contains("connected"), "{}", items[0].row);
}

#[test]
fn a_configured_but_unstarted_app_reports_stopped() {
    let home = tempfile::tempdir().unwrap();
    write_app_files(home.path(), true, None, None);
    let items = app_rows_with(&[row("discord", true)], Some(home.path()));
    assert!(items[0].row.contains("stopped"), "{}", items[0].row);
    // Stopped is startable: Enter routes to the setup/start flow (not the
    // bare enable/disable toggle), so the row carries the flag.
    assert!(items[0].needs_setup);
}
