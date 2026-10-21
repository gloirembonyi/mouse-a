// Kudu Desktop - Meeting & Interview Copilot
//
// Live transcription + context-aware assistance for meetings, interviews and
// interview practice. Same split as the agent: this file does the *doing*
// (audio capture, chunking, silence detection, prompt assembly) and the
// Kudu Core does the *thinking* - every model call goes through
// /api/agent/step (no tools), so there is still no Gemini key in this app.
//
// Audio pipeline:
//   cpal capture thread ──► per-source mono f32 buffers ("them" = WASAPI
//   loopback of the default output device, i.e. the other participants;
//   "me" = default microphone)
//   ──► chunker task: cuts at a natural pause (or a hard cap), drops silent
//   chunks, resamples to 16 kHz WAV ──► Whisper (whisper-large-v3 on Groq)
//   transcribes each speaker's clip in parallel ──► `meeting-transcript`
//   events. If Whisper is unavailable, one Gemini call via /api/agent/step
//   transcribes both clips instead.
//
// Whisper route, in order: a local GROQ_API_KEY (dev / bring-your-own-key,
// read from .env.local by lib.rs) → the backend's /api/meeting/transcribe
// (production; the key stays server-side) → Gemini fallback.
//
// Privacy: audio only ever lives in memory and is sent to the backend for
// transcription; nothing is written to disk. The transcript stays in memory
// until the next session starts, and is only saved to the user's history
// when they explicitly ask (meeting_save_notes). The user profile used for
// grounding answers lives in a local file, never on the server.

use crate::backend::BackendClient;
use crate::commands::AppState;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

// ── Tunables ─────────────────────────────────────────────────────

pub(crate) const TARGET_RATE: u32 = 16_000;
/// Don't cut a chunk before this much audio - very short clips transcribe
/// badly and burn a backend call each. Short enough that a question shows up
/// (and can be answered) a beat after the speaker pauses.
const MIN_CHUNK: Duration = Duration::from_millis(1_200);
/// Always cut by this point, pause or not, so the transcript never lags far.
const MAX_CHUNK: Duration = Duration::from_secs(5);
/// Chunks in flight at once (Whisper mode). Transcription runs concurrently; the
/// results are still appended in order (see `flush`).
const MAX_INFLIGHT: usize = 3;
/// After Whisper rate-limits us (Groq's free tier: 20 requests/min), cut less
/// often for a while instead of falling back to the slower Gemini path.
const MIN_CHUNK_THROTTLED: Duration = Duration::from_secs(8);
const THROTTLE_FOR_MS: i64 = 60_000;
static THROTTLED_UNTIL_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
/// A chunk is cut early at a pause: the trailing window must be quiet.
const PAUSE_WINDOW_SECS: f32 = 0.26;
/// 30 ms frames above this RMS count as voiced (normalized -1..1 samples).
const VOICE_RMS: f32 = 0.012;
/// A clip needs at least this much voiced audio to be worth transcribing.
const MIN_VOICED_SECS: f32 = 0.35;
/// Lowest speech threshold the adaptive detector will use (quiet mics).
const MIN_VOICE_RMS: f32 = 0.0025;
/// After this long live, a mic that never rose above near-silence gets a warning.
const MIC_SILENCE_WARN_AFTER: Duration = Duration::from_secs(20);
/// Hard cap on buffered audio per source, in case transcription stalls.
const MAX_BUFFER_SECS: usize = 60;
/// Transcript budget sent with live actions (most recent part wins).
const LIVE_TRANSCRIPT_CHARS: usize = 12_000;
/// Transcript budget for whole-session actions (notes).
const FULL_TRANSCRIPT_CHARS: usize = 60_000;

// ── Types ────────────────────────────────────────────────────────

/// What the user tells us about themselves so suggestions are grounded in
/// their real experience instead of generic advice.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeetingProfile {
    /// Role, experience, skills - or a pasted resume.
    #[serde(default)]
    pub about: String,
    /// Current projects, responsibilities, numbers they care about.
    #[serde(default)]
    pub projects: String,
    /// This session: agenda, attendees, job description, goals.
    #[serde(default)]
    pub session_context: String,
    /// How answers should read (e.g. "short bullets", "formal").
    #[serde(default)]
    pub style: String,
    /// Links the user added (job posts, company pages, docs, repos), fetched and read.
    #[serde(default)]
    pub sources: Vec<SourceNote>,
    /// The briefing the model wrote from those links + the fields above. Editable.
    #[serde(default)]
    pub research: String,
    /// Leave the profile out of prompts for now (a toggle in the meeting view and
    /// the copilot bar). Inverted so the default - use it - is `false`.
    #[serde(default)]
    pub skip_context: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceNote {
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// What the fetch produced, in characters (0 = failed; `error` says why).
    #[serde(default)]
    pub chars: usize,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub fetched_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub id: u64,
    /// "them" | "me" | "coach" | "note"
    pub speaker: String,
    pub text: String,
    pub at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetingStatus {
    pub live: bool,
    pub mode: String,
    pub mic: bool,
    pub system: bool,
    pub started_at_ms: Option<i64>,
    /// Non-fatal problem to surface (a failed chunk, a missing device…).
    pub warning: Option<String>,
    /// Live (streaming) transcription is active - words appear as they are spoken.
    pub streaming: bool,
    /// Not live, but a transcript is there to continue: Resume restarts capture on it.
    pub resumable: bool,
}

#[derive(Default)]
pub(crate) struct AudioBuf {
    pub(crate) samples: Vec<f32>,
    pub(crate) rate: u32,
    /// Running estimate of this source's background noise (frame RMS), so
    /// speech detection adapts to the device: a quiet laptop mic and a hot
    /// USB mic need very different thresholds. 0 = not measured yet.
    noise_floor: f32,
    /// Loudest frame seen this session - lets us warn about a muted mic.
    peak_rms: f32,
}

pub(crate) type SharedBuf = Arc<Mutex<AudioBuf>>;

struct LiveSession {
    mode: String,
    stop: Arc<AtomicBool>,
    /// True while the streaming STT connection is up (meeting_stream.rs). The chunker
    /// then leaves the audio to the stream and only reports levels.
    streaming: Arc<AtomicBool>,
    /// Set by the chunker once its final flush is done.
    finished: Arc<AtomicBool>,
    mic: Option<SharedBuf>,
    system: Option<SharedBuf>,
    started_at_ms: i64,
}

static LIVE: Mutex<Option<LiveSession>> = Mutex::new(None);
static TRANSCRIPT: Mutex<Vec<TranscriptEntry>> = Mutex::new(Vec::new());
static LAST_MODE: Mutex<String> = Mutex::new(String::new());
/// When the current transcript's session first started - kept across Pause / Resume
/// so the clock and "started at" stay those of the meeting, not of the last resume.
static LAST_STARTED_AT: Mutex<Option<i64>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// The saved-session file the current transcript belongs to, and the answers given in it.
/// Kept here (not in a window) so answers shown in the in-call bar are saved too.
static SESSION_ID: Mutex<Option<String>> = Mutex::new(None);
static CARDS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static LAST_PERSIST_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static WRITE_LOCK: Mutex<()> = Mutex::new(());
/// Name of the input device actually opened - quoted in the "mic is silent"
/// warning, since the usual cause is Windows defaulting to the wrong input.
static MIC_NAME: Mutex<String> = Mutex::new(String::new());

/// Draining is atomic (a chunk is one slice of each buffer), transcription runs
/// concurrently (up to MAX_INFLIGHT chunks), and appending happens strictly in cut
/// order: each flush takes a sequence number at drain time and waits its turn before
/// pushing entries. One slow Whisper call no longer delays every later line.
fn drain_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}
static FLUSH_SEQ: AtomicU64 = AtomicU64::new(0);
static FLUSH_PUBLISHED: AtomicU64 = AtomicU64::new(0);
static INFLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn flush_turn() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}
fn reset_flush_order() {
    FLUSH_SEQ.store(0, Ordering::Relaxed);
    FLUSH_PUBLISHED.store(0, Ordering::Relaxed);
    INFLIGHT.store(0, Ordering::Relaxed);
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── Profile persistence ──────────────────────────────────────────
// Its own file rather than config.json: BackendClient rewrites config.json
// from a fixed struct, which would silently drop any field it doesn't know.

fn profile_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mouse-ai").join("meeting_profile.json"))
}

fn load_profile() -> MeetingProfile {
    profile_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_profile(profile: &MeetingProfile) -> Result<(), String> {
    let path = profile_path().ok_or("No config directory available")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(profile).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("Failed to save profile: {}", e))
}

// ── Audio capture ────────────────────────────────────────────────

fn build_stream_typed<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    buf: SharedBuf,
    label: &'static str,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let cap = {
        let b = buf.lock().unwrap();
        b.rate as usize * MAX_BUFFER_SECS
    };
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mut b = match buf.lock() {
                    Ok(b) => b,
                    Err(_) => return,
                };
                for frame in data.chunks(channels.max(1)) {
                    let sum: f32 = frame.iter().map(|s| <f32 as FromSample<T>>::from_sample_(*s)).sum();
                    b.samples.push(sum / frame.len() as f32);
                }
                if b.samples.len() > cap {
                    let excess = b.samples.len() - cap;
                    b.samples.drain(..excess);
                }
            },
            move |e| eprintln!("[meeting] {} stream error: {}", label, e),
            None,
        )
        .map_err(|e| format!("Couldn't open {} audio: {}", label, e))
}

fn build_stream(
    device: &cpal::Device,
    supported: cpal::SupportedStreamConfig,
    buf: SharedBuf,
    label: &'static str,
) -> Result<cpal::Stream, String> {
    let channels = supported.channels() as usize;
    buf.lock().unwrap().rate = supported.sample_rate().0;
    let config = supported.config();
    match supported.sample_format() {
        SampleFormat::F32 => build_stream_typed::<f32>(device, &config, channels, buf, label),
        SampleFormat::I16 => build_stream_typed::<i16>(device, &config, channels, buf, label),
        SampleFormat::U16 => build_stream_typed::<u16>(device, &config, channels, buf, label),
        SampleFormat::I32 => build_stream_typed::<i32>(device, &config, channels, buf, label),
        other => Err(format!("Unsupported {} sample format: {:?}", label, other)),
    }
}

