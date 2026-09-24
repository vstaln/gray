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
    #[cfg(windows)]
    super::debug_log("gray sidecar debug: test before spawn");
    let p = SidecarPlugin::spawn(vec!["testdata/wedged_stdin_plugin.sh".into()])
        .await
        .unwrap();
    #[cfg(windows)]
    super::debug_log("gray sidecar debug: test after spawn");
    let blob = "x".repeat(200 * 1024);
    let t = std::time::Instant::now();
    #[cfg(windows)]
    super::debug_log("gray sidecar debug: test before request");
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
    #[cfg(windows)]
    super::debug_log("gray sidecar debug: test after request");
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
        .provider_auth_start("codex", "chatgpt-subscription")
        .await
        .unwrap();
    assert_eq!(started.operation_id, "op-test");

    let completed = p.provider_auth_poll(&started.operation_id).await.unwrap();
    assert!(matches!(completed, crate::ProviderAuthPoll::Completed(_)));
    p.provider_auth_cancel(&started.operation_id).await.unwrap();

    let envelope = gray_core::credential::CredentialEnvelope::new(
        "codex-auth",
        "codex",
        "chatgpt-subscription",
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
            provider: "codex".into(),
            auth_method: "chatgpt-subscription".into(),
            profile_binding: "sha256:test".into(),
            credential: envelope.clone(),
        })
        .await
        .unwrap();
    assert_eq!(refreshed.secrets.get("refresh_token"), Some("test-refresh"));

    assert!(matches!(
        p.provider_auth_revoke(&crate::ProviderRevokeRequest {
            provider: "codex".into(),
            auth_method: "chatgpt-subscription".into(),
            profile_binding: "sha256:test".into(),
            credential: envelope.clone(),
        })
        .await
        .unwrap(),
        crate::ProviderRevokeResult::Revoked
    ));

    let models = p
        .provider_models(&crate::ProviderModelsRequest {
            provider: "codex".into(),
            auth_method: "chatgpt-subscription".into(),
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
        .provider_auth_start("codex", "chatgpt-subscription")
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::ProviderRpcError::CapabilityMissing(capability)
            if capability == crate::PROVIDER_CREDENTIALS
    ));
    p.shutdown(std::time::Duration::from_secs(2)).await;
}
