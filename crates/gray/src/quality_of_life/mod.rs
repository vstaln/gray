//! Terminal niceties that stayed core: kitty graphics plumbing for the
//! composer and the provider-preset helpers that feed `/connect`. The
//! tool-detection importer, `/update everything` and `/setup` moved to
//! the gray-qol plugin.

pub mod cache_stats;
pub mod kitty;
pub mod providers;

/// The `quality_of_life` config flag — still gates provider presets
/// (`/connect`) even though the importer itself moved to gray-qol.
pub(crate) fn enabled() -> bool {
    crate::setup::quality_of_life_enabled()
}