/// Opens the requested sources on a dedicated thread (cpal streams aren't
/// Send on every platform, so they must live and die on one thread) and
/// keeps them alive until `stop` is set. Reports which sources actually
/// opened, or why none did.
fn spawn_capture_thread(
    stop: Arc<AtomicBool>,
    mic: Option<SharedBuf>,
    system: Option<SharedBuf>,
) -> Result<(bool, bool, Vec<String>), String> {
    let (tx, rx) = std::sync::mpsc::channel::<(bool, bool, Vec<String>)>();
    std::thread::Builder::new()
        .name("meeting-audio".into())
        .spawn(move || {
            let host = cpal::default_host();
            let mut streams: Vec<cpal::Stream> = Vec::new();
            let mut problems = Vec::new();
            let (mut mic_ok, mut sys_ok) = (false, false);

            if let Some(buf) = mic {
                let opened = host
                    .default_input_device()
                    .ok_or_else(|| "No microphone found".to_string())
                    .and_then(|d| {
                        *MIC_NAME.lock().unwrap() = d.name().unwrap_or_default();
                        let cfg = d.default_input_config().map_err(|e| format!("Microphone unavailable: {}", e))?;
                        build_stream(&d, cfg, buf, "microphone")
                    })
                    .and_then(|s| s.play().map(|_| s).map_err(|e| format!("Microphone failed to start: {}", e)));
                match opened {
                    Ok(s) => {
                        streams.push(s);
                        mic_ok = true;
                    }
                    Err(e) => problems.push(e),
                }
            }

            if let Some(buf) = system {
                // On Windows/WASAPI, an *input* stream built on an *output*
                // device is a loopback capture of whatever that device plays -
                // i.e. the other people on the call.
                let opened = host
                    .default_output_device()
                    .ok_or_else(|| "No speaker/output device found for system audio".to_string())
                    .and_then(|d| {
                        let cfg = d
                            .default_output_config()
                            .map_err(|e| format!("System audio unavailable: {}", e))?;
                        build_stream(&d, cfg, buf, "system audio")
                    })
                    .and_then(|s| s.play().map(|_| s).map_err(|e| format!("System audio failed to start: {}", e)));
                match opened {
                    Ok(s) => {
                        streams.push(s);
                        sys_ok = true;
                    }
                    Err(e) => problems.push(if cfg!(windows) {
                        e
                    } else {
                        // cpal only exposes loopback capture through WASAPI. Elsewhere the
                        // other side of the call needs a virtual device routed as the input.
                        format!(
                            "{} - on macOS/Linux, capturing the other participants needs a virtual audio device \
(e.g. BlackHole on macOS, a PulseAudio/PipeWire monitor on Linux) selected as the microphone; \
continuing with the microphone only.",
                            e
                        )
                    }),
                }
            }

            let _ = tx.send((mic_ok, sys_ok, problems));
            if streams.is_empty() {
                return;
            }
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
            }
            drop(streams);
        })
        .map_err(|e| format!("Couldn't start audio thread: {}", e))?;

    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| "Audio devices didn't respond in time".to_string())
}

// ── DSP helpers ──────────────────────────────────────────────────

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Speech = clearly above this source's own noise floor, never below a
/// small absolute minimum (pure digital silence) and never above VOICE_RMS
/// (a noisy room must not make normal speech "silent").
fn voice_threshold(noise_floor: f32) -> f32 {
    if noise_floor <= 0.0 {
        return VOICE_RMS;
    }
    (noise_floor * 3.5).clamp(MIN_VOICE_RMS, VOICE_RMS)
}

fn voiced_seconds(samples: &[f32], rate: u32, threshold: f32) -> f32 {
    let frame = (rate as usize * 30 / 1000).max(1);
    let voiced = samples.chunks(frame).filter(|f| rms(f) > threshold).count();
    voiced as f32 * 0.03
}

fn tail_is_quiet(buf: &SharedBuf) -> bool {
    let b = buf.lock().unwrap();
    let n = (b.rate as f32 * PAUSE_WINDOW_SECS) as usize;
    if b.samples.len() < n || n == 0 {
        return true;
    }
    rms(&b.samples[b.samples.len() - n..]) < voice_threshold(b.noise_floor)
}

/// Called ~4×/s: track the noise floor (falls fast to quieter frames, rises
/// slowly so speech doesn't drag it up) and the session's loudest frame.
fn update_noise(buf: &Option<SharedBuf>) {
    let Some(buf) = buf else { return };
    let mut b = buf.lock().unwrap();
    let n = (b.rate as usize * 30 / 1000).max(1);
    if b.samples.len() < n {
        return;
    }
    let r = rms(&b.samples[b.samples.len() - n..]);
    b.peak_rms = b.peak_rms.max(r);
    b.noise_floor = if b.noise_floor <= 0.0 || r < b.noise_floor { r.max(1e-5) } else { (b.noise_floor * 1.015).min(r) };
}

fn peak_rms(buf: &Option<SharedBuf>) -> f32 {
    buf.as_ref().map(|b| b.lock().unwrap().peak_rms).unwrap_or(0.0)
}

/// Loudness of the last ~200 ms of a source, scaled to 0..1 for the UI's
/// level meter (speech sits around 0.3–0.8).
fn level(buf: &Option<SharedBuf>) -> f32 {
    let Some(buf) = buf else { return 0.0 };
    let b = buf.lock().unwrap();
    let n = (b.rate as f32 * 0.2) as usize;
    if n == 0 || b.samples.is_empty() {
        return 0.0;
    }
    let tail = &b.samples[b.samples.len().saturating_sub(n)..];
    // Log scale (-60 dB → 0, -12 dB → 1): a quiet mic still visibly moves.
    let db = 20.0 * rms(tail).max(1e-6).log10();
    ((db + 60.0) / 48.0).clamp(0.0, 1.0)
}

