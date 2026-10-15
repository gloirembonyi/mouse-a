// Typed wrappers around the Rust commands in src-tauri/src/commands.rs.
// Keeping every invoke() call behind a named function here means a renamed
// or reshaped Rust command only needs updating in one place.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  AgentProgressEvent,
  AgentRunResult,
  ChatMessage,
  ConversationMessage,
  Conversation,
  CursorContext,
  HistoryStats,
  SessionState,
  User,
} from "./types";

export const appWindow = getCurrentWindow();

// ── Auth ─────────────────────────────────────────────────
export const ensureSession = () => invoke<SessionState>("ensure_session");
/** The login changed outside this window - e.g. adopted from the paired browser extension (auth_sync.rs). */
export const onAuthChanged = (cb: (s: SessionState) => void) => listen<SessionState>("auth-changed", (e) => cb(e.payload));
export const verifySession = () => invoke<SessionState>("verify_session");
export const startGoogleSignIn = () => invoke<SessionState>("start_google_signin");
export const signInWithToken = (accessToken: string) =>
  invoke<SessionState>("sign_in_with_token", { accessToken });
export const signOut = () => invoke<void>("sign_out");

// ── AI active / hover mode ────────────────────────────────
export const toggleAiActive = () => invoke<boolean>("toggle_ai_active");
export const getAiActive = () => invoke<boolean>("get_ai_active");
export const getCursorContext = () => invoke<CursorContext>("get_cursor_context");
export const askAtCursor = () => invoke<void>("ask_at_cursor");

// ── Chat ─────────────────────────────────────────────────
export interface ChatResult {
  text: string;
  conversation_id?: string | null;
}

export const sendChat = (text: string, webSearch = false, imageDataUrl: string | null = null) =>
  invoke<ChatResult>("send_chat", { text, imageDataUrl, webSearch });

export const captureAnalyze = () =>
  invoke<ChatResult>("analyze_at_cursor", { userQuestion: null, radius: 300 });

export const newConversation = () => invoke<void>("new_conversation");

// ── Agent ────────────────────────────────────────────────
/** `source` tags the run's progress events and its Stop flag - chat runs use
 *  "main:<session>" so several chats can each have a run going at once. */
export const agentRun = (task: string, source: string = "main", imageDataUrl: string | null = null) =>
  invoke<AgentRunResult>("agent_run", { task, source, autonomous: true, imageDataUrl });

export const agentCancel = (source: string = "main") => invoke<void>("agent_cancel", { source });
/** Talk to a running agent; false = nothing running for that source. */
export const agentSteer = (source: string, text: string) => invoke<boolean>("agent_steer", { source, text });

export const agentConfirm = (requestId: string, approved: boolean) =>
  invoke<boolean>("agent_confirm", { requestId, approved });

export const onAgentProgress = (cb: (e: AgentProgressEvent) => void): Promise<UnlistenFn> =>
  listen<AgentProgressEvent>("agent-progress", (e) => cb(e.payload));

// ── History / conversations ──────────────────────────────
export const getHistoryStats = () => invoke<HistoryStats>("get_history_stats");
export const getConversations = (limit = 50) =>
  invoke<Conversation[]>("get_conversations", { limit });
export const getConversationMessages = (id: string, limit = 200) =>
  invoke<ConversationMessage[]>("get_conversation_messages", { id, limit });
export const setConversationId = (id: string | null) =>
  invoke<void>("set_conversation_id", { id });
export const deleteConversation = (id: string) => invoke<void>("delete_conversation", { id });

// ── Settings ──────────────────────────────────────────────
export const getBackendUrl = () => invoke<string>("get_backend_url");
export const setBackendUrl = (url: string) => invoke<string>("set_backend_url", { url });
export const testBackend = () => invoke<string>("test_backend");
export const getMcpServers = () => invoke<string>("get_mcp_servers");
export const setMcpServers = (json: string) => invoke<string>("set_mcp_servers", { json });
export const quitApp = () => invoke<void>("quit_app");

// ── Window state events ──────────────────────────────────
export const onAiState = (cb: (active: boolean) => void) =>
  listen<boolean>("ai-state", (e) => cb(e.payload));
export const onCaptureAnalyze = (cb: () => void) => listen("capture-analyze", () => cb());
export const onQuickAsk = (cb: () => void) => listen("quick-ask", () => cb());
/** The overlay's "Continue in Mouse AI": open this conversation in the chat view. */
export const onOpenConversation = (cb: (id: string) => void) =>
  listen<string>("open-conversation", (e) => cb(e.payload));

