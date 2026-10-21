use super::dispatch::{Flow, dispatch_command};
use super::{PluginAction, is_bare_plugin_cmd, parse_plugin_args};

#[test]
fn activation_requires_new_installs_and_now() {
    use super::ActivationChoice::{Later, Now};
    assert!(!super::should_reload(&[], Now, true));
    assert!(!super::should_reload(&["demo".into()], Later, true));
    assert!(!super::should_reload(&["demo".into()], Now, false));
    assert!(super::should_reload(
        &["demo".into(), "other".into()],
        Now,
        true
    ));
    assert!(!super::should_prompt(&[], true));
    assert!(!super::should_prompt(&["demo".into()], false));
    assert!(super::should_prompt(&["demo".into()], true));
}

#[tokio::test]
async fn bare_aliases_parse_and_dispatch_to_marketplace_picker() {
    use clap::Parser;
    for raw in ["/plugin", "/plugins", "/PLUGIN", "/PLUGINS"] {
        let cli = crate::Cli::try_parse_from(["gray"]).unwrap();
        let mut config = crate::config::Config::resolve_with(&cli, |_| None).unwrap();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(crate::composer::Tui::test_tui(
            80, 24,
        )));
        let tui = Some((
            shared,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ));
        super::TEST_PLUGIN_MODAL.with_borrow_mut(|slot| {
            *slot = Some(crate::setup::PluginModalOutcome::default());
        });
        let mut agent = None;
        let mut session = None;
        let flow = dispatch_command(
            super::parse_command(raw),
            &mut agent,
            &mut config,
            tempfile::tempdir().unwrap().path(),
            &tui,
            &mut session,
            &mut super::SessionTotals::default(),
            &mut None,
            &mut std::sync::Arc::new(Vec::new()),
            &mut false,
            &mut false,
        )
        .await
        .unwrap();
        assert_eq!(flow, Flow::Continue);
        assert!(
            super::TEST_PLUGIN_MODAL.with_borrow(|slot| slot.is_none()),
            "{raw} did not open the marketplace picker"
        );
        assert!(agent.is_none(), "opening marketplace must stay lazy");
        assert!(session.is_none());
    }
}

#[test]
fn bare_detection_covers_case_and_trailing_space() {
    assert!(is_bare_plugin_cmd("/plugin"));
    assert!(is_bare_plugin_cmd("/PLUGINS"));
    assert!(is_bare_plugin_cmd("/plugin  "));
    assert!(!is_bare_plugin_cmd("/plugin list"));
    assert!(!is_bare_plugin_cmd("/plugin list foo"));
    assert!(!is_bare_plugin_cmd("/plugin install foo"));
    assert!(!is_bare_plugin_cmd("/plugin enable foo"));
}

