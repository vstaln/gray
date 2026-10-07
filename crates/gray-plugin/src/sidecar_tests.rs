use super::*;
use gray_core::agent::ToolContext;
use gray_core::event::Usage;

#[tokio::test]
async fn hanging_hook_times_out_and_skips() {
    let p = SidecarPlugin::spawn(vec!["testdata/hang_plugin.sh".into()])
        .await
        .unwrap();
    let t = std::time::Instant::now();
    p.on_event(CoreEvent::TurnEnd {
        usage: Usage::default(),
    })
    .await;
    assert!(t.elapsed() < std::time::Duration::from_secs(10));
}

#[tokio::test]
async fn crashed_plugin_returns_error_not_panic() {
    let p = SidecarPlugin::spawn(vec!["testdata/crash_plugin.sh".into()])
        .await
        .unwrap();
    let out = p.tools()[0]
        .execute(&ToolContext::default(), serde_json::json!({}))
        .await;
    assert!(out.is_error);
    assert!(
        out.content.contains("plugin crashed: crash"),
        "got: {}",
        out.content
    );
}

#[tokio::test]
async fn empty_manifest_name_bails() {
    let err = SidecarPlugin::spawn(vec!["testdata/empty_name_plugin.sh".into()])
        .await
        .err()
        .expect("spawn must bail on missing/empty name");
    assert!(err.to_string().contains("empty name"), "got: {err:#}");
}

#[tokio::test]
async fn notify_sends_no_id_and_needs_no_reply() {
    // hang fixture never replies to event/notify; if on_event waited for a
    // reply it would hit the 5s timeout. True notification returns fast.
    let p = SidecarPlugin::spawn(vec!["testdata/hang_plugin.sh".into()])
        .await
        .unwrap();
    let t = std::time::Instant::now();
    p.on_event(CoreEvent::TurnEnd {
        usage: Usage::default(),
    })
    .await;
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
}

#[tokio::test]
async fn a_child_that_stopped_reading_fails_the_request_not_the_protocol() {
    // The wedged stub answers the handshake, then never reads stdin again.
    // The request frame is far larger than the pipe buffer, so its write
    // cannot complete: the call must fail within WRITE_TIMEOUT instead of
    // tearing the frame and desyncing every later request.
    let p = SidecarPlugin::spawn(vec!["testdata/wedged_stdin_plugin.sh".into()])
        .await
        .unwrap();
    let blob = "x".repeat(200 * 1024);
    let t = std::time::Instant::now();
    let err = p
        .transport
        .request(
            "echo",
            Some(serde_json::json!({"blob": blob})),
            std::time::Duration::from_secs(30),
        )
        .await
        .err()
        .expect("a wedged child must fail the request");
    assert!(t.elapsed() < std::time::Duration::from_secs(15), "{err:#}");
    let text = err.to_string();
    assert!(
        text.contains("stopped reading stdin") || text.contains("child closed stdout"),
        "got: {text}"
    );
}

