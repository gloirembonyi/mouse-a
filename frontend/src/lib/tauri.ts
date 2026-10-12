// Typed wrappers around the Rust commands in src-tauri/src/commands.rs.
// Keeping every invoke() call behind a named function here means a renamed
// or reshaped Rust command only needs updating in one place.
import { invoke } from "@tauri-apps/api/core";
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
export const agentRun = (task: string, source: "main" | "overlay" = "main", imageDataUrl: string | null = null) =>
  invoke<AgentRunResult>("agent_run", { task, source, autonomous: true, imageDataUrl });

export const agentCancel = (source: "main" | "overlay" = "main") => invoke<void>("agent_cancel", { source });

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

// ── Terminal (IDE dock) ───────────────────────────────────
export const ptySpawn = (cwd: string | null, cols: number, rows: number) =>
  invoke<void>("pty_spawn", { cwd, cols, rows });
export const ptyWrite = (data: string) => invoke<void>("pty_write", { data });
export const ptyResize = (cols: number, rows: number) => invoke<void>("pty_resize", { cols, rows });
export const onPtyOutput = (cb: (data: string) => void) => listen<string>("pty-output", (e) => cb(e.payload));
export const onPtyExit = (cb: () => void) => listen("pty-exit", () => cb());

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
  | "suggest"
  | "summary"
  | "followups"
  | "explain"
  | "notes"
  | "practice_question"
  | "practice_feedback"
  | "ask";

export interface MeetingProfile {
  about: string;
  projects: string;
  session_context: string;
  style: string;
}

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
}

export const meetingStart = (mode: MeetingMode, captureMic: boolean, captureSystem: boolean) =>
  invoke<MeetingStatus>("meeting_start", { mode, captureMic, captureSystem });
export const meetingStop = () => invoke<MeetingStatus>("meeting_stop");
export const meetingStatus = () => invoke<MeetingStatus>("meeting_status");
export const meetingTranscript = () => invoke<TranscriptEntry[]>("meeting_transcript");
export const meetingAddEntry = (speaker: "them" | "me" | "note", text: string) =>
  invoke<TranscriptEntry>("meeting_add_entry", { speaker, text });
export const meetingClear = () => invoke<void>("meeting_clear");
export const meetingAssist = (action: MeetingAction, question: string | null = null, includeScreen = false) =>
  invoke<string>("meeting_assist", { action, question, includeScreen });
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