pub(crate) fn resample_linear(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() || from == 0 {
        return samples.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = (samples.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = samples[idx.min(samples.len() - 1)];
            let b = samples[(idx + 1).min(samples.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

fn encode_wav_16k(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&TARGET_RATE.to_le_bytes());
    out.extend_from_slice(&(TARGET_RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Takes everything buffered so far for one source and returns it as 16 kHz
/// WAV bytes - or None when it's effectively silence (not worth a backend call,
/// and silence is exactly where speech models hallucinate words).
fn drain_clip(buf: &Option<SharedBuf>) -> Option<Vec<u8>> {
    let buf = buf.as_ref()?;
    let (samples, rate, threshold) = {
        let mut b = buf.lock().unwrap();
        let threshold = voice_threshold(b.noise_floor);
        (std::mem::take(&mut b.samples), b.rate, threshold)
    };
    if samples.is_empty() || voiced_seconds(&samples, rate, threshold) < MIN_VOICED_SECS {
        return None;
    }
    let mut mono16k = resample_linear(&samples, rate, TARGET_RATE);
    // Auto-gain: Whisper does much better on a normalized clip than on a
    // quiet laptop-mic recording (capped so noise isn't blown up).
    let peak = mono16k.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let gain = (0.9 / peak).clamp(1.0, 12.0);
        if gain > 1.05 {
            mono16k.iter_mut().for_each(|s| *s *= gain);
        }
    }
    Some(encode_wav_16k(&mono16k))
}

// ── Backend calls ────────────────────────────────────────────────

fn response_text(resp: &Value) -> String {
    resp.get("parts")
        .and_then(|p| p.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter(|p| !p.get("thought").and_then(|t| t.as_bool()).unwrap_or(false))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
        .trim()
        .to_string()
}

const TRANSCRIBE_SYSTEM: &str = "You are a precise speech-to-text engine for a live meeting. \
Transcribe the audio clips verbatim in the language spoken (do not translate, summarize, answer, or comment). \
Output exactly two lines and nothing else:\n\
THEM: <transcript of the THEM clip>\n\
ME: <transcript of the ME clip>\n\
Leave a line's text empty when that clip is missing or has no intelligible speech - never guess or invent words. \
If the ME clip only contains the same speech as the THEM clip (speaker echo picked up by the microphone), leave ME empty.";

async fn transcribe_gemini(
    backend: &BackendClient,
    them: Option<&[u8]>,
    me: Option<&[u8]>,
) -> Result<Vec<(String, String)>, String> {
    let mut parts = vec![json!({ "text": "Transcribe these clips." })];
    if let Some(wav) = them {
        parts.push(json!({ "text": "THEM clip (the other participants, captured from the computer's speakers):" }));
        parts.push(json!({ "inlineData": { "mimeType": "audio/wav", "data": STANDARD.encode(wav) } }));
    }
    if let Some(wav) = me {
        parts.push(json!({ "text": "ME clip (the user's own microphone):" }));
        parts.push(json!({ "inlineData": { "mimeType": "audio/wav", "data": STANDARD.encode(wav) } }));
    }
    let contents = json!([{ "role": "user", "parts": parts }]);
    let resp = backend.agent_step(TRANSCRIBE_SYSTEM, contents, json!([]), None).await?;
    Ok(parse_transcription(&response_text(&resp)))
}

// ── Whisper ──────────────────────────────────────────────────────

const GROQ_TRANSCRIBE_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

/// Set once the backend reports it can't transcribe, so a session doesn't pay
/// a wasted round-trip on every chunk before falling back.
static BACKEND_WHISPER_MISSING: AtomicBool = AtomicBool::new(false);

fn whisper_model() -> String {
    std::env::var("MOUSE_AI_WHISPER_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "whisper-large-v3".to_string())
}

fn local_groq_key() -> Option<String> {
    ["MOUSE_AI_GROQ_API_KEY", "GROQ_API_KEY"]
        .iter()
        .find_map(|k| std::env::var(k).ok())
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

fn groq_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(6))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("Failed to create HTTP client")
    })
}

async fn groq_transcribe(key: &str, wav: Vec<u8>, prompt: &str) -> Result<String, String> {
    let file = reqwest::multipart::Part::bytes(wav)
        .file_name("chunk.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", file)
        .text("model", whisper_model())
        .text("response_format", "json")
        .text("temperature", "0");
    if !prompt.is_empty() {
        form = form.text("prompt", prompt.to_string());
    }
    let resp = groq_client()
        .post(GROQ_TRANSCRIBE_URL)
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Whisper request failed: {}", e))?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        if status == 429 {
            THROTTLED_UNTIL_MS.store(now_ms() + THROTTLE_FOR_MS, Ordering::Relaxed);
        }
        return Err(format!("Whisper failed ({}): {}", status, truncate_chars(&body, 200)));
    }
    let data: Value = resp.json().await.map_err(|e| format!("Bad Whisper response: {}", e))?;
    Ok(data.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string())
}

/// One clip through Whisper. `Ok(None)` = no Whisper route available.
async fn whisper_one(backend: &BackendClient, wav: Vec<u8>, prompt: &str) -> Result<Option<String>, String> {
    if let Some(key) = local_groq_key() {
        return groq_transcribe(&key, wav, prompt).await.map(Some);
    }
    if BACKEND_WHISPER_MISSING.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let result = backend.transcribe_audio(&STANDARD.encode(&wav), prompt).await.map_err(|e| {
        if e.contains("429") || e.to_lowercase().contains("rate limit") {
            THROTTLED_UNTIL_MS.store(now_ms() + THROTTLE_FOR_MS, Ordering::Relaxed);
        }
        e
    })?;
    if result.is_none() {
        BACKEND_WHISPER_MISSING.store(true, Ordering::Relaxed);
    }
    Ok(result)
}

/// Phrases Whisper is known to produce from near-silence or noise (learned
/// from subtitle-heavy training data). Only dropped when they are the
/// *entire* utterance.
fn is_whisper_hallucination(text: &str) -> bool {
    let t: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t.is_empty()
        || matches!(
            t.as_str(),
            "you" | "thank you" | "thanks" | "thank you very much" | "thanks for watching" | "thank you for watching"
                | "bye" | "okay" | "so" | "um" | "uh" | "music" | "applause" | "silence"
        )
        || t.contains("subtitles by")
        || t.contains("amaraorg")
        || t.contains("please subscribe")
}

/// Fraction of shared words - catches the mic picking up the other side's
/// voice from the speakers (no headphones) and re-transcribing it as "me".
fn word_overlap(a: &str, b: &str) -> f32 {
    let words = |s: &str| -> std::collections::HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .map(String::from)
            .collect()
    };
    let (wa, wb) = (words(a), words(b));
    if wa.is_empty() || wb.is_empty() {
        return 0.0;
    }
    wa.intersection(&wb).count() as f32 / wa.len().min(wb.len()) as f32
}

/// Recent transcript, handed to Whisper as its prompt: it keeps spelling of
/// names/jargon consistent and gives context across chunk boundaries.
fn whisper_prompt() -> String {
    let tail = transcript_text(600);
    let start = tail.len().saturating_sub(400);
    let start = (start..tail.len()).find(|i| tail.is_char_boundary(*i)).unwrap_or(tail.len());
    tail[start..].to_string()
}

async fn transcribe(
    backend: &BackendClient,
    them: Option<Vec<u8>>,
    me: Option<Vec<u8>>,
) -> Result<Vec<(String, String)>, String> {
    let prompt = whisper_prompt();
    // Ok(Some(text)) per clip = transcribed (None when that source had no
    // speech); Ok(None) = no Whisper route; Err = Whisper failed.
    let run = |clip: Option<Vec<u8>>| {
        let prompt = prompt.clone();
        async move {
            match clip {
                Some(wav) => whisper_one(backend, wav, &prompt).await.map(|t| t.map(Some)),
                None => Ok(Some(None)),
            }
        }
    };
    let (them_res, me_res) = tokio::join!(run(them.clone()), run(me.clone()));

    match (them_res, me_res) {
        (Ok(Some(them_text)), Ok(Some(me_text))) => {
            let them_text = them_text.filter(|t| !is_whisper_hallucination(t));
            let me_text = me_text
                .filter(|t| !is_whisper_hallucination(t))
                .filter(|t| them_text.as_deref().map(|th| word_overlap(th, t) < 0.6).unwrap_or(true));
            let mut out = Vec::new();
            if let Some(t) = them_text {
                out.push(("them".to_string(), t));
            }
            if let Some(t) = me_text {
                out.push(("me".to_string(), t));
            }
            Ok(out)
        }
        // No Whisper route, or it failed (rate limit, outage): don't lose the
        // audio - transcribe this chunk through Gemini instead.
        (them_res, me_res) => {
            for r in [&them_res, &me_res] {
                if let Err(e) = r {
                    eprintln!("[meeting] whisper failed, falling back to Gemini: {}", e);
                }
            }
            transcribe_gemini(backend, them.as_deref(), me.as_deref()).await
        }
    }
}

fn parse_transcription(raw: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let tagged = [("THEM:", "them"), ("ME:", "me")]
            .iter()
            .find_map(|(tag, who)| line.strip_prefix(tag).map(|rest| (who.to_string(), rest.trim().to_string())));
        match tagged {
            Some(entry) => out.push(entry),
            // Long utterances sometimes wrap onto extra lines.
            None => {
                if let Some(last) = out.last_mut() {
                    last.1.push(' ');
                    last.1.push_str(line);
                }
            }
        }
    }
    out.into_iter()
        .map(|(who, text)| (who, text.trim().to_string()))
        .filter(|(_, text)| {
            let t = text.to_lowercase();
            !t.is_empty()
                && !matches!(
                    t.trim_matches(|c: char| !c.is_alphanumeric() && c != ' '),
                    "silence" | "no speech" | "inaudible" | "empty" | "none" | "n/a"
                )
        })
        .collect()
}

pub(crate) fn push_entry(app: &AppHandle, speaker: &str, text: String) -> TranscriptEntry {
    let entry = TranscriptEntry {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        speaker: speaker.to_string(),
        text,
        at_ms: now_ms(),
    };
    TRANSCRIPT.lock().unwrap().push(entry.clone());
    let _ = app.emit("meeting-transcript", &entry);
    persist(false);
    entry
}

fn live_buffers() -> Option<(Option<SharedBuf>, Option<SharedBuf>)> {
    LIVE.lock().unwrap().as_ref().map(|s| (s.system.clone(), s.mic.clone()))
}

/// Drain whatever audio is buffered, transcribe it, append it. Used by the
/// chunker on every cut and by assist actions, so "What should I say?"
/// always sees the question that was asked a second ago.
async fn flush(app: &AppHandle, backend: &BackendClient) -> Result<(), String> {
    if is_streaming() {
        return Ok(()); // the live stream owns the audio
    }
    let (them, me, seq) = {
        let _guard = drain_lock().lock().await;
        let Some((system, mic)) = live_buffers() else {
            return Ok(());
        };
        let them = drain_clip(&system);
        let me = drain_clip(&mic);
        if them.is_none() && me.is_none() {
            return Ok(());
        }
        (them, me, FLUSH_SEQ.fetch_add(1, Ordering::SeqCst))
    };
    // Lets the UI show a "transcribing…" placeholder line while Whisper works.
    if INFLIGHT.fetch_add(1, Ordering::SeqCst) == 0 {
        let _ = app.emit("meeting-transcribing", true);
    }
    let result = transcribe(backend, them, me).await;
    // Wait for the chunks cut before this one, so lines never reorder.
    loop {
        if FLUSH_PUBLISHED.load(Ordering::SeqCst) == seq {
            break;
        }
        let _ = tokio::time::timeout(Duration::from_millis(500), flush_turn().notified()).await;
    }
    let out = match &result {
        Ok(lines) => {
            for (who, text) in lines {
                push_entry(app, who, text.clone());
            }
            Ok(())
        }
        Err(e) => Err(e.clone()),
    };
    FLUSH_PUBLISHED.fetch_add(1, Ordering::SeqCst);
    flush_turn().notify_waiters();
    if INFLIGHT.fetch_sub(1, Ordering::SeqCst) == 1 {
        let _ = app.emit("meeting-transcribing", false);
    }
    out
}

fn emit_status(app: &AppHandle, warning: Option<String>) {
    let _ = app.emit("meeting-status", current_status(warning));
}

pub(crate) fn current_status(warning: Option<String>) -> MeetingStatus {
    let live = LIVE.lock().unwrap();
    match live.as_ref() {
        Some(s) => MeetingStatus {
            live: true,
            mode: s.mode.clone(),
            mic: s.mic.is_some(),
            system: s.system.is_some(),
            started_at_ms: Some(s.started_at_ms),
            warning,
            streaming: s.streaming.load(Ordering::Relaxed),
            resumable: false,
        },
        None => MeetingStatus {
            live: false,
            mode: LAST_MODE.lock().unwrap().clone(),
            mic: false,
            system: false,
            started_at_ms: *LAST_STARTED_AT.lock().unwrap(),
            warning,
            streaming: false,
            resumable: !TRANSCRIPT.lock().unwrap().is_empty(),
        },
    }
}

fn is_streaming() -> bool {
    LIVE.lock().unwrap().as_ref().map(|s| s.streaming.load(Ordering::Relaxed)).unwrap_or(false)
}

/// Background chunker: waits for a natural pause (or the hard cap), then
/// flushes. Exits - after one last flush - once the session is stopped.
async fn chunker(app: AppHandle, backend: BackendClient, stop: Arc<AtomicBool>, finished: Arc<AtomicBool>) {
    let mut chunk_start = Instant::now();
    let session_start = Instant::now();
    let mut warned_silent_mic = false;
    let failures = Arc::new(std::sync::atomic::AtomicU32::new(0));
    loop {
        // 120 ms ticks: a pause is noticed (and the chunk cut) almost as soon
        // as the speaker stops, instead of up to a quarter second later.
        tokio::time::sleep(Duration::from_millis(120)).await;
        let stopping = stop.load(Ordering::Relaxed);
        if let Some((system, mic)) = live_buffers() {
            update_noise(&mic);
            update_noise(&system);
            let _ = app.emit("meeting-level", json!({ "mic": level(&mic), "system": level(&system) }));
            if !warned_silent_mic && mic.is_some() && session_start.elapsed() > MIC_SILENCE_WARN_AFTER && peak_rms(&mic) < MIN_VOICE_RMS {
                warned_silent_mic = true;
                let name = MIC_NAME.lock().unwrap().clone();
                emit_status(
                    &app,
                    Some(format!(
                        "Your microphone{} is almost silent - it may be muted or the wrong input. Check Windows Settings → System → Sound → Input, then restart the session.",
                        if name.is_empty() { String::new() } else { format!(" (\"{}\")", name) }
                    )),
                );
            }
        }
        // Live stream up: it drains the audio itself; this loop only reports levels.
        if is_streaming() && !stopping {
            chunk_start = Instant::now();
            continue;
        }
        let elapsed = chunk_start.elapsed();
        let min_chunk = if now_ms() < THROTTLED_UNTIL_MS.load(Ordering::Relaxed) { MIN_CHUNK_THROTTLED } else { MIN_CHUNK };
        if !stopping && elapsed < min_chunk {
            continue;
        }
        // Back-pressure: don't cut more than MAX_INFLIGHT chunks ahead of Whisper.
        if !stopping && INFLIGHT.load(Ordering::SeqCst) >= MAX_INFLIGHT {
            continue;
        }
        if !stopping && elapsed < MAX_CHUNK {
            let quiet = match live_buffers() {
                Some((system, mic)) => {
                    system.as_ref().map(tail_is_quiet).unwrap_or(true) && mic.as_ref().map(tail_is_quiet).unwrap_or(true)
                }
                None => true,
            };
            if !quiet {
                continue;
            }
        }
        chunk_start = Instant::now();
        if stopping {
            // Last chunk: wait for it (and anything still in flight) before finishing.
            if let Err(e) = flush(&app, &backend).await {
                eprintln!("[meeting] final transcription failed: {}", e);
            }
            let deadline = Instant::now() + Duration::from_secs(15);
            while INFLIGHT.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            break;
        }
        // Not awaited: the next cut can happen while this chunk is at Whisper.
        let (app2, backend2, failures2) = (app.clone(), backend.clone(), failures.clone());
        tauri::async_runtime::spawn(async move {
            match flush(&app2, &backend2).await {
                Ok(()) => failures2.store(0, Ordering::Relaxed),
                Err(e) => {
                    let n = failures2.fetch_add(1, Ordering::Relaxed) + 1;
                    eprintln!("[meeting] transcription failed: {}", e);
                    // One blip isn't worth interrupting anyone; a streak is.
                    if n >= 2 {
                        emit_status(&app2, Some(format!("Transcription is failing: {}", e)));
                    }
                }
            }
        });
    }
    finished.store(true, Ordering::Relaxed);
}

// ── Saved sessions ───────────────────────────────────────────────
// Every call is written to disk as it happens (transcript lines throttled to one write per
// few seconds, answers and Pause immediately), so a crash, a closed window or a restart
// loses at most the last few seconds - and the session list can offer the call back.

fn session_title(entries: &[TranscriptEntry], started_ms: i64) -> String {
    let first = entries.iter().find(|e| e.speaker == "them" && e.text.chars().count() >= 25).map(|e| e.text.trim());
    match first {
        Some(t) => {
            let short: String = t.chars().take(56).collect();
            if t.chars().count() > 56 {
                format!("{}…", short.trim_end())
            } else {
                short
            }
        }
        None => chrono::DateTime::from_timestamp_millis(started_ms)
            .map(|d| format!("Call · {}", d.with_timezone(&chrono::Local).format("%b %-d, %H:%M")))
            .unwrap_or_else(|| "Call".to_string()),
    }
}

/// Write the current session. `force` skips the throttle (answers, Pause, New session).
fn persist(force: bool) {
    let now = now_ms();
    if !force && now - LAST_PERSIST_MS.load(Ordering::Relaxed) < 4_000 {
        return;
    }
    let transcript = TRANSCRIPT.lock().unwrap().clone();
    let cards = CARDS.lock().unwrap().clone();
    if transcript.is_empty() && cards.is_empty() {
        return;
    }
    LAST_PERSIST_MS.store(now, Ordering::Relaxed);
    let started = LAST_STARTED_AT.lock().unwrap().unwrap_or(now);
    let id = SESSION_ID.lock().unwrap().get_or_insert_with(|| format!("s{}", started)).clone();
    let session = json!({
        "id": id,
        "title": session_title(&transcript, started),
        "mode": LAST_MODE.lock().unwrap().clone(),
        "started_ms": started,
        "updated_ms": now,
        "ended": LIVE.lock().unwrap().is_none(),
        "transcript": transcript,
        "cards": cards,
    });
    std::thread::spawn(move || {
        let _guard = WRITE_LOCK.lock().unwrap();
        if let Err(e) = crate::sessions::save_value(&id, &session) {
            eprintln!("[pilot] could not save the session: {}", e);
        }
    });
}

fn record_card(id: Option<&str>, action: &str, question: Option<&str>, text: &str) {
    if action == "insights" {
        return;
    }
    let mut cards = CARDS.lock().unwrap();
    cards.push(json!({
        "id": id.map(str::to_string).unwrap_or_else(|| format!("c{}", now_ms())),
        "action": action,
        "question": question.map(str::trim).filter(|q| !q.is_empty()),
        "text": text,
        "at_ms": now_ms(),
    }));
    let len = cards.len();
    if len > 200 {
        cards.drain(0..len - 200);
    }
    drop(cards);
    persist(true);
}

// ── Speech-recognition vocabulary ────────────────────────────────

/// Percent-encode a query-string value.
pub(crate) fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Distinctive words from the profile (product and tool names, acronyms, CamelCase, words
/// with digits) - what speech recognition gets wrong most. Empty when context is switched off.
pub(crate) fn stt_keyterms() -> Vec<String> {
    const MAX_TERMS: usize = 40;
    let profile = load_profile();
    if profile.skip_context {
        return Vec::new();
    }
    let text = [&profile.session_context, &profile.projects, &profile.about, &profile.research]
        .iter()
        .map(|s| truncate_chars(s.trim(), 6000))
        .collect::<Vec<_>>()
        .join("
");
    let mut seen = std::collections::HashSet::new();
    let mut terms: Vec<String> = Vec::new();
    // First word of a line or sentence is capitalised anyway, so it only counts when it is
    // also unusual (acronym, digits, inner capitals).
    let mut sentence_start = true;
    for raw in text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '"' | '[' | ']' | '|' | '*' | '#')) {
        let word = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '+' && c != '#' && c != '.');
        let starts_sentence = sentence_start;
        sentence_start = raw.ends_with('.') || raw.ends_with(':') || raw.ends_with('!') || raw.ends_with('?');
        let word = word.trim_end_matches('.');
        let len = word.chars().count();
        if !(3..=30).contains(&len) || !word.chars().any(|c| c.is_alphabetic()) || word.contains("://") || word.contains('@') {
            continue;
        }
        let upper = word.chars().filter(|c| c.is_uppercase()).count();
        let digit = word.chars().any(|c| c.is_ascii_digit());
        let inner_caps = word.chars().skip(1).any(|c| c.is_uppercase());
        let capitalised = word.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
        let distinctive = digit || inner_caps || (upper >= 2 && len <= 8) || (capitalised && !starts_sentence);
        if distinctive && seen.insert(word.to_lowercase()) {
            terms.push(word.to_string());
            if terms.len() >= MAX_TERMS {
                break;
            }
        }
    }
    terms
}

