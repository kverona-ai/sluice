//! sluice-ui — GPUI views. The only layer allowed to depend on gpui (02 §1).
//! Visual language follows the Claude Design prototype (`SluiceDesktop.dc.html`,
//! Broadsheet design system: Source Serif 4, cyan / magenta / process-yellow inks).

pub mod ai;
pub mod assets;
pub mod changes;
pub mod conflict;
pub mod console;
pub mod diff_view;
pub mod file_view;
pub mod icons;
pub mod log;
pub mod overlays;
pub mod proposals;
pub mod rebase;
pub mod recent;
pub mod theme;
pub mod workbench;

use gpui::{App, KeyBinding};

pub use theme::Theme;
pub use workbench::Workbench;

/// Fonts, gpui-component, key bindings. Call once from the application entry point.
pub fn init(cx: &mut App) {
    assets::load_fonts(cx);
    gpui_component::init(cx);
    sync_component_theme(cx, &Theme::light());
    let ctx = Some("Workbench");
    // Single keys must not fire while a text input has focus (05 §11): `!Input` scans the whole context stack.
    let nav = Some("Workbench && !Input");
    cx.bind_keys([
        KeyBinding::new("up", workbench::MoveUp, nav),
        KeyBinding::new("down", workbench::MoveDown, nav),
        KeyBinding::new("k", workbench::MoveUp, nav),
        KeyBinding::new("j", workbench::MoveDown, nav),
        KeyBinding::new("home", workbench::SelectFirst, nav),
        KeyBinding::new("end", workbench::SelectLast, nav),
        KeyBinding::new("pageup", workbench::PageUp, nav),
        KeyBinding::new("pagedown", workbench::PageDown, nav),
        KeyBinding::new("escape", workbench::Escape, ctx),
        KeyBinding::new("space", workbench::ToggleSelected, nav),
        KeyBinding::new("f7", workbench::NextHunk, nav),
        KeyBinding::new("shift-f7", workbench::PrevHunk, nav),
        // IDEA preset (05 §11)
        KeyBinding::new("cmd-9", workbench::ShowLog, ctx),
        KeyBinding::new("cmd-0", workbench::ShowChanges, ctx),
        KeyBinding::new("cmd-6", workbench::ShowConsole, ctx),
        KeyBinding::new("cmd-alt-y", workbench::Refresh, ctx),
        KeyBinding::new("ctrl-alt-y", workbench::Refresh, ctx),
        KeyBinding::new("cmd-f", workbench::FocusSearch, ctx),
        KeyBinding::new("ctrl-f", workbench::FocusSearch, ctx),
        KeyBinding::new("cmd-k", workbench::FocusCommit, ctx),
        KeyBinding::new("ctrl-k", workbench::FocusCommit, ctx),
        KeyBinding::new("cmd-enter", workbench::CommitAction, ctx),
        KeyBinding::new("ctrl-enter", workbench::CommitAction, ctx),
        KeyBinding::new("cmd-alt-a", workbench::StageAll, ctx),
        KeyBinding::new("ctrl-alt-a", workbench::StageAll, ctx),
        KeyBinding::new("cmd-alt-u", workbench::UnstageAll, ctx),
        KeyBinding::new("ctrl-alt-u", workbench::UnstageAll, ctx),
        KeyBinding::new("cmd-alt-\\", workbench::ToggleSideBySide, ctx),
        KeyBinding::new("ctrl-alt-\\", workbench::ToggleSideBySide, ctx),
        // Windows / Linux: the same bindings with ctrl
        KeyBinding::new("ctrl-shift-`", workbench::OpenBranches, ctx),
        KeyBinding::new("ctrl-~", workbench::OpenBranches, ctx),
        KeyBinding::new("cmd-b", workbench::OpenBranches, ctx),
        KeyBinding::new("ctrl-b", workbench::OpenBranches, ctx),
        KeyBinding::new("cmd-5", workbench::OpenStash, ctx),
        KeyBinding::new("ctrl-5", workbench::OpenStash, ctx),
        KeyBinding::new("cmd-7", workbench::OpenSnapshots, ctx),
        KeyBinding::new("ctrl-7", workbench::OpenSnapshots, ctx),
        KeyBinding::new("cmd-,", workbench::OpenSettings, ctx),
        KeyBinding::new("cmd-shift-k", workbench::OpenPush, ctx),
        KeyBinding::new("cmd-shift-t", workbench::ToggleTheme, ctx),
        KeyBinding::new("cmd-u", workbench::OpenUserFilter, ctx),
        KeyBinding::new("cmd-shift-h", workbench::OpenFileHistory, ctx),
        KeyBinding::new("cmd-shift-i", workbench::OpenAiConnect, ctx),
        KeyBinding::new("cmd-shift-p", workbench::OpenProposals, ctx),
        KeyBinding::new("cmd-o", workbench::OpenRepository, ctx),
        KeyBinding::new("ctrl-o", workbench::OpenRepository, ctx),
        KeyBinding::new("cmd-shift-o", workbench::OpenRecent, ctx),
        KeyBinding::new("ctrl-shift-o", workbench::OpenRecent, ctx),
        KeyBinding::new("enter", workbench::ProposalAccept, nav),
        KeyBinding::new("alt-up", workbench::RebaseMoveUp, nav),
        KeyBinding::new("alt-1", workbench::ConflictOurs, nav),
        KeyBinding::new("alt-2", workbench::ConflictTheirs, nav),
        KeyBinding::new("alt-3", workbench::ConflictBoth, nav),
        KeyBinding::new("cmd-s", workbench::ConflictResolve, ctx),
        KeyBinding::new("ctrl-s", workbench::ConflictResolve, ctx),
        KeyBinding::new("cmd-alt-r", workbench::RebaseFromSelection, ctx),
        KeyBinding::new("ctrl-alt-r", workbench::RebaseFromSelection, ctx),
        KeyBinding::new("alt-down", workbench::RebaseMoveDown, nav),
        KeyBinding::new("delete", workbench::ProposalReject, nav),
        KeyBinding::new("backspace", workbench::ProposalReject, nav),
        KeyBinding::new("ctrl-shift-p", workbench::OpenProposals, ctx),
        KeyBinding::new("ctrl-shift-i", workbench::OpenAiConnect, ctx),
        KeyBinding::new("ctrl-shift-h", workbench::OpenFileHistory, ctx),
        KeyBinding::new("cmd-alt-b", workbench::OpenBlame, ctx),
        KeyBinding::new("ctrl-alt-b", workbench::OpenBlame, ctx),
        KeyBinding::new("ctrl-u", workbench::OpenUserFilter, ctx),
        KeyBinding::new("cmd-alt-d", workbench::OpenDateFilter, ctx),
        KeyBinding::new("cmd-alt-p", workbench::OpenPathFilter, ctx),
        KeyBinding::new("ctrl-alt-p", workbench::OpenPathFilter, ctx),
        KeyBinding::new("ctrl-alt-d", workbench::OpenDateFilter, ctx),
        KeyBinding::new("ctrl-shift-t", workbench::ToggleTheme, ctx),
        KeyBinding::new("ctrl-shift-k", workbench::OpenPush, ctx),
        KeyBinding::new("alt-9", workbench::ShowLog, ctx),
        KeyBinding::new("alt-0", workbench::ShowChanges, ctx),
        KeyBinding::new("alt-6", workbench::ShowConsole, ctx),
    ]);
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