#[tokio::test]
async fn provider_rpcs_round_trip_without_shutdown() {
    let p = SidecarPlugin::spawn(vec!["testdata/provider_plugin.sh".into()])
        .await
        .unwrap();
    p.set_capabilities(vec![crate::PROVIDER_CREDENTIALS.into()]);

    let started = p
        .provider_auth_start("example", "example-login")
        .await
        .unwrap();
    assert_eq!(started.operation_id, "op-test");

    let completed = p.provider_auth_poll(&started.operation_id).await.unwrap();
    assert!(matches!(completed, crate::ProviderAuthPoll::Completed(_)));
    p.provider_auth_cancel(&started.operation_id).await.unwrap();

    let envelope = gray_core::credential::CredentialEnvelope::new(
        "example-sub",
        "example",
        "example-login",
        "sha256:test",
        gray_core::credential::CredentialMaterial {
            secrets: gray_core::credential::SecretMap::from_iter([("access_token", "test-access")]),
            metadata: [("account_id".into(), "acct_test".into())]
                .into_iter()
                .collect(),
            expires_at: None,
        },
    )
    .unwrap();
    let refreshed = p
        .provider_auth_refresh(&crate::ProviderRefreshRequest {
            provider: "example".into(),
            auth_method: "example-login".into(),
            profile_binding: "sha256:test".into(),
            credential: envelope.clone(),
        })
        .await
        .unwrap();
    assert_eq!(refreshed.secrets.get("refresh_token"), Some("test-refresh"));

    assert!(matches!(
        p.provider_auth_revoke(&crate::ProviderRevokeRequest {
            provider: "example".into(),
            auth_method: "example-login".into(),
            profile_binding: "sha256:test".into(),
            credential: envelope.clone(),
        })
        .await
        .unwrap(),
        crate::ProviderRevokeResult::Revoked
    ));

    let models = p
        .provider_models(&crate::ProviderModelsRequest {
            provider: "example".into(),
            auth_method: "example-login".into(),
            profile_binding: "sha256:test".into(),
            credential: envelope,
        })
        .await
        .unwrap();
    assert_eq!(models.models[0].id, "gpt-test");

    let error = p.provider_auth_poll("bad").await.unwrap_err();
    assert!(matches!(
        error,
        crate::ProviderRpcError::Protocol(message) if message == "invalid provider auth state"
    ));
    p.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test]
async fn provider_rpc_requires_the_sensitive_capability() {
    let p = SidecarPlugin::spawn(vec!["testdata/provider_plugin.sh".into()])
        .await
        .unwrap();
    let error = p
        .provider_auth_start("example", "example-login")
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::ProviderRpcError::CapabilityMissing(capability)
            if capability == crate::PROVIDER_CREDENTIALS
    ));
    p.shutdown(std::time::Duration::from_secs(2)).await;
}

/// `session.id` rides `prompt/context`: anonymous until the builder pins the
/// agent's session, then pinned — the freeze-per-session contract a
/// session-scoped plugin (memory) builds on.
#[tokio::test]
async fn prompt_context_carries_the_agent_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let echo = dir.path().join("echo.log");
    let p = SidecarPlugin::spawn(vec![
        "testdata/session_echo_plugin.sh".into(),
        echo.to_string_lossy().into_owned(),
    ])
    .await
    .unwrap();
    assert_eq!(
        crate::Plugin::prompt_context(&p, "/cwd").await.as_deref(),
        Some("ok")
    );
    let sent = std::fs::read_to_string(&echo).unwrap();
    assert!(
        sent.contains(r#""session":{"cwd":"/cwd","id":""}"#),
        "{sent}"
    );
    crate::Plugin::set_session_id(&p, "sess-abc-123");
    assert_eq!(
        crate::Plugin::prompt_context(&p, "/cwd").await.as_deref(),
        Some("ok")
    );
    let sent = std::fs::read_to_string(&echo).unwrap();
    assert!(
        sent.contains(r#""session":{"cwd":"/cwd","id":"sess-abc-123"}"#),
        "{sent}"
    );
    p.shutdown(std::time::Duration::from_secs(2)).await;
}

#[test]
fn a_model_picker_reply_wins_over_prompt_and_text() {
    use gray_core::agent::CommandOutcome;
    let parse = |v: serde_json::Value| super::command_outcome(&v);
    assert_eq!(
        parse(serde_json::json!({"model_picker": "fusion", "text": "fallback"})),
        Some(CommandOutcome::ModelPicker("fusion".into()))
    );
    assert_eq!(
        parse(serde_json::json!({"model_picker": "", "prompt": "go", "text": "t"})),
        Some(CommandOutcome::Prompt("go".into())),
        "an empty model_picker falls through"
    );
    assert_eq!(
        parse(serde_json::json!({"text": "hi"})),
        Some(CommandOutcome::Say("hi".into()))
    );
    assert_eq!(parse(serde_json::json!({})), None);
}

#[tokio::test]
async fn idless_host_frames_reach_the_notify_handler() {
    let p = SidecarPlugin::spawn(vec!["testdata/notify_plugin.sh".into()])
        .await
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, Value)>();
    p.set_notify_handler(Arc::new(move |m, v| {
        let _ = tx.send((m, v));
    }));
    let out = p.tools()[0]
        .execute(&ToolContext::default(), serde_json::json!({}))
        .await;
    assert!(!out.is_error, "got: {}", out.content);
    assert_eq!(out.content, "pong");
    let got = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("notification within 5s")
        .expect("channel open");
    assert_eq!(got.0, "host/tools_changed");
    assert_eq!(got.1["reason"], "test");
    p.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test]
