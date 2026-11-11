#![recursion_limit = "512"]
mod app;
mod assets;
mod engine;
mod git;
mod graycmds;
mod icons;
mod motion;
mod plugins;
mod provider;
mod sessions;
mod skills;
mod store;
mod workspace;

use gpui_kit::*;
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    std::env::var_os("GRAY_GUI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".gray")
                .join("gui")
        })
}

fn migrate_legacy_home() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let old = home.join(".gray").join("tui");
    let new = home.join(".gray").join("gui");
    if !new.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &new);
    }
}

fn main() {
    migrate_legacy_home();
    let started = std::time::Instant::now();
    env_logger::init();
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("gray-gui {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("--help" | "-h") => {
            println!(
                "usage: gray-gui [FOLDER]\n\nOpens FOLDER (default: the current directory) as a project."
            );
            return;
        }
        Some(flag) if flag.starts_with('-') => {
            eprintln!("gray-gui: unknown option {flag} (try --help)");
            std::process::exit(2);
        }
        _ => {}
    }
    let initial = std::env::args().nth(1).map(PathBuf::from).or_else(|| {
        if std::env::var_os("GRAY_GUI_NO_CWD").is_some() {
            None
        } else {
            std::env::current_dir().ok()
        }
    });
    let store = store::Store::open(&data_dir()).expect("open gray data directory");
    gpui_kit::application()
        .with_assets(assets::GrayAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            app::bind_keys(cx);
            cx.text_system()
                .add_fonts(vec![
                    // Static weights: gpui's cosmic-text backend loads every
                    // face at its default instance, so a variable font
                    // renders bold/semibold as regular.
                    std::borrow::Cow::Borrowed(include_bytes!(
                        "../fonts/PlusJakartaSans-Regular.ttf"
                    ) as &[u8]),
                    std::borrow::Cow::Borrowed(
                        include_bytes!("../fonts/PlusJakartaSans-Medium.ttf") as &[u8],
                    ),
                    std::borrow::Cow::Borrowed(include_bytes!(
                        "../fonts/PlusJakartaSans-SemiBold.ttf"
                    ) as &[u8]),
                    std::borrow::Cow::Borrowed(
                        include_bytes!("../fonts/PlusJakartaSans-Bold.ttf") as &[u8]
                    ),
                    std::borrow::Cow::Borrowed(
                        include_bytes!("../fonts/CommitMono-400.ttf") as &[u8]
                    ),
                    std::borrow::Cow::Borrowed(
                        include_bytes!("../fonts/CommitMono-700.ttf") as &[u8]
                    ),
                    std::borrow::Cow::Borrowed(
                        include_bytes!("../fonts/AncizarSerif-Var.ttf") as &[u8]
                    ),
                ])
                .expect("load gray fonts");
            app::apply_brand(cx);
            let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1440.), px(900.)));
            let opts = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Gray GUI".into()),
                    ..Default::default()
                }),
                window_background: WindowBackgroundAppearance::Opaque,
                app_id: Some("gray-gui".into()),
                ..Default::default()
            };
            gpui_kit::open_window(opts, cx, move |window, cx| {
                cx.new(|cx| app::GrayApp::new(store, initial, started, window, cx))
            })
            .expect("open window");
            cx.activate(true);
        });
}