// ── Assistance ───────────────────────────────────────────────────

fn mode_label(mode: &str) -> &'static str {
    match mode {
        "interview" => "a live job interview (the user is the candidate)",
        "practice" => "an interview PRACTICE session (you are the coach and interviewer)",
        _ => "a live work meeting",
    }
}

fn system_prompt(mode: &str, profile: &MeetingProfile) -> String {
    let mut s = format!(
        "You are Kudu Pilot, the meeting and interview copilot. The user is in {}. You see a rolling transcript \
(THEM = other participants, ME = the user, COACH = practice questions you asked, NOTE = text the user typed) \
plus the user's own background notes.\n\n\
Rules:\n\
- Be fast to read: the user is mid-conversation. The FIRST sentence is the direct answer (no preamble, no 'great question', no restating the question). \
Default to under 60 words; add at most 2-3 short bullets only if they genuinely help. Markdown is fine.\n\
- Answer the right question: the one the OTHER side (THEM) most recently asked that ME has not already answered in a later line. \
Never answer something ME said, a rhetorical remark, small talk or a half-finished sentence. If the newest THEM line is cut off, \
answer the last complete question, or say in one short line that they are still mid-question.\n\
- Ground everything in the user's real background below. NEVER invent experience, employers, projects, metrics, credentials or results \
they haven't given you. If their notes don't cover something, say so in one line and suggest an honest way to answer \
(related experience, how they would approach it, or a clarifying question).\n\
- Write suggested replies in the user's first-person voice, natural and spoken - not an essay.\n\
- Transcripts come from speech recognition and may contain errors, missing punctuation or mis-attributed lines; infer intent sensibly, \
using the session context and recent topic to fix misheard technical terms. If you had to assume a word, say so in a few words, never a paragraph of hedging.\n\
- Correctness beats speed. For factual, technical, math, coding or multiple-choice questions, work the problem out \
carefully before answering and give the answer that is actually right - never a plausible guess. If you are not sure, say \
what you are sure of and flag the uncertain part in a few words.\n\
- Code goes in fenced code blocks with the language tag. For multiple-choice, lead with the chosen option.\n\
- Reply in the language the conversation is happening in.\n\
- If a [SCREEN] image is attached, read it closely (questions, code, error messages, slide text, numbers) and use it - \
it is often the real context of what was just asked.\n",
        mode_label(mode)
    );
    if mode == "interview" || mode == "practice" {
        s.push_str("- For behavioral questions use a compact STAR shape (Situation, Task, Action, Result) built only from the user's real experience.\n\
- Interview rule: name the key term, decision or number up front, then the reasoning. Keep it to what can be said in 30-60 seconds.\n");
    }
    let style = profile.style.trim();
    if !style.is_empty() {
        s.push_str(&format!("- The user's preferred answer style: {}\n", style));
    }
    if profile.skip_context {
        return s;
    }
    let sections = [
        ("ABOUT THE USER", &profile.about),
        ("THEIR PROJECTS & RESPONSIBILITIES", &profile.projects),
        ("THIS SESSION (agenda / attendees / job description / goals)", &profile.session_context),
        ("RESEARCH BRIEF (from links the user added - treat as facts about the other side, the role or the product)", &profile.research),
    ];
    for (title, body) in sections {
        let body = body.trim();
        if !body.is_empty() {
            s.push_str(&format!("\n[{}]\n{}\n", title, truncate_chars(body, 8000)));
        }
    }
    s
}

/// Direct fetch from this PC, used when the backend cannot read a page (its own
/// fetch is server-side and behind rate limits). HTML becomes plain text here.
pub(crate) async fn fetch_page_direct(url: &str) -> Result<crate::backend::FetchedPage, String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) KuduDesktop/1.0")
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !(ct.is_empty() || ct.contains("html") || ct.contains("text/") || ct.contains("json") || ct.contains("xml")) {
        return Err(format!("not a text page ({})", ct));
    }
    let body = resp.text().await.map_err(|e| e.to_string())?;
    let (title, text) = if ct.contains("html") || body.trim_start().starts_with('<') { html_to_text(&body) } else { (String::new(), body) };
    if text.trim().is_empty() {
        return Err("Nothing readable on that page".to_string());
    }
    Ok(crate::backend::FetchedPage { title, text })
}

