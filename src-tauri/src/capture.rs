// Kudu Desktop - Screen Capture Module
// Captures screen regions using the `screenshots` crate for cross-platform support.
//
// Coordinate contract: all public functions here take GLOBAL (virtual desktop)
// coordinates - the same space GetCursorPos reports. The screenshots crate's
// capture_area() wants coordinates RELATIVE to a specific screen, so every
// capture first resolves which monitor the point is on and converts.

use base64::{engine::general_purpose::STANDARD, Engine};
use image::codecs::jpeg::JpegEncoder;
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
/// sent to an LLM and discarded, never saved - a larger file is a fine trade
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

// Longest side an outgoing screenshot is allowed to have - a small crop
// around the cursor often misses the very content the user is asking about
// (a whole File Explorer pane, a full browser tab); capturing the entire
// window instead gives the model far more to work with. Downscaling keeps
// the base64 payload and per-request image cost sane on 4K/ultrawide rigs.
const MAX_CAPTURE_SIDE: u32 = 1600;

/// Capture the whole top-level window under the given point so the model
/// can see everything relevant, not just a small crop - falls back to the
/// radius-based crop if no window rect can be resolved (non-Windows, an
/// unusual/minimized window, etc). Resizes BEFORE encoding (not after) so a
/// large 4K window doesn't pay for a full-resolution PNG encode it's about
/// to throw away.
pub fn capture_window_or_around_point(cx: i32, cy: i32, fallback_radius: u32) -> Result<String, String> {
    capture_target_or_around_point(0, cx, cy, fallback_radius)
}

/// Like `capture_window_or_around_point`, but for a remembered window handle
/// first: the window itself is rendered (PrintWindow), so the always-on-top Ask Kudu
/// panel sitting over it never ends up in the picture. 0 = no handle, use the point.
pub fn capture_target_or_around_point(target: isize, cx: i32, cy: i32, fallback_radius: u32) -> Result<String, String> {
    if target != 0 {
        if let Ok(img) = capture_hwnd_raw(target) {
            return encode_png_fast(&maybe_downscale(img, MAX_CAPTURE_SIDE));
        }
        if let Some((x, y, w, h)) = crate::mouse::window_rect_of(target) {
            if let Ok(img) = capture_region_raw(x, y, w, h) {
                return encode_png_fast(&maybe_downscale(img, MAX_CAPTURE_SIDE));
            }
        }
    }
    // The point: never our own overlay (it floats over whatever the user meant).
    let under = crate::mouse::target_hwnd_at_point(cx, cy);
    if under != 0 {
        if let Ok(img) = capture_hwnd_raw(under) {
            return encode_png_fast(&maybe_downscale(img, MAX_CAPTURE_SIDE));
        }
    }
    if let Some((x, y, w, h)) = crate::explorer::window_rect_at(cx, cy) {
        if let Ok(img) = capture_region_raw(x, y, w, h) {
            return encode_png_fast(&maybe_downscale(img, MAX_CAPTURE_SIDE));
        }
    }
    capture_around_point(cx, cy, fallback_radius)
}

/// Render one top-level window into a bitmap (PrintWindow with
/// PW_RENDERFULLCONTENT, which GPU-composited apps such as browsers honour).
/// Fails on a minimized window or a blank render so the caller can fall back.
#[cfg(windows)]
pub fn capture_hwnd_raw(hwnd_raw: isize) -> Result<RgbaImage, String> {
    use winapi::ctypes::c_void;
    use winapi::shared::windef::{HWND, RECT};
    use winapi::um::wingdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS,
    };
    use winapi::um::winuser::{GetDC, GetWindowRect, IsIconic, IsWindow, PrintWindow, ReleaseDC};
    const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;

    let hwnd = hwnd_raw as HWND;
    unsafe {
        if hwnd.is_null() || IsWindow(hwnd) == 0 {
            return Err("window is gone".to_string());
        }
        if IsIconic(hwnd) != 0 {
            return Err("window is minimized".to_string());
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return Err("no window rect".to_string());
        }
        let w = (rect.right - rect.left).max(1);
        let h = (rect.bottom - rect.top).max(1);
        let screen_dc = GetDC(std::ptr::null_mut());
        let mem_dc = CreateCompatibleDC(screen_dc);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(screen_dc, &bmi, DIB_RGB_COLORS, &mut bits as *mut *mut c_void, std::ptr::null_mut(), 0);
        if bmp.is_null() || bits.is_null() {
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            return Err("could not allocate a bitmap".to_string());
        }
        let old = SelectObject(mem_dc, bmp as _);
        let ok = PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT);
        let len = (w as usize) * (h as usize) * 4;
        let src = std::slice::from_raw_parts(bits as *const u8, len);
        let mut rgba = Vec::with_capacity(len);
        let mut dark = 0usize;
        for px in src.chunks_exact(4) {
            if px[0] < 8 && px[1] < 8 && px[2] < 8 {
                dark += 1;
            }
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        SelectObject(mem_dc, old);
        DeleteObject(bmp as _);
        DeleteDC(mem_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        if ok == 0 {
            return Err("PrintWindow failed".to_string());
        }
        // A blank render (some protected or GPU-only surfaces) is worse than a
        // screen crop: let the caller fall back.
        if dark * 100 / (len / 4).max(1) > 98 {
            return Err("blank render".to_string());
        }
        RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or_else(|| "bad buffer".to_string())
    }
}
#[cfg(not(windows))]
pub fn capture_hwnd_raw(_hwnd_raw: isize) -> Result<RgbaImage, String> {
    Err("window capture by handle is Windows-only".to_string())
}

