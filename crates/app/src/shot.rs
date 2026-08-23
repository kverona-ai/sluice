//! Self-screenshot for docs / visual checks (macOS): capture *our own* window via
//! CGWindowList (no Screen Recording permission needed for your own windows) and
//! write a PNG. Triggered by `SLUICE_SCREENSHOT=<path>`.

#[cfg(target_os = "macos")]
pub fn capture_own_window(out: &std::path::Path) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::display::{
        CGDisplay, CGPoint, CGRect, CGSize, kCGWindowImageBestResolution, kCGWindowImageBoundsIgnoreFraming,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionIncludingWindow,
        kCGWindowListOptionOnScreenOnly,
    };

    let pid = std::process::id() as i64;
    let list = CGDisplay::window_list_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        None,
    )
    .context("CGWindowListCopyWindowInfo")?;
    let mut window_id: Option<u32> = None;
    let mut best_area = 0.0_f64;
    for i in 0..list.len() {
        let item = list.get(i).context("window info item")?;
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(*item as CFDictionaryRef) };
        let owner = dict
            .find(CFString::new("kCGWindowOwnerPID"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(-1);
        if owner != pid {
            continue;
        }
        let layer = dict
            .find(CFString::new("kCGWindowLayer"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(0);
        if layer != 0 {
            continue;
        }
        let id = dict
            .find(CFString::new("kCGWindowNumber"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(0) as u32;
        // prefer the biggest window (the main one)
        let area = dict
            .find(CFString::new("kCGWindowBounds"))
            .map(|v| {
                // untyped dictionary → wrap by ref
                let b: CFDictionary<CFString, CFType> =
                    unsafe { CFDictionary::wrap_under_get_rule(v.as_CFTypeRef() as CFDictionaryRef) };
                let g = |k: &str| {
                    b.find(CFString::new(k))
                        .and_then(|v| v.downcast::<CFNumber>())
                        .and_then(|n| n.to_f64())
                        .unwrap_or(0.0)
                };
                g("Width") * g("Height")
            })
            .unwrap_or(0.0);
        if area >= best_area {
            best_area = area;
            window_id = Some(id);
        }
    }
    let window_id = window_id.context("no on-screen window for this process")?;
    let null_rect = CGRect::new(
        &CGPoint::new(f64::INFINITY, f64::INFINITY),
        &CGSize::new(0.0, 0.0),
    );
    let image = CGDisplay::screenshot(
        null_rect,
        kCGWindowListOptionIncludingWindow,
        window_id,
        kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution,
    )
    .context("CGWindowListCreateImage returned null")?;
    let (w, h, bpr) = (image.width(), image.height(), image.bytes_per_row());
    let data = image.data();
    let bytes = data.bytes();
    // BGRA premultiplied → RGBA
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &bytes[y * bpr..y * bpr + w * 4];
        for px in row.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(out)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    writer.write_image_data(&rgba)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn capture_own_window(_out: &std::path::Path) -> anyhow::Result<()> {
    anyhow::bail!("self-screenshot is implemented on macOS only")
}
