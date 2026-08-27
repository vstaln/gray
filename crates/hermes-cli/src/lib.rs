//! hermes-cli — CLI entry point and subcommand dispatch.
//!
//! T0681: 1:1 port of `hermes_cli/main.py` slice 1/10.
//! Crate root re-exports the sliced modules; each `main_sliceN.rs`
//! covers ~1/10 of the 14 268-line Python source.

pub mod main_slice1;
pub mod kanban_db_slice1;
pub mod auth_slice1;
pub mod update_cmd_slice1;
pub mod gateway_slice1;
pub mod models_slice1;
pub mod config_slice1;
pub mod tools_config_slice1;
pub mod tools_config_slice2;
pub mod config_defaults_slice1;
pub mod model_switch_slice1;
pub mod cli_commands_slice1;
pub mod setup_slice1;
pub mod kanban_slice1;
pub mod model_setup_slice1;
pub mod doctor_slice1;
pub mod profiles_slice1;
pub mod runtime_provider_slice1;
pub mod goals_slice1;
pub mod backup_slice1;
pub mod session_recovery_slice1;
pub mod skills_hub_slice1;
