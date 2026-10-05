use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gray_core::credential::{CredentialEnvelope, CredentialMaterial, CredentialSource, SecretMap};
use gray_plugin::{
    AuthMethodDecl, ProviderAuthPoll, ProviderAuthStart, ProviderAuthorizationDecl, ProviderDecl,
    ProviderHeaderDecl, ProviderModelCatalog, ProviderModelsRequest, ProviderRefreshRequest,
    ProviderRevokeRequest, ProviderRevokeResult, ProviderTransportDecl,
};

use super::*;
use gray_plugin::{ProviderRpcError, ProviderRpcFailure};

struct FakeRefresh {
    calls: Arc<AtomicUsize>,
    next: CredentialMaterial,
}

#[async_trait::async_trait]
impl ProviderRpc for FakeRefresh {
    async fn auth_start(
        &self,
        _provider: &str,
        _auth_method: &str,
    ) -> Result<ProviderAuthStart, ProviderRpcError> {
        unimplemented!("not needed for refresh tests")
    }
    async fn auth_poll(&self, _operation_id: &str) -> Result<ProviderAuthPoll, ProviderRpcError> {
        unimplemented!("not needed for refresh tests")
    }
    async fn auth_cancel(&self, _operation_id: &str) -> Result<(), ProviderRpcError> {
        unimplemented!("not needed for refresh tests")
    }
    async fn refresh(
        &self,
        _request: ProviderRefreshRequest,
    ) -> Result<CredentialMaterial, ProviderRpcError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.next.clone())
    }
    async fn revoke(
        &self,
        _request: ProviderRevokeRequest,
    ) -> Result<ProviderRevokeResult, ProviderRpcError> {
        unimplemented!("not needed for refresh tests")
    }
    async fn models(
        &self,
        _request: ProviderModelsRequest,
    ) -> Result<ProviderModelCatalog, ProviderRpcError> {
        unimplemented!("not needed for refresh tests")
    }
    async fn shutdown(&self) {}
}

fn expires_in(seconds: u64) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + seconds
}

fn provider() -> ProviderDecl {
    ProviderDecl {
        id: "example".into(),
        name: "Example".into(),
        transport: ProviderTransportDecl {
            kind: "openai-responses".into(),
            base_url: "https://example.test/v1".parse().unwrap(),
            authorization: ProviderAuthorizationDecl {
                kind: "bearer".into(),
                secret_name: "access_token".into(),
            },
            request: gray_plugin::ProviderRequestPolicyDecl {
                prompt_cache_key: false,
                store: false,
                include_reasoning_encrypted: true,
                previous_response_id: false,
                tool_choice: Some("auto".into()),
                parallel_tool_calls: Some(true),
                text_verbosity: Some("low".into()),
            },
            headers: vec![ProviderHeaderDecl {
                name: "originator".into(),
                value: Some("gray".into()),
                source: None,
                required: false,
            }],
        },
        auth_methods: vec![AuthMethodDecl {
            id: "example-login".into(),
            name: "Example login".into(),
            kind: "oauth".into(),
            operations: vec!["refresh".into()],
        }],
    }
}

fn installed() -> InstalledProvider {
    let provider = provider();
    let auth_method = provider.auth_methods[0].clone();
    InstalledProvider {
        plugin: "example-sub".into(),
        provider,
        auth_method,
        profile_binding: "sha256:test".into(),
        argv: Vec::new(),
    }
}

