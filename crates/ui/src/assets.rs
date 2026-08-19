//! Embedded assets: Phosphor duotone icons (MIT) and Source Serif 4 (OFL).

use std::borrow::Cow;

use anyhow::Result;
use gpui::{App, AssetSource, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../assets/icons/"]
#[prefix = "icons/"]
#[include = "*.svg"]
struct Icons;

#[derive(RustEmbed)]
#[folder = "../../assets/fonts/"]
#[include = "*.otf"]
struct Fonts;

/// The `AssetSource` handed to `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Icons::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Icons::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| SharedString::from(p.to_string()))
            .collect())
    }
}

/// Register the embedded font faces with the text system.
pub fn load_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = Fonts::iter()
        .filter_map(|name| Fonts::get(&name).map(|f| f.data))
        .collect();
    if let Err(err) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("could not register embedded fonts: {err:#}");
    }
}