// ── File browser / editor (IDE dock) ─────────────────────
export interface FsEntry {
  name: string;
  path: string;
  is_dir: boolean;
}

export const fsHomeDir = () => invoke<string>("fs_home_dir");
export const fsListDir = (path: string) => invoke<FsEntry[]>("fs_list_dir", { path });
export const fsReadTextFile = (path: string) => invoke<string>("fs_read_text_file", { path });
export const fsWriteTextFile = (path: string, content: string) =>
  invoke<void>("fs_write_text_file", { path, content });
export const fsParentDir = (path: string) => invoke<string | null>("fs_parent_dir", { path });
export const fsSearchFiles = (root: string, query: string, limit = 30) =>
  invoke<FsEntry[]>("fs_search_files", { root, query, limit });
export interface FileInfo {
  size: number;
  /** CSV, HTML and JSON stay "text" (editable); the editor picks their view by extension. */
  kind: "text" | "image" | "pdf" | "binary" | "docx" | "pptx" | "sheet" | "audio" | "video";
  crlf: boolean;
}
export const fsFileInfo = (path: string) => invoke<FileInfo>("fs_file_info", { path });
export const fsReadDataUrl = (path: string) => invoke<string>("fs_read_data_url", { path });
/** Streams a local file into <img>/<video>/<audio> through Tauri's asset
 * protocol - no base64 round trip through IPC, so big media is fine. */
export const assetUrl = (path: string) => convertFileSrc(path);
/** Plain text of a document (docx/pptx/sheets/text) for chat attachments. */
export const fsExtractText = (path: string) => invoke<string>("fs_extract_text", { path });

// ── Document viewers (src-tauri/src/documents.rs) ──
export interface DocxDoc {
  /** Built and escaped in Rust - only simple formatting tags. */
  html: string;
  words: number;
  truncated: boolean;
}
export interface Slide {
  number: number;
  title: string | null;
  lines: { text: string; level: number }[];
  notes: string | null;
  images: number;
}
export interface SheetData {
  sheets: string[];
  sheet: string;
  rows: string[][];
  /** 1-based spreadsheet row/column of rows[0][0]. */
  first_row: number;
  first_col: number;
  total_rows: number;
  total_cols: number;
  truncated: boolean;
}
export const docReadDocx = (path: string) => invoke<DocxDoc>("doc_read_docx", { path });
export const docReadPptx = (path: string) => invoke<Slide[]>("doc_read_pptx", { path });
export const docReadSheet = (path: string, sheet?: string) => invoke<SheetData>("doc_read_sheet", { path, sheet: sheet ?? null });
export const fsCreateFile = (path: string) => invoke<void>("fs_create_file", { path });
export const fsCreateDir = (path: string) => invoke<void>("fs_create_dir", { path });
export const fsRename = (from: string, to: string) => invoke<void>("fs_rename", { from, to });
export const fsDelete = (path: string) => invoke<void>("fs_delete", { path });
export const fsReveal = (path: string) => invoke<void>("fs_reveal", { path });
export const fsOpenDefault = (path: string) => invoke<void>("fs_open_default", { path });

// ── Terminal (IDE dock) ───────────────────────────────────
export const ptySpawn = (id: string, cwd: string | null, cols: number, rows: number) =>
  invoke<void>("pty_spawn", { id, cwd, cols, rows });
export const ptyWrite = (id: string, data: string) => invoke<void>("pty_write", { id, data });
export const ptyResize = (id: string, cols: number, rows: number) => invoke<void>("pty_resize", { id, cols, rows });
export const ptyKill = (id: string) => invoke<void>("pty_kill", { id });
export const onPtyOutput = (cb: (e: { id: string; data: string }) => void) =>
  listen<{ id: string; data: string }>("pty-output", (e) => cb(e.payload));
export const onPtyExit = (cb: (e: { id: string }) => void) => listen<{ id: string }>("pty-exit", (e) => cb(e.payload));

// ── Git diff (IDE dock) ───────────────────────────────────
export interface ChangedFile {
  path: string;
  status: string;
  additions: number;
  deletions: number;
}

export const gitStatus = (repoPath: string) => invoke<ChangedFile[]>("git_status", { repoPath });
export const gitDiffFile = (repoPath: string, filePath: string) =>
  invoke<string>("git_diff_file", { repoPath, filePath });

// ── Meeting & interview copilot (src-tauri/src/meeting.rs) ──
export type MeetingMode = "meeting" | "interview" | "practice";
export type MeetingAction =
  | "assist"
  | "insights"
  | "suggest"
  | "summary"
  | "followups"
  | "explain"
  | "notes"
  | "practice_question"
  | "practice_feedback"
  | "screen"
  | "ask";

