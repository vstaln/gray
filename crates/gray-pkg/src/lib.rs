pub mod errors;
pub mod fetch;
pub mod index;
pub mod ops;

use std::path::PathBuf;

pub(crate) fn plugins_dir() -> PathBuf {
    gray_core::paths::gray_home_or_temp().join("plugins")
}
