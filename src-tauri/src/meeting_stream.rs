// Mouse AI Desktop - live (streaming) transcription for the meeting copilot.
//
// The chunked path in meeting.rs cuts a clip at every pause and sends it to Whisper,
// so the first word of a sentence cannot appear until the speaker stops. This module
// streams the same two audio sources (mic = "me", system loopback = "them") to a
// streaming STT provider over a WebSocket and turns its interim results into live
// partial lines (`meeting-transcript-partial`) and its finalized utterances into the
// normal transcript entries (`meeting-transcript`, via meeting::push_entry).
//
// The provider key never reaches the desktop: the backend hands out a short-lived token
// (/api/meeting/stt-token, 503 when no provider is configured) and the desktop connects
// to the provider directly - Vercel functions cannot hold a WebSocket open. When the
// token is unavailable or the socket drops twice, the `streaming` flag goes false and the
// chunker in meeting.rs takes over with Whisper, so a session never goes silent.
//
// Provider: Deepgram (nova-3). Two channels are sent interleaved as linear16 with
// `multichannel=true`, so each result carries the channel it came from - no echo
// filtering needed, and "them" vs "me" is exact.

use crate::backend::BackendClient;
use crate::meeting::{push_entry, resample_linear, SharedBuf, TARGET_RATE};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SttToken {
    pub provider: String,
    pub token: String,
    pub model: String,
}

/// What each side has said that is not yet a finished line - shown as a live partial
/// and included in assist requests so an answer can start before the sentence ends.
static PARTIALS: Mutex<[String; 2]> = Mutex::new([String::new(), String::new()]);

pub(crate) fn live_partials() -> Vec<(&'static str, String)> {
    let p = PARTIALS.lock().unwrap();
    let mut out = Vec::new();
    if !p[0].trim().is_empty() {
        out.push(("me", p[0].clone()));
    }
    if !p[1].trim().is_empty() {
        out.push(("them", p[1].clone()));
    }
    out
}

fn set_partial(app: &AppHandle, speaker: &str, text: String) {
    {
        let mut p = PARTIALS.lock().unwrap();
        let slot = if speaker == "me" { 0 } else { 1 };
        p[slot] = text.clone();
    }
    let _ = app.emit("meeting-transcript-partial", json!({ "speaker": speaker, "text": text }));
}

fn clear_partials(app: &AppHandle) {
    set_partial(app, "me", String::new());
    set_partial(app, "them", String::new());
}

enum Ended {
    Stopped,
    Dropped,
}

/// Runs for the whole session: connects (re-fetching a token per attempt), streams,
/// and hands back to the chunker if the stream cannot be kept up.
pub(crate) async fn run_live_stream(
    app: AppHandle,
    backend: BackendClient,
    mic: Option<SharedBuf>,
    system: Option<SharedBuf>,
    stop: Arc<AtomicBool>,
    streaming: Arc<AtomicBool>,
) {
    let mut drops = 0u32;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let token = match backend.stt_token().await {
            Ok(Some(t)) => t,
            Ok(None) => {
                // Not configured on this backend - chunked Whisper it is.
                break;
            }
            Err(e) => {
                eprintln!("[meeting] live transcription token failed: {}", e);
                break;
            }
        };
        match stream_once(&app, &token, &mic, &system, &stop, &streaming).await {
            Ok(Ended::Stopped) => break,
            Ok(Ended::Dropped) | Err(_) => {
                drops += 1;
                streaming.store(false, Ordering::Relaxed);
                if drops > 2 || stop.load(Ordering::Relaxed) {
                    if !stop.load(Ordering::Relaxed) {
                        let _ = app.emit(
                            "meeting-status",
                            crate::meeting::current_status(Some(
                                "Live transcription dropped - continuing with chunked transcription (a few seconds behind).".to_string(),
                            )),
                        );
                    }
                    break;
                }
                tokio::time::sleep(Duration::from_millis(800)).await;
            }
        }
    }
    streaming.store(false, Ordering::Relaxed);
    clear_partials(&app);
}

fn take_resampled(buf: &Option<SharedBuf>) -> Vec<f32> {
    let Some(buf) = buf else { return Vec::new() };
    let (samples, rate) = {
        let mut b = buf.lock().unwrap();
        (std::mem::take(&mut b.samples), b.rate)
    };
    if samples.is_empty() || rate == 0 {
        return Vec::new();
    }
    resample_linear(&samples, rate, TARGET_RATE)
}