/** An answer, plus a thumbnail data URL of the screen it looked at (if any). */
export interface MeetingAnswer {
  text: string;
  screen: string | null;
}

export interface SourceNote {
  url: string;
  title: string;
  chars: number;
  error: string;
  fetched_at_ms: number;
}
export interface MeetingProfile {
  about: string;
  projects: string;
  session_context: string;
  style: string;
  /** Links the user added, fetched and read (meeting_research). */
  sources: SourceNote[];
  /** The briefing written from those links; editable. */
  research: string;
  /** Leave the profile out of prompts (inverted so the default is "use it"). */
  skip_context: boolean;
}
/** Fetch the links, read them and write the research brief into the profile. */
export const meetingResearch = (urls: string[]) => invoke<MeetingProfile>("meeting_research", { urls });
export const meetingContextAppend = (field: "about" | "projects" | "session_context" | "research", text: string) =>
  invoke<MeetingProfile>("meeting_context_append", { field, text });
export const onMeetingProfile = (cb: (p: MeetingProfile) => void) =>
  listen<MeetingProfile>("meeting-profile", (e) => cb(e.payload));
export const onMeetingResearchProgress = (cb: (step: string) => void) =>
  listen<string>("meeting-research-progress", (e) => cb(e.payload));

export interface TranscriptEntry {
  id: number;
  speaker: "them" | "me" | "coach" | "note";
  text: string;
  at_ms: number;
}

export interface MeetingStatus {
  live: boolean;
  mode: MeetingMode | "";
  mic: boolean;
  system: boolean;
  started_at_ms: number | null;
  warning: string | null;
  /** Live (streaming) transcription is active - words appear as they are spoken. */
  streaming?: boolean;
  /** Paused with a transcript to continue: Resume restarts capture on the same session. */
  resumable?: boolean;
}

/** The sentence a speaker is in the middle of (live transcription); empty = none. */
export interface TranscriptPartial {
  speaker: "them" | "me";
  text: string;
}
export const onMeetingPartial = (cb: (p: TranscriptPartial) => void) =>
  listen<TranscriptPartial>("meeting-transcript-partial", (e) => cb(e.payload));

export const meetingStart = (mode: MeetingMode, captureMic: boolean, captureSystem: boolean, showBar = true, resume = false) =>
  invoke<MeetingStatus>("meeting_start", { mode, captureMic, captureSystem, showBar, resume });
export const meetingStop = () => invoke<MeetingStatus>("meeting_stop");
export const meetingStatus = () => invoke<MeetingStatus>("meeting_status");
export const meetingTranscript = () => invoke<TranscriptEntry[]>("meeting_transcript");
export const meetingAddEntry = (speaker: "them" | "me" | "note", text: string) =>
  invoke<TranscriptEntry>("meeting_add_entry", { speaker, text });
export const meetingClear = () => invoke<void>("meeting_clear");
/** Pass `requestId` to have the answer streamed as `meeting-answer-delta` events while it is written. */
export const meetingAssist = (
  action: MeetingAction,
  question: string | null = null,
  includeScreen = false,
  requestId: string | null = null,
) => invoke<MeetingAnswer>("meeting_assist", { action, question, includeScreen, requestId });
export const onMeetingAnswerDelta = (cb: (e: { id: string; delta: string }) => void) =>
  listen<{ id: string; delta: string }>("meeting-answer-delta", (e) => cb(e.payload));
export const meetingGetProfile = () => invoke<MeetingProfile>("meeting_get_profile");
export const meetingSetProfile = (profile: MeetingProfile) => invoke<void>("meeting_set_profile", { profile });
export const meetingSaveNotes = (notes: string, includeTranscript: boolean) =>
  invoke<string>("meeting_save_notes", { notes, includeTranscript });
export const onMeetingTranscript = (cb: (e: TranscriptEntry) => void) =>
  listen<TranscriptEntry>("meeting-transcript", (e) => cb(e.payload));
export const onMeetingStatus = (cb: (s: MeetingStatus) => void) =>
  listen<MeetingStatus>("meeting-status", (e) => cb(e.payload));
export const onMeetingLevel = (cb: (l: { mic: number; system: number }) => void) =>
  listen<{ mic: number; system: number }>("meeting-level", (e) => cb(e.payload));
export const onMeetingTranscribing = (cb: (busy: boolean) => void) =>
  listen<boolean>("meeting-transcribing", (e) => cb(e.payload));
export const onMeetingHotkey = (cb: () => void) => listen("meeting-hotkey", () => cb());