/// Never upscales. `Triangle` (bilinear-ish) trades a little sharpness for
/// being far cheaper than `Lanczos3` on a large screenshot - plenty good
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

// ── Agent screenshots (small JPEG, coordinate-mapped) ────────────
// The agent re-sends its ENTIRE conversation each step, and that history can
// hold several screenshots. A full-res PNG is megabytes; a few of them blow
// past the backend's request-body limit (HTTP 413). So agent captures are
// downscaled hard and encoded as JPEG - an order of magnitude smaller, still
// perfectly legible for the model.
//
// Critically, the model only ever SEES the downscaled image, but clicks must
// land in REAL screen pixels. So every agent screenshot also reports the
// captured region's origin (global screen coords) and native (pre-downscale)
// size - the agent loop uses this to convert the model's normalized 0-1000
// click coordinates back to exact screen pixels. Without this, a click is off
// by the downscale ratio (e.g. 1.5x on a 1920-wide screen shown at 1280) and
// lands on the wrong element entirely.
const AGENT_MAX_SIDE: u32 = 1280;
const AGENT_JPEG_QUALITY: u8 = 68;

/// A screenshot handed to the model, plus the mapping needed to convert its
/// normalized-0-1000 click coordinates back to real screen pixels.
pub struct AgentShot {
    pub data_url: String,
    /// Top-left of the captured region, in global (virtual-desktop) pixels.
    pub origin_x: i32,
    pub origin_y: i32,
    /// Size of the captured region BEFORE downscaling - i.e. the real pixel
    /// span that the image's normalized 0-1000 coordinate space maps onto.
    pub native_width: u32,
    pub native_height: u32,
}

fn encode_jpeg_downscaled(img: RgbaImage, max_side: u32, quality: u8) -> Result<String, String> {
    let img = maybe_downscale(img, max_side);
    // JPEG has no alpha channel - flatten RGBA → RGB first.
    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality)
        .encode(rgb.as_raw(), rgb.width(), rgb.height(), ExtendedColorType::Rgb8)
        .map_err(|e| format!("Failed to encode JPEG: {}", e))?;
    Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(buf)))
}

/// Full screen under the cursor, as a small JPEG for the agent's capture_screen.
pub fn capture_agent_full() -> Result<AgentShot, String> {
    let (cx, cy) = crate::mouse::get_cursor_position();
    let screen = screen_at(cx, cy)?;
    let info = screen.display_info;
    let image = screen
        .capture()
        .map_err(|e| format!("Failed to capture screen: {}", e))?;
    let img = to_rgba_image(image)?;
    let (native_width, native_height) = (img.width(), img.height());
    let data_url = encode_jpeg_downscaled(img, AGENT_MAX_SIDE, AGENT_JPEG_QUALITY)?;
    Ok(AgentShot { data_url, origin_x: info.x, origin_y: info.y, native_width, native_height })
}

/// The meeting copilot reads code, slides and quiz text off the screen, so it
/// gets a sharper shot than the agent's click-targeting one - small text at
/// 1280 px is where a model starts misreading `l` vs `1`.
const MEETING_MAX_SIDE: u32 = 1920;
const MEETING_JPEG_QUALITY: u8 = 82;
const THUMB_MAX_SIDE: u32 = 360;

