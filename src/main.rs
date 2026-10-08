//! SpurGit bootstrap: fonts, theme, window, diagnostics.
//! Git access goes through git.rs (WSL or native Git on Windows, local Git elsewhere).

// Release builds are GUI apps: no console window when launched from Explorer
// (debug builds keep the console for `cargo run` diagnostics; release dev
// builds log to %APPDATA%\SpurGit\logs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accounts;
mod discovery;
mod git;
mod graph;
mod i18n;
mod icon;
mod logging;
mod model;
mod overview;
mod process;
mod settings;
mod status;
mod theme;
mod ui;
mod update;

use std::borrow::Cow;

use gpui_kit::component::Root;
use gpui_kit::{App, AppContext};

use crate::logging::log;

/// Zeron bundles Geist + Geist Mono (SIL OFL, see assets/fonts/licenses);
/// we register the same faces before the first frame.
fn load_fonts(cx: &mut App) {
    cx.text_system()
        .add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-Medium.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist-SemiBold.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono-Medium.ttf").as_slice()),
        ])
        .expect("failed to load bundled Geist fonts");
}

/// The window's root view: the shell inside the kit's overlay root.
fn build_root(window: &mut gpui_kit::Window, cx: &mut App) -> gpui_kit::Entity<Root> {
    let view = cx.new(|cx| ui::SpurShell::new(window, cx));
    cx.new(|cx| Root::new(view, window, cx))
}

fn startup_diagnostics() {
    match git::git_version() {
        Ok(v) => log!("{v}"),
        Err(e) => log!("git unavailable: {e}"),
    }
    #[cfg(windows)]
    log!(
        "wsl: {} (distro {})",
        if git::wsl_available() { "available" } else { "unavailable" },
        git::distro()
    );
    // Settings problems and a malformed GIS_JOBS must be visible, never
    // silently ignored.
    let (_, settings_diagnostics) = settings::load();
    for diagnostic in settings_diagnostics {
        log!("settings: {diagnostic}");
    }
    if let Err(err) = process::jobs_from_env() {
        log!("{err}");
    }
}

/// Apps started from Finder get a minimal PATH without Homebrew, so the
/// user's own `git` would be missed. Prepend Homebrew's directories when they
/// are not already on the PATH.
#[cfg(target_os = "macos")]
fn with_homebrew(path: &str) -> String {
    let mut dirs: Vec<&str> = ["/opt/homebrew/bin", "/usr/local/bin"]
        .into_iter()
        .filter(|dir| !path.split(':').any(|have| have == *dir))
        .collect();
    dirs.extend(Some(path).filter(|path| !path.is_empty()));
    dirs.join(":")
}

fn main() {
    #[cfg(target_os = "macos")]
    // SAFETY: first statement of main; no other thread exists yet.
    unsafe {
        std::env::set_var("PATH", with_homebrew(&std::env::var("PATH").unwrap_or_default()));
    }
    logging::init();
    log!("spur {} (pid {})", env!("CARGO_PKG_VERSION"), std::process::id());
    log!("args: {:?}", std::env::args().collect::<Vec<String>>());

    // --size=WxH sets the initial window size (default 1280x800)
    let (w, h) = std::env::args()
        .find(|a| a.starts_with("--size="))
        .and_then(|a| {
            let s = a.trim_start_matches("--size=");
            let (w, h) = s.split_once('x')?;
            Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
        })
        .unwrap_or((1280., 800.));

    // Full icon catalog: the default `Assets` bundle only embeds a small
    // subset (Search, Close, ...), so use AllAssets for chips and toolbars.
    let app = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    // macOS keeps the app running after its last window closes; a Dock click
    // then asks for a window back. Tabs and roots restore from the settings.
    app.on_reopen(move |cx| {
        if cx.windows().is_empty()
            && let Err(err) = cx.open_window(ui::window_options(w, h), build_root)
        {
            log!("reopen: could not open a window: {err}");
        }
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        load_fonts(cx);
        theme::init(cx);
        ui::app_icon::init(cx);
        // Backend probes (`git --version`, WSL reachability) can start a cold
        // distribution; they run off the UI thread so the window and shell
        // open first.
        cx.background_executor()
            .spawn(async { startup_diagnostics() })
            .detach();
        // Remappable bindings: keymap.json overrides load first, then
        // every keystroke registers from the merged table.
        ui::shortcuts::init(cx);
        cx.bind_keys(ui::shortcuts::key_bindings(cx));
        cx.spawn(async move |cx| {
            cx.open_window(ui::window_options(w, h), build_root)
                .expect("failed to open window");
        })
        .detach();
    });
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::with_homebrew;

    #[test]
    fn homebrew_dirs_come_first_and_are_never_duplicated() {
        assert_eq!(
            with_homebrew("/usr/bin:/bin"),
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
        );
        assert_eq!(
            with_homebrew("/opt/homebrew/bin:/usr/bin"),
            "/usr/local/bin:/opt/homebrew/bin:/usr/bin"
        );
        assert_eq!(with_homebrew(""), "/opt/homebrew/bin:/usr/local/bin");
    }
}
