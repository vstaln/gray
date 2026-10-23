//! Vendored IBM Carbon-style icon set (Apache-2.0), rendered through the
//! standard `Icon` pipeline via `IconNamed`. Geometric, sharp-cornered —
//! matches the brutalist theme better than the default lucide set.

use gpui_kit::{SharedString, assets::IconNamed};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GIcon {
    ArrowDown,
    ArrowLeft,
    ArrowUp,
    BookOpen,
    Bot,
    Check,
    ChevronDown,
    ChevronRight,
    CircleX,
    Close,
    Copy,
    ExternalLink,
    File,
    FileText,
    Folder,
    FolderOpen,
    Github,
    Inbox,
    Info,
    LayoutDashboard,
    Moon,
    Palette,
    PanelBottom,
    PanelLeft,
    PanelRight,
    Plus,
    RefreshCw,
    Search,
    Settings,
    Square,
    SquareTerminal,
    Star,
    Sun,
    TriangleAlert,
    Undo2,
}

impl IconNamed for GIcon {
    fn path(self) -> SharedString {
        let name = match self {
            Self::ArrowDown => "arrow-down",
            Self::ArrowLeft => "arrow-left",
            Self::ArrowUp => "arrow-up",
            Self::BookOpen => "book",
            Self::Bot => "bot",
            Self::Check => "check",
            Self::ChevronDown => "chevron-down",
            Self::ChevronRight => "chevron-right",
            Self::CircleX => "circle-x",
            Self::Close => "close",
            Self::Copy => "copy",
            Self::ExternalLink => "external-link",
            Self::File => "file",
            Self::FileText => "file-text",
            Self::Folder => "folder",
            Self::FolderOpen => "folder-open",
            Self::Github => "github",
            Self::Inbox => "inbox",
            Self::Info => "info",
            Self::LayoutDashboard => "layout-dashboard",
            Self::Moon => "moon",
            Self::Palette => "palette",
            Self::PanelBottom => "panel-bottom",
            Self::PanelLeft => "panel-left",
            Self::PanelRight => "panel-right",
            Self::Plus => "plus",
            Self::RefreshCw => "refresh-cw",
            Self::Search => "search",
            Self::Settings => "settings",
            Self::Square => "square",
            Self::SquareTerminal => "square-terminal",
            Self::Star => "star",
            Self::Sun => "sun",
            Self::TriangleAlert => "triangle-alert",
            Self::Undo2 => "undo",
        };
        format!("icons/g/{name}.svg").into()
    }
}
