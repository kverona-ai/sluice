//! Design tokens from the prototype's `THEMES` (light / dark). Values are the
//! Broadsheet palette: neutral ramp on warm grey, cyan #0088b0 and magenta
//! #d6006c as the two accents, process-yellow #edbb00 as the third lane ink.

use gpui::{Rgba, rgb, rgba};
use sluice_core::Agent;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub is_dark: bool,
    pub desk: Rgba,
    pub paper: Rgba,
    pub surface: Rgba,
    pub panel: Rgba,
    pub ink: Rgba,
    pub muted: Rgba,
    pub faint: Rgba,
    pub line: Rgba,
    pub line_soft: Rgba,
    pub chrome: Rgba,
    pub sel: Rgba,
    pub sel_line: Rgba,
    pub cyan: Rgba,
    pub cyan_deep: Rgba,
    pub cyan_soft: Rgba,
    pub mag: Rgba,
    pub mag_deep: Rgba,
    pub mag_soft: Rgba,
    pub add_bg: Rgba,
    pub add_mark: Rgba,
    pub del_bg: Rgba,
    pub del_mark: Rgba,
    pub yellow: Rgba,
    /// ink @ 5% — sidebar / rail wash on macOS
    pub ink_05: Rgba,
    /// ink @ 8% — segmented-control track
    pub ink_08: Rgba,
    /// ink @ 13% — the "desk" behind the window frame
    pub ink_13: Rgba,
    /// cyan @ 16% — selected ref row
    pub cyan_16: Rgba,
    /// line @ 55% — hairline separators on the mac chrome
    pub line_55: Rgba,
}

impl Theme {
    pub fn light() -> Self {
        Theme {
            is_dark: false,
            desk: rgb(0xe2e0e0),
            paper: rgb(0xf3f2f2),
            surface: rgb(0xffffff),
            panel: rgb(0xf8f4f4),
            ink: rgb(0x201e1d),
            muted: rgb(0x605d5d),
            faint: rgb(0x9b9797),
            line: rgb(0xd7d3d3),
            line_soft: rgb(0xeae7e7),
            chrome: rgb(0xeae7e7),
            sel: rgb(0xe9f8ff),
            sel_line: rgb(0x99e0ff),
            cyan: rgb(0x0088b0),
            cyan_deep: rgb(0x006786),
            cyan_soft: rgb(0xe9f8ff),
            mag: rgb(0xd6006c),
            mag_deep: rgb(0xaa0b56),
            mag_soft: rgb(0xfff1f4),
            add_bg: rgb(0xe9f8ff),
            add_mark: rgb(0x006786),
            del_bg: rgb(0xfff1f4),
            del_mark: rgb(0xaa0b56),
            yellow: rgb(0xedbb00),
            ink_05: rgba(0x201e1d0d),
            ink_08: rgba(0x201e1d14),
            ink_13: rgba(0x201e1d21),
            cyan_16: rgba(0x0088b029),
            line_55: rgba(0xd7d3d38c),
        }
    }

    pub fn dark() -> Self {
        Theme {
            is_dark: true,
            desk: rgb(0x141313),
            paper: rgb(0x201e1d),
            surface: rgb(0x2d2b2b),
            panel: rgb(0x262423),
            ink: rgb(0xf3f2f2),
            muted: rgb(0xbab6b6),
            faint: rgb(0x7d7979),
            line: rgb(0x444141),
            line_soft: rgb(0x332f2f),
            chrome: rgb(0x2d2b2b),
            sel: rgb(0x0a303e),
            sel_line: rgb(0x006786),
            cyan: rgb(0x62c5ee),
            cyan_deep: rgb(0x99e0ff),
            cyan_soft: rgb(0x0a303e),
            mag: rgb(0xff90b1),
            mag_deep: rgb(0xffc0d0),
            mag_soft: rgb(0x4b1528),
            add_bg: rgb(0x0a303e),
            add_mark: rgb(0x99e0ff),
            del_bg: rgb(0x4b1528),
            del_mark: rgb(0xffc0d0),
            yellow: rgb(0xedbb00),
            ink_05: rgba(0xf3f2f20d),
            ink_08: rgba(0xf3f2f214),
            ink_13: rgba(0xf3f2f221),
            cyan_16: rgba(0x62c5ee29),
            line_55: rgba(0x4441418c),
        }
    }

    /// Lane ink by palette index (sluice-graph::PALETTE == 3).
    pub fn lane(&self, ix: u16) -> Rgba {
        match ix % 3 {
            0 => self.cyan,
            1 => self.mag,
            _ => self.yellow,
        }
    }

    /// Agent badge tone (prototype: 人 ink · C magenta · X cyan · G yellow · D cyan).
    pub fn agent_tone(&self, agent: Agent) -> Rgba {
        match agent {
            Agent::Human => self.ink,
            Agent::ClaudeCode => self.mag,
            Agent::CodexCli => self.cyan,
            Agent::GrokBuild => self.yellow,
            Agent::DeepSeekHarness => self.cyan,
            Agent::OtherAi => self.muted,
        }
    }
}

/// Font families. Source Serif 4 is embedded (assets/fonts, OFL); the mono
/// family is the platform default until a bundled mono font is decided (05 §5.6 token list).
pub const FONT_BODY: &str = "Source Serif 4";
pub const FONT_HEADING: &str = "Source Serif 4";
#[cfg(target_os = "macos")]
pub const FONT_MONO: &str = "Menlo";
#[cfg(target_os = "windows")]
pub const FONT_MONO: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const FONT_MONO: &str = "DejaVu Sans Mono";
