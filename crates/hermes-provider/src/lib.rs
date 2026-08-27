//! hermes-provider — secret + model provider crate.
//!
//! T0021: 1:1 port of `agent/auxiliary_client.py` (10831) slice1 → `auxiliary_slice1.rs` (first 900).
//! T0021: 1:1 port of `agent/auxiliary_client.py` (10831) slice2 → `auxiliary_slice2.rs` (900-1800).
//! T0022: 1:1 port of `agent/model_metadata.py` (3767) slice1 → `model_metadata_slice1.rs` (first 900).
//! T0032: 1:1 port of `agent/secret_sources/bitwarden.py` (1055) → `bitwarden.rs`.
//! T0033: 1:1 port of `agent/account_usage.py` (902) → `account_usage.rs`.
//! T0034: 1:1 port of `hermes_cli/nous_account.py` (814) → `nous_account.rs`.
//! T0035: 1:1 port of `agent/secret_sources/onepassword.py` (682) → `onepassword.rs`.
//! T0036: 1:1 port of `hermes_cli/nous_billing.py` (675) → `nous_billing.rs`.
//! T0037: 1:1 port of `agent/azure_identity_adapter.py` (571) → `azure_identity.rs`.
//! T0038: 1:1 port of `agent/secret_sources/registry.py` (564) → `secret_registry.rs`.
//! T0039: 1:1 port of `agent/billing_view.py` (511) → `billing_view.rs`.
//! T0040: 1:1 port of `agent/secret_sources/command.py` (501) → `command_secret.rs`.
//! T0042: 1:1 port of `agent/secret_sources/base.py` (336) → `secret_base.rs`.
//! T0043: 1:1 port of `agent/billing_usage.py` (323) → `billing_usage.rs`.
//! T0044: 1:1 port of `hermes_cli/credential_lifecycle.py` (272) → `credential_lifecycle.rs`.
//! T0045: 1:1 port of `agent/vertex_adapter.py` (251) → `vertex_adapter.rs`.
//! T0046: 1:1 port of `agent/secret_sources/_cache.py` (215) → `secret_cache.rs`.
//! T0047: 1:1 port of `hermes_cli/proxy/adapters/nous_portal.py` (199) → `nous_portal_adapter.rs`.
//! T0048: 1:1 port of `agent/credential_persistence.py` (174) → `credential_persistence.rs`.
//! T0049: 1:1 port of `hermes_cli/proxy/adapters/xai.py` (145) → `xai_adapter.rs`.
//! T0050: 1:1 port of `agent/aux_accounting.py` (138) → `aux_accounting.rs`.
//! Crate root re-exports the sliced modules; each `*_sliceN.rs` covers a
//! Python source file's 1:1 port.
//!
//! Workspace layout per `docs/port/00-MASTER-DESIGN.md` §2:
//! `hermes-provider ← transports/: chat_completions, anthropic, codex, bedrock, gemini, …`
//! Secret sources live here as provider-adjacent fetchers (Bitwarden, 1Password, …).

pub mod account_usage;
pub mod aux_accounting;
pub mod auxiliary_slice1;
pub mod auxiliary_slice2;
pub mod model_metadata_slice1;
pub mod azure_identity;
pub mod billing_usage;
pub mod billing_view;
pub mod bitwarden;
pub mod command_secret;
pub mod credential_lifecycle;
pub mod credential_persistence;
pub mod nous_account;
pub mod nous_billing;
pub mod nous_portal_adapter;
pub mod onepassword;
pub mod secret_base;
pub mod secret_cache;
pub mod secret_registry;
pub mod vertex_adapter;
pub mod xai_adapter;
