// Mouse AI desktop agent - screenshots, coordinate mapping, verification shots.
// Split out of the former single agent.rs; see agent/mod.rs for the module map.

#[allow(unused_imports)]
use crate::agent::*;

// ── Screen coordinate mapping ────────────────────────────────────
// The model only ever sees a DOWNSCALED screenshot (kept small so the agent's
// growing conversation history doesn't blow past the backend's request-size
// limit - see capture.rs), but clicks must land on real screen pixels. So the
// model is told to report click coordinates NORMALIZED to a 0-1000 scale
// relative to the screenshot it was just shown, and this maps that back to
// exact global pixels using the captured region's real origin/size. Skipping
// this (i.e. treating the model's numbers as raw screen pixels) is what makes
// clicks land on the wrong element - off by exactly the downscale ratio.
#[derive(Clone, Copy)]
pub(crate) struct ScreenMapping {
    pub(crate) origin_x: i32,
    pub(crate) origin_y: i32,
    pub(crate) native_width: u32,
    pub(crate) native_height: u32,
}

impl ScreenMapping {
    pub(crate) fn from_shot(shot: &capture::AgentShot) -> Self {
        Self {
            origin_x: shot.origin_x,
            origin_y: shot.origin_y,
            native_width: shot.native_width,
            native_height: shot.native_height,
        }
    }

    pub(crate) fn to_real(&self, nx: f64, ny: f64) -> (i32, i32) {
        let nx = nx.clamp(0.0, 1000.0);
        let ny = ny.clamp(0.0, 1000.0);
        let rx = self.origin_x + ((nx / 1000.0) * self.native_width as f64).round() as i32;
        let ry = self.origin_y + ((ny / 1000.0) * self.native_height as f64).round() as i32;
        (rx, ry)
    }
}

/// Keep only the most recent `keep` screenshots in the conversation, replacing
/// older ones with a short text note. The agent re-sends the whole history each
/// step, so without this the accumulated images quickly exceed the backend's
/// request-body limit (HTTP 413) on any multi-screenshot task.
pub(crate) fn prune_old_images(contents: &mut [Value], keep: usize) {
    let mut positions: Vec<(usize, usize)> = Vec::new();
    for (mi, msg) in contents.iter().enumerate() {
        if let Some(parts) = msg.get("parts").and_then(|p| p.as_array()) {
            for (pi, part) in parts.iter().enumerate() {
                if part.get("inlineData").is_some() {
                    positions.push((mi, pi));
                }
            }
        }
    }
    if positions.len() <= keep {
        return;
    }
    let drop_count = positions.len() - keep;
    for &(mi, pi) in positions.iter().take(drop_count) {
        if let Some(parts) = contents[mi].get_mut("parts").and_then(|p| p.as_array_mut()) {
            parts[pi] = json!({ "text": "[earlier screenshot omitted to keep the request small]" });
        }
    }
}

pub(crate) fn image_part(data_url: &str) -> Option<Value> {
    let (mime, b64) = if let Some(rest) = data_url.strip_prefix("data:image/png;base64,") {
        ("image/png", rest)
    } else if let Some(rest) = data_url.strip_prefix("data:image/jpeg;base64,") {
        ("image/jpeg", rest)
    } else {
        return None;
    };
    Some(json!({ "inlineData": { "mimeType": mime, "data": b64 } }))
}

/// A tiny grayscale thumbnail of a screenshot - enough to tell "the screen
/// changed" from "nothing happened" without keeping the image around.
pub(crate) const THUMB_W: u32 = 48;

pub(crate) const THUMB_H: u32 = 32;

pub(crate) fn screen_thumbnail(data_url: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let b64 = data_url.split_once(',')?.1;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let thumb = img.resize_exact(THUMB_W, THUMB_H, image::imageops::FilterType::Triangle).to_luma8();
    Some(thumb.into_raw())
}

/// Clock ticks and a blinking caret touch a cell or two; real changes touch more.
pub(crate) fn screens_differ(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return true;
    }
    a.iter().zip(b).filter(|(x, y)| x.abs_diff(**y) > 8).count() >= 3
}

/// Settle delay before the verification shot: instant for input synthesis,
/// longer for actions that open/switch windows or load pages.
pub(crate) fn post_action_delay_ms(tool_name: &str) -> Option<u64> {
    match tool_name {
        "click" | "double_click" | "scroll" | "type_text" | "press_key" => Some(900),
        "open_url" | "open_app" | "focus_window" => Some(2000),
        // After an explicit wait the whole point is to look again.
        "wait" => Some(0),
        _ => None,
    }
}

/// Capture the screen for verification and refresh the coordinate mapping.
pub(crate) async fn verification_shot(screen: &mut Option<ScreenMapping>, delay_ms: u64) -> Option<String> {
    if delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    let shot = tokio::task::spawn_blocking(capture::capture_agent_full).await.ok()?.ok()?;
    *screen = Some(ScreenMapping::from_shot(&shot));
    Some(shot.data_url)
}

/// Attach an automatic verification screenshot to a UI action's outcome (only
/// when the action itself succeeded and didn't already produce an image).
pub(crate) async fn attach_verification_shot(
    name: &str,
    outcome: &mut ToolOutcome,
    screen: &mut Option<ScreenMapping>,
) {
    if outcome.image_data_url.is_some() || outcome.response.get("error").is_some() {
        return;
    }
    let Some(delay) = post_action_delay_ms(name) else { return };
    if let Some(data_url) = verification_shot(screen, delay).await {
        outcome.image_data_url = Some(data_url);
        if let Some(obj) = outcome.response.as_object_mut() {
            obj.insert(
                "verification".to_string(),
                json!(
                    "A fresh screenshot of the screen AFTER this action is attached. Inspect it to \
confirm the action actually had the intended effect before moving on - if the expected change \
isn't visible, the action did NOT work. Base any next click/scroll coordinates on this screenshot."
                ),
            );
        }
    }
}