/// Minimal HTML → text: drop script/style/noscript/comments, break on block tags,
/// strip the rest, decode the common entities, collapse whitespace.
fn html_to_text(html: &str) -> (String, String) {
    let lower = html.to_ascii_lowercase();
    let title = lower
        .find("<title")
        .and_then(|i| lower[i..].find('>').map(|j| i + j + 1))
        .and_then(|start| lower[start..].find("</title>").map(|end| html[start..start + end].to_string()))
        .map(|t| decode_entities(&t).split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();

    let mut out = String::with_capacity(html.len() / 4);
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let rest = &lower[i..];
            // Skip whole elements whose content is never text.
            let mut skipped = false;
            for (open, close) in [("<script", "</script>"), ("<style", "</style>"), ("<noscript", "</noscript>"), ("<svg", "</svg>"), ("<!--", "-->")] {
                if rest.starts_with(open) {
                    match rest.find(close) {
                        Some(j) => i += j + close.len(),
                        None => i = bytes.len(),
                    }
                    skipped = true;
                    break;
                }
            }
            if skipped {
                continue;
            }
            let end = rest.find('>').map(|j| i + j + 1).unwrap_or(bytes.len());
            let tag = &lower[i..end];
            let name: String = tag.trim_start_matches('<').trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if matches!(
                name.as_str(),
                "p" | "div" | "br" | "li" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "tr" | "td" | "th" | "section" | "article" | "header" | "footer" | "nav" | "main" | "aside" | "blockquote" | "pre" | "table" | "dd" | "dt" | "hr"
            ) {
                out.push('\n');
            } else if !out.ends_with(' ') {
                out.push(' ');
            }
            i = end;
        } else {
            // Copy a run of text up to the next tag.
            let next = html[i..].find('<').map(|j| i + j).unwrap_or(bytes.len());
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    let decoded = decode_entities(&out);
    // Collapse: runs of spaces to one, more than two newlines to two.
    let mut text = String::with_capacity(decoded.len());
    let mut newlines = 0;
    let mut spaced = false;
    for ch in decoded.chars() {
        if ch == '\n' {
            newlines += 1;
            spaced = false;
            if newlines <= 2 {
                text.push('\n');
            }
        } else if ch.is_whitespace() {
            if !spaced && !text.ends_with('\n') {
                text.push(' ');
            }
            spaced = true;
        } else {
            newlines = 0;
            spaced = false;
            text.push(ch);
        }
    }
    (title, text.trim().to_string())
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#8217;", "'")
        .replace("&#8216;", "'")
        .replace("&#8220;", "\"")
        .replace("&#8221;", "\"")
        .replace("&ndash;", "-")
        .replace("&mdash;", "-")
}

/// A JSON object out of a model reply that may wrap it in prose or a fence.
fn json_loose(raw: &str) -> Option<Value> {
    let t = raw.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return v.is_object().then_some(v);
    }
    let a = t.find('{')?;
    let b = t.rfind('}')?;
    if b <= a {
        return None;
    }
    serde_json::from_str::<Value>(&t[a..=b]).ok().filter(|v| v.is_object())
}

/// Text of a tool-less model step (parts[].text joined).
fn step_text(v: &Value) -> String {
    v.get("parts")
        .and_then(|p| p.as_array())
        .map(|parts| parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(""))
        .unwrap_or_default()
        .trim()
        .to_string()
}

const RESEARCH_MAX_LINKS: usize = 8;
const RESEARCH_PER_SOURCE_CHARS: usize = 9_000;

/// Fetch every link, read it, and write the briefing the copilot will ground
/// answers in. Replaces the previous brief; the user can edit the result.
#[tauri::command]
pub async fn meeting_research(app: AppHandle, state: State<'_, AppState>, urls: Vec<String>) -> Result<MeetingProfile, String> {
    let urls: Vec<String> = urls
        .into_iter()
        .map(|u| u.trim().to_string())
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        .take(RESEARCH_MAX_LINKS)
        .collect();
    if urls.is_empty() {
        return Err("Add at least one link (http:// or https://).".to_string());
    }
    let mut profile = load_profile();
    let backend = state.backend.clone();
    let mut sources = Vec::new();
    let mut corpus = String::new();
    for url in &urls {
        let _ = app.emit("meeting-research-progress", format!("Reading {}", url));
        // The backend first (it extracts well and is rate-limited per user); this PC
        // directly when the backend cannot read the page.
        let fetched = match backend.agent_fetch_page(url, RESEARCH_PER_SOURCE_CHARS).await {
            Ok(p) => Ok(p),
            Err(first) => fetch_page_direct(url).await.map_err(|second| format!("{} / direct: {}", first, second)),
        };
        match fetched {
            Ok(page) => {
                let body = truncate_chars(page.text.trim(), RESEARCH_PER_SOURCE_CHARS);
                let title = if page.title.trim().is_empty() {
                    body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(120).collect::<String>()
                } else {
                    page.title.trim().chars().take(120).collect()
                };
                corpus.push_str(&format!("\n\n=== SOURCE: {} ({}) ===\n{}", url, title, body));
                sources.push(SourceNote { url: url.clone(), title, chars: body.chars().count(), error: String::new(), fetched_at_ms: now_ms() });
            }
            Err(e) => sources.push(SourceNote { url: url.clone(), error: e, fetched_at_ms: now_ms(), ..Default::default() }),
        }
    }
    if corpus.is_empty() {
        profile.sources = sources;
        save_profile(&profile)?;
        let _ = app.emit("meeting-profile", &profile);
        return Err("None of the links could be read.".to_string());
    }
    let _ = app.emit("meeting-research-progress", "Writing the brief".to_string());
    let empty = |s: &str| s.trim().is_empty();
    let (about_empty, projects_empty, session_empty) =
        (empty(&profile.about), empty(&profile.projects), empty(&profile.session_context));
    let system = "You prepare a private briefing for someone before a meeting, interview or call. You get the pages \
they linked (job posts, company pages, product docs, profiles, portfolios, repos) plus what they already wrote about \
themselves and the session. Reply with ONLY a JSON object, no prose, no fence:\n\
{\n\
  \"brief\": markdown string -\n\
    ## Sources (one line per source: what it is and the 2-4 facts that matter)\n\
    ## What matters for this session (5-10 bullets: facts, numbers, names, requirements, risks, talking points)\n\
    ## Questions they may ask / I should ask (up to 6),\n\
  \"about\": string - ONLY when a page is about the user themselves (their portfolio, resume, profile, GitHub) AND the \
field is marked EMPTY: 3-6 lines, first person, role, experience, skills, notable work. Else \"\".\n\
  \"projects\": string - the user's projects / responsibilities / results found in their pages, ONLY if marked EMPTY. Else \"\".\n\
  \"session\": string - ONLY if marked EMPTY and a page describes the company, role, product, agenda or counterpart: the \
facts for this session in 4-8 lines. Else \"\".\n\
}\n\
Be concrete and factual; never invent; keep the brief under 500 words.";
    let user = format!(
        "[FIELDS] about: {} · projects: {} · session: {}\n\n[ABOUT ME]\n{}\n\n[MY PROJECTS]\n{}\n\n[THIS SESSION]\n{}\n\n[LINKED PAGES]{}",
        if about_empty { "EMPTY" } else { "filled" },
        if projects_empty { "EMPTY" } else { "filled" },
        if session_empty { "EMPTY" } else { "filled" },
        truncate_chars(profile.about.trim(), 3000),
        truncate_chars(profile.projects.trim(), 3000),
        truncate_chars(profile.session_context.trim(), 3000),
        corpus
    );
    let contents = json!([{ "role": "user", "parts": [{ "text": user }] }]);
    let reply = backend.agent_step(system, contents, json!([]), None).await?;
    let raw = step_text(&reply);
    if raw.is_empty() {
        return Err("The model returned an empty brief - try again in a moment.".to_string());
    }
    let field = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::trim).unwrap_or("").to_string();
    match json_loose(&raw) {
        Some(v) => {
            let brief = field(&v, "brief");
            profile.research = if brief.is_empty() { raw.clone() } else { brief };
            // Fill only what was empty - never overwrite what the user wrote.
            let about = field(&v, "about");
            if about_empty && !about.is_empty() {
                profile.about = about;
            }
            let projects = field(&v, "projects");
            if projects_empty && !projects.is_empty() {
                profile.projects = projects;
            }
            let session = field(&v, "session");
            if session_empty && !session.is_empty() {
                profile.session_context = session;
            }
        }
        None => profile.research = raw,
    }
    profile.sources = sources;
    save_profile(&profile)?;
    let _ = app.emit("meeting-profile", &profile);
    Ok(profile)
}

/// Append text to one of the profile fields (Ask Kudu's "Add to meeting context").
#[tauri::command]
pub fn meeting_context_append(app: AppHandle, field: String, text: String) -> Result<MeetingProfile, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Nothing to add.".to_string());
    }
    let mut profile = load_profile();
    let slot = match field.as_str() {
        "about" => &mut profile.about,
        "projects" => &mut profile.projects,
        "research" => &mut profile.research,
        _ => &mut profile.session_context,
    };
    if !slot.trim().is_empty() {
        slot.push_str("\n\n");
    }
    slot.push_str(text);
    save_profile(&profile)?;
    let _ = app.emit("meeting-profile", &profile);
    Ok(profile)
}

fn instruction_for(action: &str, question: Option<&str>) -> Result<String, String> {
    Ok(match action {
        "suggest" => {
            let target = match question.map(str::trim).filter(|q| !q.is_empty()) {
                Some(q) => format!("Answer exactly this question that THEM asked (speech-recognised, may have small errors): \"{}\". ", q),
                None => "Find the newest question or request THEM made that I have not answered yet (the last THEM lines, including a line marked (still speaking) only if it is already a complete question). ".to_string(),
            };
            format!(
                "{}ANSWER IT DIRECTLY. Start with a bold **Say:** block of 1-4 sentences I can say verbatim, in my voice, whose FIRST sentence is the answer itself (no 'great question', no restating the question, no preamble). Technical or factual asks: the correct, specific answer first. Behavioral or experience asks: a compact STAR built only from my context, with a concrete detail or number. Then at most 2 short bullets, only if useful: a detail to add if they dig in, or a likely follow-up and how to take it. If their words are garbled by speech recognition, infer the most likely question from the context and answer that, naming the assumption in a few words. Only if nothing is being asked of me: one line saying so, then ONE useful thing to say now. Never open with analysis of the conversation - the **Say:** block comes first.",
                target
            )
        }
        "assist" => "Decide what would help me most RIGHT NOW and give only that: if I was just asked something, give me the answer \
to say (bold **Say:** line first); if a term, number or claim came up I may not know, explain it in one or two lines; if a screen is attached \
and shows a question, problem or slide, solve or explain what's on it. Lead with the single most useful line, then at most 3 short bullets."
            .to_string(),
        "screen" => "Look at my screen (attached). Work out what on it needs an answer right now - a question, a coding \
problem, a quiz, an error, a slide being discussed, a form - using the transcript for what was just asked. Then:\n\
- Coding problem: one line on the approach, then the complete working solution in a code block, then time/space complexity.\n\
- Question or quiz: the correct answer first (bold), then 1-3 bullets on why.\n\
- Error or bug: the cause in one line, then the fix.\n\
- Slide, doc or chart: the 2-3 points that matter and one smart thing I could say about it.\n\
If nothing on screen needs answering, briefly say what is there and how it relates to the conversation."
            .to_string(),
        "insights" => "Give me a glanceable live read of the conversation, in exactly this shape and nothing else:\n\
**Topic:** <what is being discussed right now, max 10 words>\n\
- <key point, number, decision or commitment just made>\n\
- <another, only if real>\n\
- <another, only if real>\n\
**Next:** <the one smartest thing I could say or ask next, max 20 words>\n\
Use at most 3 bullets, each under 15 words, only from what was actually said. If almost nothing has been said, reply with just **Topic:** Listening…"
            .to_string(),
        "summary" => "Summarize the conversation so far in under 120 words: the topic, key points made, and anything still unresolved. \
End with one line on where the discussion is heading."
            .to_string(),
        "followups" => "Suggest 3-5 sharp questions or follow-ups I could raise right now, each one line, ordered by usefulness. \
Prefer ones that use my context or uncover decisions, risks, owners or deadlines."
            .to_string(),
        "explain" => "Explain any jargon, acronyms, names or concepts from the recent discussion that I might not know - \
one line each, most important first. If everything is plain, say so."
            .to_string(),
        "notes" => "Write meeting notes from the full transcript with these sections: **Summary** (3-5 sentences), \
**Key points**, **Decisions**, **Action items** (owner - task - due, where known; write 'unassigned' otherwise), \
**Open questions**. Omit a section if it would be empty. Only include what was actually said."
            .to_string(),
        "practice_question" => "Ask me the next interview question. Base it on the job/session context and my background, \
vary the type across the session (behavioral, technical/role-specific, situational, motivation), and don't repeat earlier COACH questions. \
Reply with ONLY the question itself, as an interviewer would say it."
            .to_string(),
        "practice_feedback" => "Evaluate my answer (the ME/NOTE lines after the last COACH question). Give: **Score** x/10, \
**What worked** (1-2 bullets), **Improve** (2-3 specific bullets - structure, specificity, missing results/metrics, length, filler), \
then **Stronger version**: a tighter answer that uses ONLY facts I actually said or that appear in my background. \
If I haven't answered yet, say so."
            .to_string(),
        "ask" => {
            let q = question.map(str::trim).filter(|q| !q.is_empty()).ok_or("Type a question first.")?;
            format!(
                "My question: {}

Answer it directly, first sentence = the answer. Use the transcript and my context only where they help.",
                q
            )
        }
        other => return Err(format!("Unknown assist action: {}", other)),
    })
}

fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

/// Transcript as text, keeping the most recent `budget` characters.
fn transcript_text(budget: usize) -> String {
    let entries = TRANSCRIPT.lock().unwrap();
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;
    for e in entries.iter().rev() {
        let line = format!("{}: {}", e.speaker.to_uppercase(), e.text);
        used += line.len() + 1;
        if used > budget && !lines.is_empty() {
            lines.push("[… earlier transcript omitted]".to_string());
            break;
        }
        lines.push(line);
    }
    lines.reverse();
    lines.join("\n")
}

/// Recent transcript for other modules (the copilot's agent handoff).
pub fn recent_transcript(budget: usize) -> String {
    transcript_text(budget)
}

// ── Commands ─────────────────────────────────────────────────────

#[tauri::command]
pub async fn meeting_start(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: String,
    capture_mic: bool,
    capture_system: bool,
    show_bar: Option<bool>,
    resume: Option<bool>,
) -> Result<MeetingStatus, String> {
    if LIVE.lock().unwrap().is_some() {
        return Ok(current_status(None));
    }
    // Resume = continue the transcript that is already there (Pause is just stop).
    // Only a fresh session clears it, so nothing from a previous meeting leaks into
    // new prompts, and nothing from this one is lost by a pause.
    let resume = resume.unwrap_or(false) && !TRANSCRIPT.lock().unwrap().is_empty();
    let mode = if resume && mode.trim().is_empty() { LAST_MODE.lock().unwrap().clone() } else { mode };
    let mode = match mode.as_str() {
        "meeting" | "interview" | "practice" => mode,
        other => return Err(format!("Unknown mode: {}", other)),
    };

    if !resume {
        // The previous call is already on disk; make sure its last lines are, then start clean.
        persist(true);
        TRANSCRIPT.lock().unwrap().clear();
        CARDS.lock().unwrap().clear();
        *SESSION_ID.lock().unwrap() = None;
        *LAST_STARTED_AT.lock().unwrap() = None;
    }
    *LAST_MODE.lock().unwrap() = mode.clone();
    // Re-probe the backend each session - it may have been redeployed.
    BACKEND_WHISPER_MISSING.store(false, Ordering::Relaxed);

    let stop = Arc::new(AtomicBool::new(false));
    let mic_buf = capture_mic.then(|| SharedBuf::default());
    let sys_buf = capture_system.then(|| SharedBuf::default());

    let mut warning = None;
    let (mic_ok, sys_ok) = if mic_buf.is_some() || sys_buf.is_some() {
        let (stop_t, mic_t, sys_t) = (stop.clone(), mic_buf.clone(), sys_buf.clone());
        let (mic_ok, sys_ok, problems) =
            tokio::task::spawn_blocking(move || spawn_capture_thread(stop_t, mic_t, sys_t))
                .await
                .map_err(|e| format!("Task failed: {}", e))??;
        if !problems.is_empty() {
            if !mic_ok && !sys_ok {
                stop.store(true, Ordering::Relaxed);
                return Err(problems.join(" · "));
            }
            warning = Some(problems.join(" · "));
        }
        (mic_ok, sys_ok)
    } else {
        // Typed-only session (e.g. practicing without a mic) is allowed.
        (false, false)
    };

    let has_audio = mic_ok || sys_ok;
    let finished = Arc::new(AtomicBool::new(!has_audio));
    let streaming = Arc::new(AtomicBool::new(false));
    let mic_live = if mic_ok { mic_buf } else { None };
    let sys_live = if sys_ok { sys_buf } else { None };
    reset_flush_order();
    *LIVE.lock().unwrap() = Some(LiveSession {
        mode,
        stop: stop.clone(),
        streaming: streaming.clone(),
        finished: finished.clone(),
        mic: mic_live.clone(),
        system: sys_live.clone(),
        started_at_ms: {
            let mut first = LAST_STARTED_AT.lock().unwrap();
            let at = if resume { first.unwrap_or_else(now_ms) } else { now_ms() };
            *first = Some(at);
            at
        },
    });

    if has_audio {
        let backend = state.backend.clone();
        // Live stream first (words as they are spoken); the chunker waits behind the
        // `streaming` flag and takes over with Whisper if the stream is unavailable.
        tauri::async_runtime::spawn(crate::meeting_stream::run_live_stream(
            app.clone(), backend.clone(), mic_live, sys_live, stop.clone(), streaming,
        ));
        tauri::async_runtime::spawn(chunker(app.clone(), backend, stop, finished));
    }

    let status = current_status(warning);
    let _ = app.emit("meeting-status", &status);
    if show_bar.unwrap_or(true) {
        show_copilot(&app);
    }
    Ok(status)
}

#[tauri::command]
pub async fn meeting_stop(app: AppHandle) -> Result<MeetingStatus, String> {
    let handles = LIVE.lock().unwrap().as_ref().map(|s| (s.stop.clone(), s.finished.clone()));
    if let Some((stop, finished)) = handles {
        // The capture thread closes its streams and the chunker does one last
        // flush of what's buffered; wait for it, bounded so a slow backend
        // call can never hang the UI.
        stop.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !finished.load(Ordering::Relaxed) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        *LIVE.lock().unwrap() = None;
    }
    persist(true);
    let status = current_status(None);
    let _ = app.emit("meeting-status", &status);
    Ok(status)
}

#[tauri::command]
pub fn meeting_status() -> MeetingStatus {
    current_status(None)
}

#[tauri::command]
pub fn meeting_transcript() -> Vec<TranscriptEntry> {
    TRANSCRIPT.lock().unwrap().clone()
}

/// Add a typed line - a question the user heard but audio missed, a typed
/// practice answer, or a private note.
#[tauri::command]
pub fn meeting_add_entry(app: AppHandle, speaker: String, text: String) -> Result<TranscriptEntry, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Nothing to add.".to_string());
    }
    let speaker = match speaker.as_str() {
        "them" | "me" | "note" => speaker,
        _ => "note".to_string(),
    };
    Ok(push_entry(&app, &speaker, text))
}

/// Open a saved call as the paused session (Resume continues it): after an app restart, or
/// when the user picks a past call from the list. Whatever was paused here is saved first.
#[tauri::command]
pub fn meeting_restore(app: AppHandle, id: String) -> Result<Value, String> {
    if LIVE.lock().unwrap().is_some() {
        return Err("Pause the current call before opening another session.".to_string());
    }
    let saved = crate::sessions::session_get(id.clone())?;
    persist(true);
    let entries: Vec<TranscriptEntry> = saved
        .get("transcript")
        .cloned()
        .and_then(|t| serde_json::from_value(t).ok())
        .unwrap_or_default();
    let mode = match saved.get("mode").and_then(|m| m.as_str()) {
        Some(m @ ("meeting" | "interview" | "practice")) => m.to_string(),
        _ => "meeting".to_string(),
    };
    NEXT_ID.fetch_max(entries.iter().map(|e| e.id).max().unwrap_or(0) + 1, Ordering::Relaxed);
    *TRANSCRIPT.lock().unwrap() = entries;
    *CARDS.lock().unwrap() = saved.get("cards").and_then(|c| c.as_array()).cloned().unwrap_or_default();
    *LAST_MODE.lock().unwrap() = mode;
    *LAST_STARTED_AT.lock().unwrap() = saved.get("started_ms").and_then(|v| v.as_i64());
    *SESSION_ID.lock().unwrap() = Some(id);
    emit_status(&app, None);
    Ok(json!({ "status": current_status(None), "session": saved }))
}

#[tauri::command]
pub fn meeting_clear(app: AppHandle) {
    if let Some(id) = SESSION_ID.lock().unwrap().take() {
        let _ = crate::sessions::session_delete(id);
    }
    CARDS.lock().unwrap().clear();
    TRANSCRIPT.lock().unwrap().clear();
    *LAST_STARTED_AT.lock().unwrap() = None;
    emit_status(&app, None);
}

