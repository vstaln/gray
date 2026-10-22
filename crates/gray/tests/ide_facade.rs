use std::sync::{Arc, Mutex};

use clap::Parser;
use gray::ide::{IdeSession, SubmitOutcome, UiCommand};

fn harness() -> (
    tempfile::TempDir,
    Arc<Mutex<Vec<String>>>,
    tokio::runtime::Runtime,
) {
    let home = tempfile::tempdir().expect("temp home");
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
        std::env::set_var("GRAY_USER_HOME", home.path());
        std::env::set_var("GRAY_TURN_RETRY_DELAY_MS", "0");
        std::env::remove_var("GRAY_MODEL");
        std::env::remove_var("GRAY_API_KEY");
        std::env::remove_var("GRAY_BASE_URL");
        std::env::remove_var("GRAY_PROVIDER");
    }
    let out = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = out.clone();
    gray::ide::set_output_sink(Some(Box::new(move |line: &str| {
        sink.lock().expect("out").push(line.to_string());
    })));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    (home, out, rt)
}

fn session() -> IdeSession {
    let cli = gray::Cli::parse_from(["gray-ide"]);
    let config = gray::Config::resolve(&cli).expect("config resolve");
    IdeSession::new(config, std::env::current_dir().expect("cwd"))
}

#[test]
fn ide_facade_e2e() {
    let (home, out, rt) = harness();
    rt.block_on(async move {
        let mut s = session();

        let outcome = s.submit("/help").await.expect("help");
        assert_eq!(outcome, SubmitOutcome::Done);
        let text = out.lock().expect("out").join("\n");
        assert!(
            text.contains("/model") || text.contains("/new") || text.contains("commands"),
            "expected help-style output, got: {text}"
        );

        for (line, want) in [
            ("/models", UiCommand::ModelPicker),
            ("/resume", UiCommand::ResumePicker),
            ("/restart", UiCommand::Restart),
            (
                "/sys",
                UiCommand::SysEdit {
                    path: Some(home.path().join("AGENTS.md")),
                },
            ),
        ] {
            let outcome = s.submit(line).await.expect(line);
            match outcome {
                SubmitOutcome::NeedsUi(ui) => {
                    assert_eq!(ui, want, "wrong UI routing for {line}")
                }
                other => panic!("{line}: expected NeedsUi({want:?}), got {other:?}"),
            }
        }

        assert!(
            !s.poll_idle().await.expect("poll_idle"),
            "no session yet — idle poll must not run a turn"
        );

        let outcome = s.submit("hello").await.expect("prompt");
        assert_eq!(outcome, SubmitOutcome::NeedsUi(UiCommand::SetupWizard));
        assert!(!s.configured());

        for line in ["/keys", "/setup", "/usage", "/context", "/skills", "/theme"] {
            out.lock().expect("out").clear();
            let outcome = s.submit(line).await.expect(line);
            assert_eq!(outcome, SubmitOutcome::Done, "{line} should dispatch");
        }

        let outcome = s.submit("/new").await.expect("new");
        assert_eq!(outcome, SubmitOutcome::Done);
        assert!(s.history().is_empty(), "new should leave empty history");

        out.lock().expect("out").clear();
        let outcome = s.submit("/bogus-cmd-xyz").await.expect("bogus");
        assert_eq!(outcome, SubmitOutcome::Done);
        let text = out.lock().expect("out").join("\n");
        assert!(
            text.contains("unknown command"),
            "expected unknown-command message, got: {text}"
        );

        std::fs::write(
            home.path().join("config.json"),
            r#"{"model":"test/model","provider_id":"fake","credential_source":"plugin","auth_ref":"fake"}"#,
        )
        .expect("write saved config");
        let outcome = s.submit("hello again").await.expect("prompt after setup");
        assert!(s.configured(), "saved config should clear SetupWizard");
        assert!(
            !matches!(outcome, SubmitOutcome::NeedsUi(UiCommand::SetupWizard)),
            "prompt must not dead-end on SetupWizard after setup, got {outcome:?}"
        );

        let outcome = s.submit("/quit").await.expect("quit");
        assert_eq!(outcome, SubmitOutcome::Quit);
    });
    gray::ide::set_output_sink(None);
}

#[test]
fn ide_parse_matches_repl() {
    use gray::repl::commands::ReplCommand;

    assert_eq!(
        IdeSession::parse("/new"),
        ReplCommand::New(None),
        "same parse as the repl"
    );
    assert!(matches!(
        IdeSession::parse("/models"),
        ReplCommand::ModelPanes
    ));
    assert!(matches!(
        IdeSession::parse("/models p/m"),
        ReplCommand::Model(Some(_))
    ));
    assert!(matches!(
        IdeSession::parse("plain prompt"),
        ReplCommand::Prompt(_)
    ));
    assert!(IdeSession::is_interactive(&ReplCommand::ModelPanes));
    assert!(!IdeSession::is_interactive(&ReplCommand::Model(Some(
        "p/m".into()
    ))));

    for local in ["/jobs", "/help", "/plugins"] {
        assert!(
            gray::ide::is_turn_local_command(local),
            "{local} runs locally mid-turn, never steered to the model"
        );
    }
    for steered in ["hello", "/new", "/models", "/resume"] {
        assert!(
            !gray::ide::is_turn_local_command(steered),
            "{steered} must steer/queue, not run as a turn-local command"
        );
    }
}