fn push_i16(out: &mut Vec<u8>, s: Option<&f32>) {
    let v = (s.copied().unwrap_or(0.0).clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
    out.extend_from_slice(&v.to_le_bytes());
}

/// linear16 frames: [me, them] interleaved when both sources exist, else the one source.
fn pcm_frames(me: Vec<f32>, them: Vec<f32>, both: bool) -> Vec<u8> {
    if both {
        let n = me.len().max(them.len());
        let mut out = Vec::with_capacity(n * 4);
        for i in 0..n {
            push_i16(&mut out, me.get(i));
            push_i16(&mut out, them.get(i));
        }
        out
    } else {
        let src = if me.is_empty() { them } else { me };
        let mut out = Vec::with_capacity(src.len() * 2);
        for s in &src {
            push_i16(&mut out, Some(s));
        }
        out
    }
}

async fn stream_once(
    app: &AppHandle,
    token: &SttToken,
    mic: &Option<SharedBuf>,
    system: &Option<SharedBuf>,
    stop: &Arc<AtomicBool>,
    streaming: &Arc<AtomicBool>,
) -> Result<Ended, String> {
    let both = mic.is_some() && system.is_some();
    let single_speaker: &'static str = if mic.is_some() { "me" } else { "them" };
    let channels = if both { 2 } else { 1 };
    let url = format!(
        "wss://api.deepgram.com/v1/listen?model={}&encoding=linear16&sample_rate={}&channels={}{}\
&interim_results=true&smart_format=true&punctuate=true&endpointing=300&utterance_end_ms=1200&vad_events=true",
        token.model,
        TARGET_RATE,
        channels,
        if both { "&multichannel=true" } else { "" }
    );
    let mut request = url.into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", token.token).parse().map_err(|_| "bad token header".to_string())?,
    );
    let (ws, _) = tokio_tungstenite::connect_async(request).await.map_err(|e| format!("connect: {}", e))?;
    let (mut tx, mut rx) = ws.split();
    streaming.store(true, Ordering::Relaxed);
    // Whatever Whisper may have left half-buffered is now the stream's.
    let _ = take_resampled(mic);
    let _ = take_resampled(system);

    // Writer: every 100 ms, whatever audio arrived goes out as one binary frame.
    let (w_mic, w_sys, w_stop) = (mic.clone(), system.clone(), stop.clone());
    let writer = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(100));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_send = Instant::now();
        loop {
            ticker.tick().await;
            let stopping = w_stop.load(Ordering::Relaxed);
            let frames = pcm_frames(take_resampled(&w_mic), take_resampled(&w_sys), both);
            if !frames.is_empty() {
                if tx.send(Message::Binary(frames)).await.is_err() {
                    return;
                }
                last_send = Instant::now();
            } else if last_send.elapsed() > Duration::from_secs(8) {
                if tx.send(Message::Text(r#"{"type":"KeepAlive"}"#.to_string())).await.is_err() {
                    return;
                }
                last_send = Instant::now();
            }
            if stopping {
                let _ = tx.send(Message::Text(r#"{"type":"CloseStream"}"#.to_string())).await;
                return;
            }
        }
    });

    let speaker_of = |ch: usize| -> &'static str {
        if both {
            if ch == 0 { "me" } else { "them" }
        } else {
            single_speaker
        }
    };
    let mut pending: [String; 2] = [String::new(), String::new()];
    let mut ended = Ended::Dropped;
    let finish_pending = |app: &AppHandle, pending: &mut [String; 2], ch: usize| {
        let speaker = speaker_of(ch);
        let text = std::mem::take(&mut pending[ch]);
        if !text.trim().is_empty() {
            push_entry(app, speaker, text.trim().to_string());
        }
        set_partial(app, speaker, String::new());
    };

    while let Some(msg) = rx.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                match v.get("type").and_then(|t| t.as_str()) {
                    Some("Results") => {
                        let ch = v
                            .get("channel_index")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.as_u64())
                            .unwrap_or(0) as usize;
                        let ch = ch.min(1);
                        let transcript = v
                            .pointer("/channel/alternatives/0/transcript")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let is_final = v.get("is_final").and_then(|b| b.as_bool()).unwrap_or(false);
                        let speech_final = v.get("speech_final").and_then(|b| b.as_bool()).unwrap_or(false);
                        let speaker = speaker_of(ch);
                        if is_final {
                            if !transcript.is_empty() {
                                if !pending[ch].is_empty() {
                                    pending[ch].push(' ');
                                }
                                pending[ch].push_str(&transcript);
                            }
                            if speech_final {
                                finish_pending(app, &mut pending, ch);
                            } else {
                                set_partial(app, speaker, pending[ch].clone());
                            }
                        } else if !transcript.is_empty() {
                            let shown = if pending[ch].is_empty() {
                                transcript
                            } else {
                                format!("{} {}", pending[ch], transcript)
                            };
                            set_partial(app, speaker, shown);
                        }
                    }
                    Some("UtteranceEnd") => {
                        let ch = v
                            .get("channel")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.as_u64())
                            .unwrap_or(0) as usize;
                        finish_pending(app, &mut pending, ch.min(1));
                    }
                    Some("Error") => {
                        eprintln!("[meeting] stt stream error: {}", text);
                        break;
                    }
                    _ => {}
                }
            }
            Ok(Message::Close(_)) => {
                ended = if stop.load(Ordering::Relaxed) { Ended::Stopped } else { Ended::Dropped };
                break;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[meeting] stt stream closed: {}", e);
                break;
            }
        }
        if stop.load(Ordering::Relaxed) && writer.is_finished() {
            // CloseStream was sent; give the server a moment to flush, then stop reading.
            ended = Ended::Stopped;
            let _ = tokio::time::timeout(Duration::from_millis(1500), async {
                while let Some(Ok(Message::Text(text))) = rx.next().await {
                    if text.contains("\"Results\"") {
                        // (rare) late finals - keep them
                        if let Ok(v) = serde_json::from_str::<Value>(&text) {
                            let t = v.pointer("/channel/alternatives/0/transcript").and_then(|t| t.as_str()).unwrap_or("");
                            let ch = v.get("channel_index").and_then(|c| c.get(0)).and_then(|c| c.as_u64()).unwrap_or(0) as usize;
                            if !t.trim().is_empty() && v.get("is_final").and_then(|b| b.as_bool()).unwrap_or(false) {
                                if !pending[ch.min(1)].is_empty() { pending[ch.min(1)].push(' '); }
                                pending[ch.min(1)].push_str(t.trim());
                            }
                        }
                    }
                }
            })
            .await;
            break;
        }
    }
    if stop.load(Ordering::Relaxed) {
        ended = Ended::Stopped;
    }
    for ch in 0..2 {
        finish_pending(app, &mut pending, ch);
    }
    writer.abort();
    Ok(ended)
}