fn material(expires_at: Option<u64>, refresh_token: &str) -> CredentialMaterial {
    CredentialMaterial {
        secrets: SecretMap::from_iter([
            ("access_token", format!("access-{refresh_token}")),
            ("refresh_token", refresh_token.to_string()),
        ]),
        metadata: [("account_id".into(), "acct_test".into())]
            .into_iter()
            .collect(),
        expires_at,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_callers_share_one_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let store = CredentialStore::new(dir.path().join("auth.json"));
    let current = CredentialEnvelope::new(
        "example-sub",
        "example",
        "example-login",
        "sha256:test",
        material(Some(expires_in(30)), "old-refresh"),
    )
    .unwrap();
    store.put_plugin(current).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let rpc = Arc::new(FakeRefresh {
        calls: calls.clone(),
        next: material(Some(expires_in(3600)), "new-refresh"),
    });
    let source = shared_plugin_source(installed(), store, rpc);
    let (first, second, third) = tokio::join!(source.acquire(), source.acquire(), source.acquire());
    for lease in [first, second, third] {
        assert_eq!(
            lease.unwrap().secrets.get("refresh_token"),
            Some("new-refresh")
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn terminal_refresh_rejection_removes_one_entry() {
    struct Reject;
    #[async_trait::async_trait]
    impl ProviderRpc for Reject {
        async fn auth_start(
            &self,
            _provider: &str,
            _auth_method: &str,
        ) -> Result<ProviderAuthStart, ProviderRpcError> {
            unimplemented!()
        }
        async fn auth_poll(
            &self,
            _operation_id: &str,
        ) -> Result<ProviderAuthPoll, ProviderRpcError> {
            unimplemented!()
        }
        async fn auth_cancel(&self, _operation_id: &str) -> Result<(), ProviderRpcError> {
            unimplemented!()
        }
        async fn refresh(
            &self,
            _request: ProviderRefreshRequest,
        ) -> Result<CredentialMaterial, ProviderRpcError> {
            Err(ProviderRpcError::Rpc(ProviderRpcFailure {
                code: "invalid_grant".into(),
                message: "rotated elsewhere".into(),
                retryable: false,
                terminal: true,
            }))
        }
        async fn revoke(
            &self,
            _request: ProviderRevokeRequest,
        ) -> Result<ProviderRevokeResult, ProviderRpcError> {
            unimplemented!()
        }
        async fn models(
            &self,
            _request: ProviderModelsRequest,
        ) -> Result<ProviderModelCatalog, ProviderRpcError> {
            unimplemented!()
        }
        async fn shutdown(&self) {}
    }

    let dir = tempfile::tempdir().unwrap();
    let store = CredentialStore::new(dir.path().join("auth.json"));
    store
        .put_plugin(
            CredentialEnvelope::new(
                "example-sub",
                "example",
                "example-login",
                "sha256:test",
                material(Some(expires_in(30)), "old-refresh"),
            )
            .unwrap(),
        )
        .unwrap();
    store
        .put_plugin(
            CredentialEnvelope::new(
                "example-sub",
                "example",
                "other-login",
                "sha256:test",
                material(Some(expires_in(3600)), "unrelated"),
            )
            .unwrap(),
        )
        .unwrap();
    let source = shared_plugin_source(installed(), store.clone(), Arc::new(Reject));
    let error = source.acquire().await.unwrap_err();
    assert!(matches!(error, CredentialError::ReauthRequired(_)));
    assert!(
        store
            .read_plugin("plugin:example-sub:example:example-login")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .read_plugin("plugin:example-sub:example:other-login")
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn profile_mismatch_does_not_delete_credential() {
    struct Mismatch;
    #[async_trait::async_trait]
    impl ProviderRpc for Mismatch {
        async fn auth_start(
            &self,
            _provider: &str,
            _auth_method: &str,
        ) -> Result<ProviderAuthStart, ProviderRpcError> {
            unimplemented!()
        }
        async fn auth_poll(
            &self,
            _operation_id: &str,
        ) -> Result<ProviderAuthPoll, ProviderRpcError> {
            unimplemented!()
        }
        async fn auth_cancel(&self, _operation_id: &str) -> Result<(), ProviderRpcError> {
            unimplemented!()
        }
        async fn refresh(
            &self,
            _request: ProviderRefreshRequest,
        ) -> Result<CredentialMaterial, ProviderRpcError> {
            Err(ProviderRpcError::Rpc(ProviderRpcFailure {
                code: "profile_mismatch".into(),
                message: "new profile".into(),
                retryable: false,
                terminal: true,
            }))
        }
        async fn revoke(
            &self,
            _request: ProviderRevokeRequest,
        ) -> Result<ProviderRevokeResult, ProviderRpcError> {
            unimplemented!()
        }
        async fn models(
            &self,
            _request: ProviderModelsRequest,
        ) -> Result<ProviderModelCatalog, ProviderRpcError> {
            unimplemented!()
        }
        async fn shutdown(&self) {}
    }

    let dir = tempfile::tempdir().unwrap();
    let store = CredentialStore::new(dir.path().join("auth.json"));
    store
        .put_plugin(
            CredentialEnvelope::new(
                "example-sub",
                "example",
                "example-login",
                "sha256:test",
                material(Some(expires_in(30)), "old-refresh"),
            )
            .unwrap(),
        )
        .unwrap();
    let source = shared_plugin_source(installed(), store.clone(), Arc::new(Mismatch));
    let error = source.acquire().await.unwrap_err();
    assert!(matches!(error, CredentialError::Invalid(_)));
    assert!(
        store
            .read_plugin("plugin:example-sub:example:example-login")
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn expired_credential_fails_closed() {
    struct Unreachable;
    #[async_trait::async_trait]
    impl ProviderRpc for Unreachable {
        async fn auth_start(
            &self,
            _provider: &str,
            _auth_method: &str,
        ) -> Result<ProviderAuthStart, ProviderRpcError> {
            unimplemented!()
        }
        async fn auth_poll(
            &self,
            _operation_id: &str,
        ) -> Result<ProviderAuthPoll, ProviderRpcError> {
            unimplemented!()
        }
        async fn auth_cancel(&self, _operation_id: &str) -> Result<(), ProviderRpcError> {
            unimplemented!()
        }
        async fn refresh(
            &self,
            _request: ProviderRefreshRequest,
        ) -> Result<CredentialMaterial, ProviderRpcError> {
            unimplemented!("expired credentials must not reach refresh")
        }
        async fn revoke(
            &self,
            _request: ProviderRevokeRequest,
        ) -> Result<ProviderRevokeResult, ProviderRpcError> {
            unimplemented!()
        }
        async fn models(
            &self,
            _request: ProviderModelsRequest,
        ) -> Result<ProviderModelCatalog, ProviderRpcError> {
            unimplemented!()
        }
        async fn shutdown(&self) {}
    }

    let dir = tempfile::tempdir().unwrap();
    let store = CredentialStore::new(dir.path().join("auth.json"));
    store
        .put_plugin(
            CredentialEnvelope::new(
                "example-sub",
                "example",
                "example-login",
                "sha256:test",
                material(Some(1), "expired"),
            )
            .unwrap(),
        )
        .unwrap();
    let source = shared_plugin_source(installed(), store.clone(), Arc::new(Unreachable));
    let error = source.acquire().await.unwrap_err();
    assert!(matches!(error, CredentialError::ReauthRequired(_)));
    assert!(
        store
            .read_plugin("plugin:example-sub:example:example-login")
            .unwrap()
            .is_none()
    );
}