/// Screen under the cursor for the meeting copilot: (model-quality base64
/// JPEG without the data-URL prefix, small thumbnail data URL for the UI).
/// Kudu Desktop's own windows are content-protected, so they never appear in it.
pub fn capture_meeting_screen() -> Result<(String, String), String> {
    let (cx, cy) = crate::mouse::get_cursor_position();
    let image = screen_at(cx, cy)?
        .capture()
        .map_err(|e| format!("Failed to capture screen: {}", e))?;
    let img = to_rgba_image(image)?;
    let thumb = encode_jpeg_downscaled(img.clone(), THUMB_MAX_SIDE, 70)?;
    let full = encode_jpeg_downscaled(img, MEETING_MAX_SIDE, MEETING_JPEG_QUALITY)?;
    let b64 = full.strip_prefix("data:image/jpeg;base64,").unwrap_or(&full).to_string();
    Ok((b64, thumb))
}

// ── Adaptive glass theming ────────────────────────────────────────
// Mirrors the browser extension's shared/adaptive-theme.js (samples the page
// behind the panel to decide light-glass-vs-dark-glass), but there's no DOM to
// read behind a native overlay window - real screen pixels are sampled
// instead, at a coarse stride since a rough average is all a two-way light/
// dark decision needs.

fn srgb_channel_linear(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Relative luminance per the same WCAG formula the extension's
/// adaptive-theme.js uses, so the two surfaces feel consistent: 0.0 = black,
/// 1.0 = white.
fn srgb_luminance(r: u8, g: u8, b: u8) -> f64 {
    0.2126 * srgb_channel_linear(r) + 0.7152 * srgb_channel_linear(g) + 0.0722 * srgb_channel_linear(b)
}

/// Average luminance of a screen region, sampled on a coarse grid (not every
/// pixel - a rough average is all a light-vs-dark decision needs, and this
/// keeps it cheap enough to call on every overlay reposition). Returns `None`
/// if the region can't be captured (e.g. entirely off-screen).
pub fn average_luminance_at(x: i32, y: i32, width: u32, height: u32) -> Option<f32> {
    let img = capture_region_raw(x, y, width.max(1), height.max(1)).ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    const GRID: u32 = 20;
    let stride_x = (w / GRID).max(1);
    let stride_y = (h / GRID).max(1);

    let mut sum = 0.0f64;
    let mut count = 0u64;
    let mut px = 0u32;
    while px < w {
        let mut py = 0u32;
        while py < h {
            let p = img.get_pixel(px, py);
            sum += srgb_luminance(p[0], p[1], p[2]);
            count += 1;
            py += stride_y;
        }
        px += stride_x;
    }
    if count == 0 {
        return None;
    }
    Some((sum / count as f64) as f32)
}

/// The window under the cursor (or a crop fallback), as a small JPEG - the
/// agent's initial grounding screenshot.
/// The agent's grounding shot of a remembered target window (0 = the window
/// under the point, excluding our own overlay).
pub fn capture_agent_window_for(target: isize, cx: i32, cy: i32) -> Result<AgentShot, String> {
    let target = if target != 0 { target } else { crate::mouse::target_hwnd_at_point(cx, cy) };
    if target != 0 {
        if let (Ok(img), Some((x, y, w, h))) = (capture_hwnd_raw(target), crate::mouse::window_rect_of(target)) {
            let data_url = encode_jpeg_downscaled(img, AGENT_MAX_SIDE, AGENT_JPEG_QUALITY)?;
            return Ok(AgentShot { data_url, origin_x: x, origin_y: y, native_width: w, native_height: h });
        }
    }
    if let Some((x, y, w, h)) = crate::explorer::window_rect_at(cx, cy) {
        if let Ok(img) = capture_region_raw(x, y, w, h) {
            let data_url = encode_jpeg_downscaled(img, AGENT_MAX_SIDE, AGENT_JPEG_QUALITY)?;
            return Ok(AgentShot { data_url, origin_x: x, origin_y: y, native_width: w, native_height: h });
        }
    }
    let r = 450i32;
    let (x, y, w, h) = (cx - r, cy - r, 900u32, 900u32);
    let img = capture_region_raw(x, y, w, h)?;
    let data_url = encode_jpeg_downscaled(img, AGENT_MAX_SIDE, AGENT_JPEG_QUALITY)?;
    Ok(AgentShot { data_url, origin_x: x, origin_y: y, native_width: w, native_height: h })
}
