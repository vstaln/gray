use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

pub struct GrayAssets;

impl AssetSource for GrayAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "icons/gray-logo.svg" => Some(include_bytes!("../assets/icons/gray-logo.svg")),
            "icons/claude.svg" => Some(include_bytes!("../assets/icons/claude.svg")),
            "icons/openai.svg" => Some(include_bytes!("../assets/icons/openai.svg")),
            "icons/g/arrow-down.svg" => Some(include_bytes!("../assets/icons/g/arrow-down.svg")),
            "icons/g/arrow-left.svg" => Some(include_bytes!("../assets/icons/g/arrow-left.svg")),
            "icons/g/arrow-up.svg" => Some(include_bytes!("../assets/icons/g/arrow-up.svg")),
            "icons/g/book.svg" => Some(include_bytes!("../assets/icons/g/book.svg")),
            "icons/g/bot.svg" => Some(include_bytes!("../assets/icons/g/bot.svg")),
            "icons/g/check.svg" => Some(include_bytes!("../assets/icons/g/check.svg")),
            "icons/g/chevron-down.svg" => {
                Some(include_bytes!("../assets/icons/g/chevron-down.svg"))
            }
            "icons/g/chevron-right.svg" => {
                Some(include_bytes!("../assets/icons/g/chevron-right.svg"))
            }
            "icons/g/circle-x.svg" => Some(include_bytes!("../assets/icons/g/circle-x.svg")),
            "icons/g/close.svg" => Some(include_bytes!("../assets/icons/g/close.svg")),
            "icons/g/copy.svg" => Some(include_bytes!("../assets/icons/g/copy.svg")),
            "icons/g/external-link.svg" => {
                Some(include_bytes!("../assets/icons/g/external-link.svg"))
            }
            "icons/g/file-text.svg" => Some(include_bytes!("../assets/icons/g/file-text.svg")),
            "icons/g/file.svg" => Some(include_bytes!("../assets/icons/g/file.svg")),
            "icons/g/folder-open.svg" => Some(include_bytes!("../assets/icons/g/folder-open.svg")),
            "icons/g/folder.svg" => Some(include_bytes!("../assets/icons/g/folder.svg")),
            "icons/g/github.svg" => Some(include_bytes!("../assets/icons/g/github.svg")),
            "icons/g/inbox.svg" => Some(include_bytes!("../assets/icons/g/inbox.svg")),
            "icons/g/info.svg" => Some(include_bytes!("../assets/icons/g/info.svg")),
            "icons/g/layout-dashboard.svg" => {
                Some(include_bytes!("../assets/icons/g/layout-dashboard.svg"))
            }
            "icons/g/moon.svg" => Some(include_bytes!("../assets/icons/g/moon.svg")),
            "icons/g/palette.svg" => Some(include_bytes!("../assets/icons/g/palette.svg")),
            "icons/g/panel-bottom.svg" => {
                Some(include_bytes!("../assets/icons/g/panel-bottom.svg"))
            }
            "icons/g/panel-left.svg" => Some(include_bytes!("../assets/icons/g/panel-left.svg")),
            "icons/g/panel-right.svg" => Some(include_bytes!("../assets/icons/g/panel-right.svg")),
            "icons/g/plus.svg" => Some(include_bytes!("../assets/icons/g/plus.svg")),
            "icons/g/refresh-cw.svg" => Some(include_bytes!("../assets/icons/g/refresh-cw.svg")),
            "icons/g/search.svg" => Some(include_bytes!("../assets/icons/g/search.svg")),
            "icons/g/settings.svg" => Some(include_bytes!("../assets/icons/g/settings.svg")),
            "icons/g/square-terminal.svg" => {
                Some(include_bytes!("../assets/icons/g/square-terminal.svg"))
            }
            "icons/g/square.svg" => Some(include_bytes!("../assets/icons/g/square.svg")),
            "icons/g/star.svg" => Some(include_bytes!("../assets/icons/g/star.svg")),
            "icons/g/sun.svg" => Some(include_bytes!("../assets/icons/g/sun.svg")),
            "icons/g/triangle-alert.svg" => {
                Some(include_bytes!("../assets/icons/g/triangle-alert.svg"))
            }
            "icons/g/undo.svg" => Some(include_bytes!("../assets/icons/g/undo.svg")),
            "bg/andromeda.png" => Some(include_bytes!("../assets/andromeda-dither.png")),
            "bg/bluemarble.png" => Some(include_bytes!("../assets/bluemarble-dither.png")),
            _ => None,
        };
        if let Some(b) = bytes {
            return Ok(Some(Cow::Borrowed(b)));
        }
        gpui_kit::assets::Assets::new("").load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::Assets::new("").list(path)
    }
}
