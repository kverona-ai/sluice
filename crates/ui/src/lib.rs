//! sluice-ui — GPUI views. The only layer allowed to depend on gpui (02 §1).
//! Visual language follows the Claude Design prototype (`SluiceDesktop.dc.html`,
//! Broadsheet design system: Source Serif 4, cyan / magenta / process-yellow inks).
use std::collections::HashMap;

#[macro_use]
pub mod i18n;
pub mod ai;
pub mod assets;
pub mod changes;
pub mod conflict;
pub mod console;
pub mod diff_view;
pub mod file_view;
pub mod icons;
pub mod keymap;
pub mod log;
pub mod overlays;
pub mod proposals;
pub mod pulls;
pub mod rebase;
pub mod recent;
pub mod theme;
pub mod workbench;
pub mod worktrees;

use gpui::App;

pub use theme::Theme;
pub use workbench::Workbench;

/// Fonts, gpui-component, key bindings. Call once from the application entry point.
pub fn init(cx: &mut App) {
    assets::load_fonts(cx);
    gpui_component::init(cx);
    sync_component_theme(cx, &Theme::light());
    keymap::bind(cx, "idea", &HashMap::new());
}

/// Make gpui-component widgets (text inputs …) use the Broadsheet tokens.
pub fn sync_component_theme(cx: &mut App, t: &Theme) {
    let theme = gpui_component::theme::Theme::global_mut(cx);
    theme.mode = if t.is_dark {
        gpui_component::theme::ThemeMode::Dark
    } else {
        gpui_component::theme::ThemeMode::Light
    };
    theme.colors.foreground = t.ink.into();
    theme.colors.muted_foreground = t.faint.into();
    theme.colors.background = t.surface.into();
    theme.colors.border = t.line.into();
    theme.colors.primary = t.cyan.into();
    theme.colors.accent = t.cyan_soft.into();
    theme.colors.ring = t.cyan.into();
    theme.colors.caret = t.cyan_deep.into();
    theme.colors.selection = t.sel_line.into();
    theme.font_family = theme::FONT_BODY.into();
    theme.font_size = gpui::px(13.);
}