#[tauri::command]
pub async fn meeting_assist(
    app: AppHandle,
    state: State<'_, AppState>,
    action: String,
    question: Option<String>,
    include_screen: Option<bool>,
    request_id: Option<String>,
) -> Result<MeetingAnswer, String> {
    let backend = state.backend.clone();
    let instruction = instruction_for(&action, question.as_deref())?;

    // Pull in what was *just* said before answering - this is the difference
    // between answering the current question and the previous one. (Insights
    // run in the background and can wait for the next natural cut.)
    if action != "practice_question" && action != "insights" && !is_streaming() {
        if let Err(e) = flush(&app, &backend).await {
            eprintln!("[meeting] pre-assist flush failed: {}", e);
        }
    }

    let mode = {
        let live = LIVE.lock().unwrap();
        live.as_ref().map(|s| s.mode.clone()).unwrap_or_else(|| LAST_MODE.lock().unwrap().clone())
    };
    let mode = if mode.is_empty() { "meeting".to_string() } else { mode };
    let profile = load_profile();
    let budget = if action == "notes" { FULL_TRANSCRIPT_CHARS } else { LIVE_TRANSCRIPT_CHARS };
    let mut transcript = transcript_text(budget);
    // With live transcription the sentence being spoken right now is not a line yet -
    // include it, marked, so "What should I say?" answers the question still in progress.
    for (who, text) in crate::meeting_stream::live_partials() {
        if !transcript.is_empty() {
            transcript.push('\n');
        }
        transcript.push_str(&format!("{} (still speaking): {}", who.to_uppercase(), text));
    }

    let mut parts = vec![json!({
        "text": format!(
            "[TRANSCRIPT]\n{}\n\n[REQUEST]\n{}",
            if transcript.is_empty() { "(nothing captured yet)".to_string() } else { transcript },
            instruction
        )
    })];

    // "screen" is only useful with the screenshot, so it never depends on the toggle.
    let mut screen_thumb = None;
    if action == "screen" || include_screen.unwrap_or(false) {
        let shot = tokio::task::spawn_blocking(crate::capture::capture_meeting_screen)
            .await
            .map_err(|e| format!("Task failed: {}", e))?;
        match shot {
            Ok((b64, thumb)) => {
                parts.push(json!({ "text": "[SCREEN - what is on the user's screen right now, e.g. shared slides, a document or a coding question]" }));
                parts.push(json!({ "inlineData": { "mimeType": "image/jpeg", "data": b64 } }));
                screen_thumb = Some(thumb);
            }
            Err(e) if action == "screen" => return Err(e),
            Err(e) => eprintln!("[meeting] screen capture failed: {}", e),
        }
    }

    let contents = json!([{ "role": "user", "parts": parts }]);
    let system = system_prompt(&mode, &profile);
    // Streamed when the caller can show it (request_id) and the backend
    // supports it: the first words land in ~1 s instead of after the whole
    // answer is written. Older backends fall back to one buffered step.
    // A little thinking where a wrong answer costs most (technical / factual asks, the screen);
    // none for the glance-only actions, which stay as fast as before.
    let thinking: Option<u32> = match action.as_str() {
        "suggest" | "ask" | "assist" => Some(384),
        "screen" => Some(1024),
        _ => None,
    };
    let streamed = match &request_id {
        Some(id) => {
            let emit_app = app.clone();
            let id = id.clone();
            backend
                .agent_stream(&system, contents.clone(), thinking, move |delta| {
                    let _ = emit_app.emit("meeting-answer-delta", json!({ "id": id, "delta": delta }));
                })
                .await
                .map_err(|e| friendly_assist_error(&e))?
        }
        None => None,
    };
    let text = match streamed {
        Some(text) => text.trim().to_string(),
        None => {
            let resp = backend
                .agent_step(&system, contents, json!([]), thinking)
                .await
                .map_err(|e| friendly_assist_error(&e))?;
            response_text(&resp)
        }
    };
    if text.is_empty() {
        return Err("The assistant returned an empty answer - try again.".to_string());
    }
    if action == "practice_question" {
        push_entry(&app, "coach", text.clone());
    }
    record_card(request_id.as_deref(), &action, question.as_deref(), &text);
    Ok(MeetingAnswer { text, screen: screen_thumb })
}

/// An answer, plus a thumbnail of the screen it was based on (when one was
/// attached) so the user can see exactly what the copilot looked at.
#[derive(Debug, Clone, Serialize)]
pub struct MeetingAnswer {
    pub text: String,
    pub screen: Option<String>,
}

/// The panel shows this mid-call: one readable line, not a raw HTTP body.
fn friendly_assist_error(err: &str) -> String {
    let lower = err.to_lowercase();
    if lower.contains("(503)") || lower.contains("(502)") || lower.contains("(504)") || lower.contains("busy") || lower.contains("high demand") || lower.contains("overloaded") {
        "The AI is busy right now - try again in a few seconds.".to_string()
    } else if lower.contains("(429)") || lower.contains("rate limit") || lower.contains("quota") {
        "Too many requests in a row - wait a few seconds and try again.".to_string()
    } else if lower.contains("agent step failed") {
        // Some other backend error: keep the status, drop the JSON body.
        err.split(':').next().unwrap_or(err).replace("Agent step failed", "The AI request failed")
    } else {
        err.to_string()
    }
}

#[tauri::command]
pub fn meeting_get_profile() -> MeetingProfile {
    load_profile()
}

#[tauri::command]
pub fn meeting_set_profile(app: AppHandle, profile: MeetingProfile) -> Result<(), String> {
    save_profile(&profile)?;
    // The copilot bar and the meeting view both show the "use my context" switch.
    let _ = app.emit("meeting-profile", &profile);
    Ok(())
}

/// Explicit, user-initiated save of the session into their Threads history.
/// The full transcript is only included when the user opts in.
#[tauri::command]
pub async fn meeting_save_notes(
    state: State<'_, AppState>,
    notes: String,
    include_transcript: bool,
) -> Result<String, String> {
    let notes = notes.trim();
    if notes.is_empty() {
        return Err("Generate notes first.".to_string());
    }
    let mode = LAST_MODE.lock().unwrap().clone();
    let title = match mode.as_str() {
        "interview" => "Interview notes",
        "practice" => "Interview practice",
        _ => "Meeting notes",
    };
    let mut task = format!("{} - {}", title, chrono::Local::now().format("%Y-%m-%d %H:%M"));
    if include_transcript {
        task.push_str("\n\nTranscript:\n");
        task.push_str(&transcript_text(7_000));
    }
    state.backend.log_agent_run(None, &task, notes).await
}

// ── In-call copilot bar ──────────────────────────────────────────
// A small always-on-top window (`copilot`, copilot.html) that floats over
// the call - Assist / What should I say? / Follow-ups / Recap plus live
// insights - so the user never has to leave the meeting app. Like the other
// Kudu Desktop windows it is excluded from screen capture by default
// (WDA_EXCLUDEFROMCAPTURE), so it doesn't show up in a shared screen or a
// recording; the user can turn that off.

/// Hide Kudu Desktop's windows from screen share / recordings.
static STEALTH: AtomicBool = AtomicBool::new(true);
/// The bar is centered at the top on its first show; after that it stays
/// wherever the user dragged it.
static COPILOT_PLACED: AtomicBool = AtomicBool::new(false);

const COPILOT_WINDOWS: [&str; 4] = ["main", "overlay", "copilot", "shield"];

pub fn is_live() -> bool {
    LIVE.lock().unwrap().is_some()
}

/// Set the in-memory flag and apply it to every window (no persistence - prefs
/// owns that and calls this from `prefs::apply`).
pub fn set_stealth(app: &AppHandle, enabled: bool) {
    STEALTH.store(enabled, Ordering::Relaxed);
    apply_stealth(app);
}

/// Applies the screen-share rule to every window that is on screen right now. Hidden
/// windows are skipped on purpose and pick the rule up from `protect_shown` when they
/// are next shown: on Windows 11, changing a HIDDEN window's display affinity makes
/// DWM paint it - a framed, white, click-through "ghost" (Kudu Lens, Kudu Desktop
/// Working, the Pilot bar) that stays on screen until the app exits.
pub fn apply_stealth(app: &AppHandle) {
    for label in COPILOT_WINDOWS {
        if let Some(w) = app.get_webview_window(label) {
            if w.is_visible().unwrap_or(false) {
                protect_shown(&w);
            }
        }
    }
}

/// The affinity each window actually has, so a show only calls into the OS when the
/// rule changed. Windows start unprotected (no `contentProtected` in tauri.conf.json).
fn protected_state() -> &'static Mutex<std::collections::HashMap<String, bool>> {
    static S: OnceLock<Mutex<std::collections::HashMap<String, bool>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Call right AFTER showing a window: brings its capture protection in line with
/// the stealth setting. Never call it on a hidden window (see `apply_stealth`).
pub fn protect_shown(w: &tauri::WebviewWindow) {
    let on = STEALTH.load(Ordering::Relaxed);
    let mut state = protected_state().lock().unwrap();
    if state.get(w.label()).copied().unwrap_or(false) == on {
        return;
    }
    if w.set_content_protected(on).is_ok() {
        state.insert(w.label().to_string(), on);
    }
}

/// Show the bar without taking focus away from the call window.
fn show_no_activate(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        unsafe {
            winapi::um::winuser::ShowWindow(hwnd.0 as _, winapi::um::winuser::SW_SHOWNOACTIVATE);
        }
        return;
    }
    let _ = window.show();
}

pub fn show_copilot(app: &AppHandle) {
    let Some(w) = app.get_webview_window("copilot") else { return };
    let _ = w.set_always_on_top(true);
    crate::copilot::remember_foreground();
    if !COPILOT_PLACED.swap(true, Ordering::Relaxed) {
        let (cx, cy) = crate::mouse::get_cursor_position();
        let monitor = app
            .monitor_from_point(cx as f64, cy as f64)
            .ok()
            .flatten()
            .or_else(|| w.primary_monitor().ok().flatten());
        if let Some(mon) = monitor {
            let width = w.outer_size().map(|s| s.width as i32).unwrap_or(680);
            let x = mon.position().x + (mon.size().width as i32 - width) / 2;
            let y = mon.position().y + (12.0 * mon.scale_factor()) as i32;
            let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
        }
    }
    show_no_activate(&w);
    protect_shown(&w);
    set_move_keys(app);
}

/// Mirror of `show_no_activate`: the bar is shown with a raw ShowWindow so the call
/// keeps focus, which means tao never records it as visible - and tao's `hide()`
/// only applies a *change* of visibility, so on its own it did nothing. This is why
/// the X and Ctrl+Shift+\ "closed the meeting but not the bar".
fn hide_copilot(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("copilot") {
        #[cfg(windows)]
        if let Ok(hwnd) = w.hwnd() {
            unsafe {
                winapi::um::winuser::ShowWindow(hwnd.0 as _, winapi::um::winuser::SW_HIDE);
            }
        }
        let _ = w.hide();
    }
    set_move_keys(app);
}

/// Pixels per Ctrl+Alt+Arrow press (logical; scaled per monitor).
const NUDGE_STEP: f64 = 48.0;

/// Ctrl+Alt+Arrows move the bar so it can sit right where the user is looking,
/// without ever taking focus from the call. Registered only while the bar is
/// visible - globally grabbing those keys all the time would steal them from
/// editors that use them (e.g. VS Code's add-cursor).
///
/// Runs on its own thread: show/hide is often triggered from another global
/// shortcut's handler, and the plugin holds its shortcut table lock while a
/// handler runs - registering from inside one would deadlock.
fn set_move_keys(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || apply_move_keys(&app));
}

