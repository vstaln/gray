//! External secret source integrations.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/secret_sources/__init__.py` (41 lines).
//!
//! A secret source is anything that can supply environment-variable-shaped
//! credentials at process startup, _after_ `~/.hermes/.env` has loaded.
//!
//! The contract every source implements is `agent.secret_sources.base.SecretSource`
//! (`crate::secret_base::SecretSource`); the orchestrator that runs the enabled
//! sources (ordering, mapped-beats-bulk precedence, first-claim-wins conflicts,
//! `override_existing` semantics, provenance) is
//! `agent.secret_sources.registry.apply_all` (`crate::secret_registry::apply_all`).
//! Multiple sources can be enabled at once — see `secret_registry` for the
//! precedence ladder. The atomic-write / `0600` / TTL disk-cache substrate is
//! shared across backends in `agent.secret_sources._cache` (`crate::secret_cache`)
//! so the security-sensitive bits live in exactly one place.
//!
//! Currently bundled:
//!
//!   - `bitwarden` — Bitwarden Secrets Manager (`bws` CLI). See
//!     `agent.secret_sources.bitwarden` (`crate::bitwarden`) for the integration.
//!   - `onepassword` — 1Password `op://` secret references (`op` CLI). See
//!     `agent.secret_sources.onepassword` (`crate::onepassword`) for the integration.
//!
//! The bundled set is deliberately closed (policy mirrors memory providers):
//! new third-party secret managers ship as standalone plugin repos that
//! subclass `SecretSource` and register through
//! `PluginContext.register_secret_source()` — they are NOT added to this
//! package. A generic `command` source is a possible future exception; OS
//! keystores (Keychain/DPAPI/libsecret) are under discussion.
//!
//! Python `agent/secret_sources/__init__.py` is a re-export façade:
//! ```python
//! from agent.secret_sources.base import (
//!     SECRET_SOURCE_API_VERSION,
//!     ErrorKind,
//!     FetchResult,
//!     SecretSource,
//!     is_valid_env_name,
//!     run_secret_cli,
//!     scrub_ansi,
//! )
//! ```
//! This module is the Rust equivalent — it re-exports the public contract
//! surface from `crate::secret_base` so consumers can import from one place:
//! `crate::secret_init::{SecretSource, FetchResult, …}`.
//!
//! T0052 — 1:1 port, no cargo (NEVER cargo).

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// Re-export the public contract surface — mirrors
// `from agent.secret_sources.base import …` (lines 33-41).

pub use crate::secret_base::{
    is_valid_env_name, run_secret_cli, scrub_ansi, ErrorKind, FetchResult, SecretSource,
    SECRET_SOURCE_API_VERSION,
};

// `run_secret_cli` also exposes `CompletedProcess`; re-export for completeness
// even though `__init__.py` does not list it — callers that use the helper
// need the return type.
pub use crate::secret_base::{CompletedProcess, DEFAULT_CLI_TIMEOUT_SECONDS, DEFAULT_FETCH_TIMEOUT_SECONDS};
