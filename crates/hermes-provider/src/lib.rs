//! hermes-provider — secret + model provider crate.
//!
//! T0032: 1:1 port of `agent/secret_sources/bitwarden.py` (1055) → `bitwarden.rs`.
//! T0033: 1:1 port of `agent/account_usage.py` (902) → `account_usage.rs`.
//! T0034: 1:1 port of `hermes_cli/nous_account.py` (814) → `nous_account.rs`.
//! T0035: 1:1 port of `agent/secret_sources/onepassword.py` (682) → `onepassword.rs`.
//! Crate root re-exports the sliced modules; each `*_sliceN.rs` covers a
//! Python source file's 1:1 port.
//!
//! Workspace layout per `docs/port/00-MASTER-DESIGN.md` §2:
//! `hermes-provider ← transports/: chat_completions, anthropic, codex, bedrock, gemini, …`
//! Secret sources live here as provider-adjacent fetchers (Bitwarden, 1Password, …).

pub mod account_usage;
pub mod bitwarden;
pub mod nous_account;
pub mod onepassword;