// ── In-call copilot bar (copilot window, meeting.rs) ──
export const copilotShow = () => invoke<void>("copilot_show");
export const showMainWindow = () => invoke<void>("show_main_window");
/** The bar's "Open full copilot" → switch the main window to the meeting view. */
export const onOpenMeetingView = (cb: () => void) => listen("open-meeting-view", () => cb());
export const copilotHide = () => invoke<void>("copilot_hide");
export const copilotResize = (width: number, height: number) => invoke<void>("copilot_resize", { width, height });
export const copilotFocus = () => invoke<void>("copilot_focus");
/** Copilot "Fix it": the full desktop agent, progress as agent-progress events with source "copilot". */
export const COPILOT_AGENT_SOURCE = "copilot";
export const copilotAgentRun = (task: string, project: string | null) =>
  invoke<{ text: string }>("copilot_agent_run", { task, project });
/** Paste ("paste") or naturally type ("type") text into the app the user was working in. */
export const copilotInsert = (text: string, mode: "paste" | "type") =>
  invoke<{ completed: boolean }>("copilot_insert", { text, mode });
export const copilotInsertCancel = () => invoke<void>("copilot_insert_cancel");
export const onCopilotAssist = (cb: (action: MeetingAction) => void) =>
  listen<MeetingAction>("copilot-assist", (e) => cb(e.payload));
/** Whether Mouse AI's windows are hidden from screen share / recordings. */
export const stealthGet = () => invoke<boolean>("stealth_get");
export const stealthSet = (enabled: boolean) => invoke<boolean>("stealth_set", { enabled });
/** How Mouse AI sits on the desktop (src-tauri/src/prefs.rs). */
export interface WindowPrefs {
  hide_from_taskbar: boolean;
  start_in_background: boolean;
  /** Every Mouse AI window is excluded from screen capture. Persisted; on by default. */
  hide_from_screen_share: boolean;
}
/** A page the browser extension handed to the copilot bar (bridge.rs `open_copilot`). */
export interface CopilotContext {
  url: string;
  title: string;
  text: string;
  selection: string;
}
export const onCopilotContext = (cb: (c: CopilotContext) => void) =>
  listen<CopilotContext>("copilot-context", (e) => cb(e.payload));
export const windowPrefsGet = () => invoke<WindowPrefs>("window_prefs_get");
export const windowPrefsSet = (prefs: WindowPrefs) => invoke<WindowPrefs>("window_prefs_set", { prefs });
export const onWindowPrefs = (cb: (p: WindowPrefs) => void) => listen<WindowPrefs>("window-prefs", (e) => cb(e.payload));
export const onStealthChanged = (cb: (enabled: boolean) => void) =>
  listen<boolean>("stealth-changed", (e) => cb(e.payload));

// ── Agent link: desktop ⇄ browser agents (src-tauri/src/link.rs) ──
export interface LinkInfo {
  connected: boolean;
  paired: boolean;
  port: number;
  pair_code: string;
  accept_tasks: boolean;
}

export interface LinkStep {
  kind: string;
  message: string;
  at_ms: number;
}

export interface LinkTask {
  id: string;
  direction: "to_browser" | "from_browser";
  title: string;
  status: "queued" | "running" | "waiting" | "stalled" | "done" | "failed" | "cancelled";
  steps: LinkStep[];
  started_ms: number;
  updated_ms: number;
  result: string | null;
  error: string | null;
}

export const linkInfo = () => invoke<LinkInfo>("link_info");
export const linkSetAcceptTasks = (accept: boolean) => invoke<LinkInfo>("link_set_accept_tasks", { accept });
export const linkNewPairCode = () => invoke<LinkInfo>("link_new_pair_code");
export const linkTasks = () => invoke<LinkTask[]>("link_tasks");
export const linkCancelTask = (id: string) => invoke<void>("link_cancel_task", { id });
export const linkClearFinished = () => invoke<LinkTask[]>("link_clear_finished");
export const onLinkTask = (cb: (t: LinkTask) => void) => listen<LinkTask>("bridge-task", (e) => cb(e.payload));
export const onBridgeStatus = (cb: (s: { connected: boolean; paired: boolean; port: number }) => void) =>
  listen<{ connected: boolean; paired: boolean; port: number }>("bridge-status", (e) => cb(e.payload));

export function withTimeout<T>(promise: Promise<T>, ms: number, message = "Request timed out."): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((_, reject) => setTimeout(() => reject(new Error(message)), ms)),
  ]);
}

export function newId(): string {
  return crypto.randomUUID();
}

export type { ChatMessage, User };
