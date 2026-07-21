// Mouse AI Desktop — Screen Capture Module
// Captures screen regions using the `screenshots` crate for cross-platform support.
//
// Coordinate contract: all public functions here take GLOBAL (virtual desktop)
// coordinates — the same space GetCursorPos reports. The screenshots crate's
// capture_area() wants coordinates RELATIVE to a specific screen, so every
// capture first resolves which monitor the point is on and converts.

use base64::{engine::general_purpose::STANDARD, Engine};
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use screenshots::Screen;

/// Find the screen containing the given global point, falling back to the
/// primary/first screen (e.g. point briefly outside any monitor bounds).
fn screen_at(x: i32, y: i32) -> Result<Screen, String> {
    if let Ok(screen) = Screen::from_point(x, y) {
        return Ok(screen);
    }
    let screens = Screen::all().map_err(|e| format!("Failed to enumerate screens: {}", e))?;
    screens
        .iter()
        .find(|s| s.display_info.is_primary)
        .or_else(|| screens.first())
        .copied()
        .ok_or_else(|| "No screens found".to_string())
}

/// Encodes with `Fast` compression instead of the `image` crate's default
/// (`Balanced`, with adaptive per-scanline filtering). These screenshots are
/// sent to an LLM and discarded, never saved — a larger file is a fine trade
/// for cutting encode time on a large capture from ~seconds to milliseconds.
fn encode_png_fast(img: &RgbaImage) -> Result<String, String> {
    let mut buf = Vec::new();
    PngEncoder::new_with_quality(&mut buf, CompressionType::Fast, PngFilterType::Sub)
        .write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8)
        .map_err(|e| format!("Failed to encode PNG: {}", e))?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(buf)))
}

fn to_rgba_image(image: screenshots::image::RgbaImage) -> Result<RgbaImage, String> {
    RgbaImage::from_raw(image.width(), image.height(), image.into_raw())
        .ok_or_else(|| "Failed to create image buffer".to_string())
}

/// Capture the entire screen under the cursor (falls back to primary) and
/// return as base64 PNG data URL.
pub fn capture_full_screen() -> Result<String, String> {
    let (cx, cy) = crate::mouse::get_cursor_position();
    let screen = screen_at(cx, cy)?;
    let image = screen
        .capture()
        .map_err(|e| format!("Failed to capture screen: {}", e))?;
    encode_png_fast(&to_rgba_image(image)?)
}

/// Capture a specific region of the virtual desktop as a raw RGBA buffer.
/// (x, y) is the global top-left corner, (width, height) the size in pixels.
/// The region is clamped to the monitor that contains its center.
fn capture_region_raw(x: i32, y: i32, width: u32, height: u32) -> Result<RgbaImage, String> {
    let center_x = x + (width / 2) as i32;
    let center_y = y + (height / 2) as i32;
    let screen = screen_at(center_x, center_y)?;
    let info = screen.display_info;

    // Global → screen-relative; capture_area() clamps to the screen bounds,
    // so partially off-screen regions shrink instead of failing.
    let rel_x = x - info.x;
    let rel_y = y - info.y;

    let image = screen
        .capture_area(rel_x, rel_y, width, height)
        .map_err(|e| format!("Failed to capture region: {}", e))?;
    to_rgba_image(image)
}

/// Capture a specific region of the virtual desktop and return as base64 PNG.
pub fn capture_region(x: i32, y: i32, width: u32, height: u32) -> Result<String, String> {
    encode_png_fast(&capture_region_raw(x, y, width, height)?)
}

/// Capture a square region centered around the given global point.
/// `radius` is half the side length in pixels.
pub fn capture_around_point(cx: i32, cy: i32, radius: u32) -> Result<String, String> {
    let r = radius as i32;
    capture_region(cx - r, cy - r, radius * 2, radius * 2)
}

// Longest side an outgoing screenshot is allowed to have — a small crop
// around the cursor often misses the very content the user is asking about
// (a whole File Explorer pane, a full browser tab); capturing the entire
// window instead gives the model far more to work with. Downscaling keeps
// the base64 payload and per-request image cost sane on 4K/ultrawide rigs.
const MAX_CAPTURE_SIDE: u32 = 1600;

/// Capture the whole top-level window under the given point so the model
/// can see everything relevant, not just a small crop — falls back to the
/// radius-based crop if no window rect can be resolved (non-Windows, an
/// unusual/minimized window, etc). Resizes BEFORE encoding (not after) so a
/// large 4K window doesn't pay for a full-resolution PNG encode it's about
/// to throw away.
pub fn capture_window_or_around_point(cx: i32, cy: i32, fallback_radius: u32) -> Result<String, String> {
    if let Some((x, y, w, h)) = crate::explorer::window_rect_at(cx, cy) {
        if let Ok(img) = capture_region_raw(x, y, w, h) {
            return encode_png_fast(&maybe_downscale(img, MAX_CAPTURE_SIDE));
        }
    }
    capture_around_point(cx, cy, fallback_radius)
}

/// Never upscales. `Triangle` (bilinear-ish) trades a little sharpness for
/// being far cheaper than `Lanczos3` on a large screenshot — plenty good
/// enough for a model reading text/UI, not a saved photo.
fn maybe_downscale(img: RgbaImage, max_side: u32) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w <= max_side && h <= max_side {
        return img;
    }
    let scale = max_side as f32 / w.max(h) as f32;
    let new_w = ((w as f32 * scale).round() as u32).max(1);
    let new_h = ((h as f32 * scale).round() as u32).max(1);
    image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Triangle)
}
