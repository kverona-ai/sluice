//! Phosphor icons rendered through gpui's svg element (tinted by `text_color`).
//! Duotone for the large rail glyphs; bold / fill weights for small chrome
//! (13–16px) where the regular strokes were too thin to read.

use gpui::{Pixels, Rgba, Styled, Svg, px, svg};

pub fn icon(name: &'static str, size: Pixels, color: Rgba) -> Svg {
    svg()
        .path(format!("icons/{name}.svg"))
        .w(size)
        .h(size)
        .text_color(color)
        .flex_none()
}

pub fn icon16(name: &'static str, color: Rgba) -> Svg {
    icon(name, px(16.), color)
}

/// Bold weight — small UI chrome.
pub fn icon_b(name: &'static str, size: Pixels, color: Rgba) -> Svg {
    svg()
        .path(format!("icons/bold/{name}.svg"))
        .w(size)
        .h(size)
        .text_color(color)
        .flex_none()
}

/// Fill weight — tags, stars, folders.
pub fn icon_f(name: &'static str, size: Pixels, color: Rgba) -> Svg {
    svg()
        .path(format!("icons/fill/{name}.svg"))
        .w(size)
        .h(size)
        .text_color(color)
        .flex_none()
}
