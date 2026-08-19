//! Phosphor duotone icons rendered through gpui's svg element (tinted by `text_color`).

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
