pub mod errors;
pub mod fetch;
pub mod index;
pub mod ops;
pub mod skills_ops;
pub mod sources;

use std::path::PathBuf;

pub(crate) fn plugins_dir() -> PathBuf {
    gray_core::paths::gray_home_or_temp().join("plugins")
}