async fn dynamic_plugin_refreshes_tools_on_tools_changed() {
    let dir = tempfile::tempdir().unwrap();
    let flag = dir.path().join("flag");
    let p = SidecarPlugin::spawn(vec![
        "testdata/dynamic_tools_plugin.sh".into(),
        flag.to_string_lossy().into_owned(),
    ])
    .await
    .unwrap();
    assert!(p.is_dynamic());
    let names = |p: &SidecarPlugin| {
        p.tools()
            .iter()
            .map(|t| t.def().name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&p), vec!["dyn_a"]);
    let t = p.tools()[0].clone();
    let out = t
        .execute(&ToolContext::default(), serde_json::json!({}))
        .await;
    assert!(!out.is_error, "got: {}", out.content);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while names(&p).len() < 2 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(names(&p), vec!["dyn_a", "dyn_b"]);
    p.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test]
async fn tools_changed_during_initial_refresh_is_not_lost() {
    // The fixture answers the first `plugin/tools` with [early_a] and
    // immediately emits `host/tools_changed` — inside spawn's initial
    // refresh window. The handler is installed before that refresh, so
    // the notification must still land and trigger a second refresh.
    let p = SidecarPlugin::spawn(vec!["testdata/early_tools_changed_plugin.sh".into()])
        .await
        .unwrap();
    assert!(p.is_dynamic());
    let names = |p: &SidecarPlugin| {
        p.tools()
            .iter()
            .map(|t| t.def().name.clone())
            .collect::<Vec<_>>()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while names(&p).len() < 2 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(names(&p), vec!["early_a", "early_b"]);
    p.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test]
async fn dropping_a_dynamic_plugin_frees_its_transport() {
    let dir = tempfile::tempdir().unwrap();
    let flag = dir.path().join("flag");
    let p = SidecarPlugin::spawn(vec![
        "testdata/dynamic_tools_plugin.sh".into(),
        flag.to_string_lossy().into_owned(),
    ])
    .await
    .unwrap();
    assert!(p.is_dynamic());
    let weak = Arc::downgrade(&p.transport);
    drop(p);
    assert!(
        weak.upgrade().is_none(),
        "dynamic SidecarPlugin leaked its Transport (notify-handler cycle)"
    );
}

#[tokio::test]
async fn tool_call_reply_images_and_media_are_attached() {
    let p = SidecarPlugin::spawn(vec!["testdata/media_plugin.sh".into()])
        .await
        .unwrap();
    let t = p.tools()[0].clone();
    assert_eq!(t.def().name, "shot");
    let out = t
        .execute(&ToolContext::default(), serde_json::json!({}))
        .await;
    assert!(!out.is_error, "got: {}", out.content);
    assert_eq!(out.content, "here");
    assert_eq!(out.images.len(), 1, "malformed image must be dropped");
    assert_eq!(out.images[0].media_type, "image/png");
    assert_eq!(out.images[0].data, "iVBORw0KGgo=");
    assert_eq!(out.media.len(), 1);
    assert_eq!(out.media[0].media_type, "application/pdf");
    assert_eq!(
        out.media[0].fallback,
        vec![gray_core::message::ContentBlock::Text {
            text: "pdf text".into()
        }]
    );
}