/// Reconciles with the bar's real visibility, so a quick show→hide can't
/// leave the keys registered whichever thread runs last.
fn apply_move_keys(app: &AppHandle) {
    use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
    static KEYS_LOCK: Mutex<()> = Mutex::new(());
    let _serial = KEYS_LOCK.lock().unwrap();
    let on = app
        .get_webview_window("copilot")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    let keys = [(Code::ArrowLeft, -1.0, 0.0), (Code::ArrowRight, 1.0, 0.0), (Code::ArrowUp, 0.0, -1.0), (Code::ArrowDown, 0.0, 1.0)];
    let gs = app.global_shortcut();
    for (code, dx, dy) in keys {
        let sc = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), code);
        let registered = gs.is_registered(sc);
        if on && !registered {
            let app_handle = app.clone();
            let res = gs.on_shortcut(sc, move |_app, _sc, event| {
                if event.state() == ShortcutState::Pressed {
                    nudge_copilot(&app_handle, dx, dy);
                }
            });
            if let Err(e) = res {
                eprintln!("[copilot] could not register move key: {}", e);
            }
        } else if !on && registered {
            let _ = gs.unregister(sc);
        }
    }
}

fn nudge_copilot(app: &AppHandle, dx: f64, dy: f64) {
    let Some(w) = app.get_webview_window("copilot") else { return };
    let Ok(pos) = w.outer_position() else { return };
    let scale = w.scale_factor().unwrap_or(1.0);
    let step = (NUDGE_STEP * scale).round() as i32;
    let (mut x, mut y) = (pos.x + dx as i32 * step, pos.y + dy as i32 * step);
    // Keep at least the pill on whichever monitor it is on.
    if let (Ok(Some(mon)), Ok(size)) = (w.current_monitor(), w.outer_size()) {
        let (mx, my) = (mon.position().x, mon.position().y);
        let (mw, mh) = (mon.size().width as i32, mon.size().height as i32);
        let pill = (60.0 * scale) as i32;
        x = x.clamp(mx - size.width as i32 + pill * 2, mx + mw - pill * 2);
        y = y.clamp(my, my + mh - pill);
    }
    let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
}

/// Toggle the bar's visibility (Ctrl+Shift+\). Returns whether it is now visible.
pub fn toggle_copilot(app: &AppHandle) -> bool {
    let Some(w) = app.get_webview_window("copilot") else { return false };
    #[cfg(windows)]
    let visible = w
        .hwnd()
        .map(|h| unsafe { winapi::um::winuser::IsWindowVisible(h.0 as _) != 0 })
        .unwrap_or(false);
    #[cfg(not(windows))]
    let visible = w.is_visible().unwrap_or(false);
    if visible {
        hide_copilot(app);
        false
    } else {
        show_copilot(app);
        true
    }
}

#[tauri::command]
pub fn copilot_show(app: AppHandle) {
    show_copilot(&app);
}

#[tauri::command]
pub fn copilot_hide(app: AppHandle) {
    hide_copilot(&app);
}

/// Resize the bar to fit its content. Width stays put; the top edge stays
/// anchored so the pill never jumps while the panel grows.
#[tauri::command]
pub fn copilot_resize(app: AppHandle, width: f64, height: f64) {
    if let Some(w) = app.get_webview_window("copilot") {
        let (width, height) = (width.clamp(320.0, 960.0), height.clamp(48.0, 900.0));
        let _ = w.set_size(tauri::LogicalSize::new(width, height));
        clamp_copilot_into_monitor(&app, width, height);
    }
}

/// The bar grows downward (a longer answer, the More menu). Near the bottom of the screen that
/// used to push its last rows - the input and the menu - below the edge, where they could not
/// be seen or clicked. After every resize the window is pulled back up so all of it is on
/// the monitor it is on (leaving room for a bottom taskbar).
fn clamp_copilot_into_monitor(app: &AppHandle, width: f64, height: f64) {
    let Some(win) = app.get_webview_window("copilot") else { return };
    let Ok(pos) = win.outer_position() else { return };
    let scale = win.scale_factor().unwrap_or(1.0);
    let (w, h) = ((width * scale) as i32, (height * scale) as i32);
    let monitor = app
        .monitor_from_point((pos.x + w / 2) as f64, (pos.y + 24) as f64)
        .ok()
        .flatten()
        .or_else(|| win.current_monitor().ok().flatten());
    let Some(mon) = monitor else { return };
    let (mpos, msize) = (mon.position(), mon.size());
    let taskbar = (48.0 * mon.scale_factor()) as i32;
    let max_x = mpos.x + msize.width as i32 - w - 4;
    let max_y = mpos.y + msize.height as i32 - h - taskbar;
    let nx = pos.x.min(max_x).max(mpos.x + 4);
    let ny = pos.y.min(max_y).max(mpos.y + 4);
    if nx != pos.x || ny != pos.y {
        let _ = win.set_position(tauri::PhysicalPosition::new(nx, ny));
    }
}

#[tauri::command]
pub fn copilot_focus(app: AppHandle) {
    // Still the user's app at this point - remember it so an insert can go back to it.
    crate::copilot::remember_foreground();
    if let Some(w) = app.get_webview_window("copilot") {
        let _ = w.set_focus();
    }
}

#[tauri::command]
pub fn stealth_get() -> bool {
    STEALTH.load(Ordering::Relaxed)
}

#[tauri::command]
pub fn stealth_set(app: AppHandle, enabled: bool) -> bool {
    // Remembered for the next launch; prefs::update applies it to every window.
    let mut prefs = crate::prefs::get();
    prefs.hide_from_screen_share = enabled;
    if crate::prefs::update(&app, prefs).is_err() {
        set_stealth(&app, enabled);
    }
    let _ = app.emit("stealth-changed", enabled);
    enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tagged_lines_and_drops_empty() {
        let out = parse_transcription("THEM: How did you scale it?\nME:\n");
        assert_eq!(out, vec![("them".to_string(), "How did you scale it?".to_string())]);
    }

    #[test]
    fn joins_wrapped_lines_and_filters_silence_markers() {
        let out = parse_transcription("THEM: first part\nsecond part\nME: (silence)");
        assert_eq!(out, vec![("them".to_string(), "first part second part".to_string())]);
    }

    #[test]
    fn silent_audio_is_not_sent() {
        let buf: Option<SharedBuf> = Some(Arc::new(Mutex::new(AudioBuf { samples: vec![0.0; 48_000], rate: 48_000, ..Default::default() })));
        assert!(drain_clip(&buf).is_none());
    }

    #[test]
    fn voiced_audio_becomes_16k_wav() {
        let samples: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let buf: Option<SharedBuf> = Some(Arc::new(Mutex::new(AudioBuf { samples, rate: 48_000, ..Default::default() })));
        let wav = drain_clip(&buf).expect("voiced clip");
        assert_eq!(&wav[0..4], b"RIFF");
        // 1 s at 16 kHz, 16-bit mono.
        assert_eq!(wav.len(), 44 + 16_000 * 2);
    }

    /// A quiet laptop mic (speech RMS ~0.007, under the old fixed 0.012
    /// cutoff) must still be transcribed once the noise floor is known.
    #[test]
    fn quiet_mic_speech_is_kept_once_noise_floor_is_known() {
        let samples: Vec<f32> = (0..48_000).map(|i| 0.01 * (i as f32 * 0.05).sin()).collect();
        let buf: Option<SharedBuf> =
            Some(Arc::new(Mutex::new(AudioBuf { samples, rate: 48_000, noise_floor: 0.0003, peak_rms: 0.0 })));
        let wav = drain_clip(&buf).expect("quiet speech should be kept");
        // Auto-gain lifted it: the loudest sample is far above the raw 0.01 (≈327).
        let peak = wav[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs()).max().unwrap();
        assert!(peak > 3000, "expected boosted audio, got peak {}", peak);
    }

    #[test]
    fn filters_whisper_silence_hallucinations() {
        assert!(is_whisper_hallucination(" Thank you."));
        assert!(is_whisper_hallucination("Thanks for watching!"));
        assert!(!is_whisper_hallucination("Thank you for having me, I am excited about this role."));
    }

    #[test]
    fn detects_speaker_echo_on_mic() {
        let them = "Can you walk me through how you scaled the payments service?";
        assert!(word_overlap(them, "walk me through how you scaled the payments service") > 0.6);
        assert!(word_overlap(them, "Sure, we moved the ledger to Postgres partitions.") < 0.6);
    }

    /// Calls Groq for real with the key from .env.local / the environment.
    /// `MEETING_TEST_WAV=clip.wav cargo test --lib meeting -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn transcribes_speech_with_groq_whisper() {
        let _ = dotenvy::from_path("../.env.local");
        let key = local_groq_key().expect("GROQ_API_KEY not set");
        let path = std::env::var("MEETING_TEST_WAV").expect("set MEETING_TEST_WAV to a speech .wav");
        let wav = std::fs::read(path).unwrap();
        let t = Instant::now();
        let text = groq_transcribe(&key, wav, "").await.expect("whisper");
        println!("{} in {:?}: {}", whisper_model(), t.elapsed(), text);
        assert!(text.to_lowercase().contains("payments"));
    }

    /// Opens the real microphone + loopback devices for a moment. Hardware
    /// dependent, so opt-in: `cargo test --lib meeting -- --ignored`.
    #[test]
    #[ignore]
    fn opens_real_audio_devices() {
        let stop = Arc::new(AtomicBool::new(false));
        let (mic, sys) = (SharedBuf::default(), SharedBuf::default());
        let (mic_ok, sys_ok, problems) =
            spawn_capture_thread(stop.clone(), Some(mic.clone()), Some(sys.clone())).expect("capture thread");
        std::thread::sleep(Duration::from_millis(3000));
        stop.store(true, Ordering::Relaxed);
        let (mic_len, mic_rate, mic_rms) = { let b = mic.lock().unwrap(); (b.samples.len(), b.rate, rms(&b.samples)) };
        let (sys_len, sys_rate, sys_rms) = { let b = sys.lock().unwrap(); (b.samples.len(), b.rate, rms(&b.samples)) };
        println!("mic_rms={:.5} system_rms={:.5} (voice threshold {})", mic_rms, sys_rms, VOICE_RMS);
        println!(
            "mic_ok={} ({} samples @ {} Hz), system_ok={} ({} samples @ {} Hz), problems={:?}",
            mic_ok, mic_len, mic_rate, sys_ok, sys_len, sys_rate, problems
        );
        assert!(mic_ok || sys_ok, "no audio source opened: {:?}", problems);
    }
}