#[test]
fn parses_install_url() {
    assert!(matches!(
        parse_plugin_args("/plugin install https://h/x.tar.gz"),
        Ok(PluginAction::Install(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugins"),
        Ok(PluginAction::List)
    ));
}

#[test]
fn parse_covers_every_subcommand() {
    assert!(matches!(
        parse_plugin_args("/plugin"),
        Ok(PluginAction::List)
    ));
    assert!(matches!(
        parse_plugin_args("/plugin LIST"),
        Ok(PluginAction::List)
    ));
    assert!(parse_plugin_args("/plugin list foo").is_err());
    assert!(parse_plugin_args("/plugin list foo bar").is_err());
    assert!(matches!(
        parse_plugin_args("/plugin install foo"),
        Ok(PluginAction::Install(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugin remove foo"),
        Ok(PluginAction::Remove(_))
    ));

    assert!(matches!(
        parse_plugin_args("/plugin update"),
        Ok(PluginAction::Update(t)) if t == "all"
    ));
    assert!(matches!(
        parse_plugin_args("/plugin update foo"),
        Ok(PluginAction::Update(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugin enable foo"),
        Ok(PluginAction::Enable(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugin disable foo"),
        Ok(PluginAction::Disable(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugin check ./x"),
        Ok(PluginAction::Check(_))
    ));
    assert!(parse_plugin_args("/plugin install").is_err());
    assert!(parse_plugin_args("/plugin search foo").is_err());
    assert!(parse_plugin_args("/plugin frobnicate x").is_err());
}

#[tokio::test]
async fn plugin_list_dispatches_without_unknown_command() {
    use clap::Parser;
    let cli = crate::Cli::try_parse_from(["gray"]).expect("cli parses");
    let mut config = crate::config::Config::resolve_with(&cli, |_| None).expect("config resolves");
    let cwd = tempfile::tempdir().expect("tempdir").keep();
    let tui: super::TuiOpt = None;
    let mut agent: Option<super::Agent> = None;
    let mut session_state: Option<super::SessionState> = None;
    let mut totals = super::SessionTotals::default();
    let mut pending: Option<super::ReplCommand> = None;
    let mut history = std::sync::Arc::new(Vec::new());
    let mut unconfigured = false;
    let mut hide_thinking = false;
    let flow = dispatch_command(
        super::ReplCommand::Plugin("/plugin list".to_string()),
        &mut agent,
        &mut config,
        &cwd,
        &tui,
        &mut session_state,
        &mut totals,
        &mut pending,
        &mut history,
        &mut unconfigured,
        &mut hide_thinking,
    )
    .await
    .expect("dispatch ok");
    assert_eq!(flow, Flow::Continue);
    assert!(pending.is_none());
}

#[tokio::test]
async fn background_off_dispatches_without_provider_or_prompt() {
    use gray_core::agent::{Agent, CommandOutcome, PluginCommand, PluginHooks};
    use std::sync::{Arc, Mutex};
    struct NoRequests;
    impl gray_core::agent::Provider for NoRequests {
        fn stream(&self, _: gray_core::message::ChatRequest) -> gray_core::agent::ProviderStream {
            panic!("plugin commands must not call the provider")
        }
    }
    #[derive(Default)]
    struct BackgroundHook(Mutex<Vec<(String, Vec<String>)>>);
    #[async_trait::async_trait]
    impl PluginHooks for BackgroundHook {
        fn commands(&self) -> Vec<PluginCommand> {
            vec![PluginCommand {
                name: "/background".into(),
                description: "background".into(),
                turn_local_verbs: Vec::new(),
            }]
        }
        async fn run_command(&self, name: &str, argv: Vec<String>) -> Option<CommandOutcome> {
            self.0.lock().unwrap().push((name.into(), argv));
            Some(CommandOutcome::Say("Background off.".into()))
        }
    }
    use clap::Parser;
    let cli = crate::Cli::try_parse_from(["gray"]).unwrap();
    let mut config = crate::config::Config::resolve_with(&cli, |_| None).unwrap();
    let hook = Arc::new(BackgroundHook::default());
    let mut agent = Some(
        Agent::new(
            Box::new(NoRequests),
            Arc::new(gray_tools::Registry::default()),
        )
        .with_hooks(vec![hook.clone()]),
    );
    let cwd = tempfile::tempdir().unwrap();
    let mut session = None;
    let mut totals = super::SessionTotals::default();
    let mut pending = None;
    let mut history = std::sync::Arc::new(Vec::new());
    let flow = dispatch_command(
        super::parse_command("/background off"),
        &mut agent,
        &mut config,
        cwd.path(),
        &None,
        &mut session,
        &mut totals,
        &mut pending,
        &mut history,
        &mut false,
        &mut false,
    )
    .await
    .unwrap();
    assert_eq!(flow, Flow::Continue);
    assert_eq!(
        *hook.0.lock().unwrap(),
        vec![("/background".into(), vec!["off".into()])]
    );
    assert!(pending.is_none());
    assert!(agent.unwrap().messages().is_empty());
}
