//! sluice-ui — GPUI views. The only layer allowed to depend on gpui (02 §1).
//! Visual language follows the Claude Design prototype (`SluiceDesktop.dc.html`,
//! Broadsheet design system: Source Serif 4, cyan / magenta / process-yellow inks).

pub mod assets;
pub mod icons;
pub mod log;
pub mod theme;
pub mod workbench;

use gpui::{App, KeyBinding};

pub use theme::Theme;
pub use workbench::Workbench;

/// Fonts, key bindings. Call once from the application entry point.
pub fn init(cx: &mut App) {
    assets::load_fonts(cx);
    cx.bind_keys([
        KeyBinding::new("up", workbench::MoveUp, Some("Workbench")),
        KeyBinding::new("down", workbench::MoveDown, Some("Workbench")),
        KeyBinding::new("k", workbench::MoveUp, Some("Workbench")),
        KeyBinding::new("j", workbench::MoveDown, Some("Workbench")),
        KeyBinding::new("home", workbench::SelectFirst, Some("Workbench")),
        KeyBinding::new("end", workbench::SelectLast, Some("Workbench")),
        KeyBinding::new("pageup", workbench::PageUp, Some("Workbench")),
        KeyBinding::new("pagedown", workbench::PageDown, Some("Workbench")),
        // IDEA preset (05 §11): ⌘9 Log · ⌘0 Local Changes · ⌘6 Console · ⌥⌘Y refresh
        KeyBinding::new("cmd-9", workbench::ShowLog, Some("Workbench")),
        KeyBinding::new("cmd-0", workbench::ShowChanges, Some("Workbench")),
        KeyBinding::new("cmd-6", workbench::ShowConsole, Some("Workbench")),
        KeyBinding::new("cmd-alt-y", workbench::Refresh, Some("Workbench")),
        KeyBinding::new("ctrl-alt-y", workbench::Refresh, Some("Workbench")),
    ]);
}
