export interface User {
  name?: string;
  email?: string;
  picture?: string;
}

export interface SessionState {
  signed_in: boolean;
  user?: User;
}

export type MessageRole = "user" | "assistant" | "system";

export interface ChatMessage {
  id: string;
  role: MessageRole;
  content: string;
}

export interface Conversation {
  id: string;
  title?: string;
  updated_at: string;
  message_count?: number;
  /** Which Kudu app started it - "desktop" | "extension" | "web" (null on old rows). */
  app?: string | null;
  /** "chat" | "act" | "handoff" (a task one agent gave the other). */
  kind?: string | null;
}

/** GET /api/history/changes: what changed since a cursor (`now` of the previous call). */
export interface HistoryChanges {
  now: string;
  conversations: Conversation[];
  deleted: string[];
}

export interface ConversationMessage {
  id: string;
  role: string;
  content: string;
}

export interface CursorContext {
  process_name?: string;
  window_title?: string;
}

export type AgentProgressKind =
  | "plan"
  | "thinking"
  | "phase-summary"
  | "tool"
  | "tool-result"
  | "error"
  | "confirm"
  | "done";

export interface AgentProgressEvent {
  kind: AgentProgressKind;
  message?: string;
  request_id?: string;
  source?: string;
}

export interface AgentRunResult {
  text?: string;
  /** Backend conversation the run was logged to (lets a background chat
   *  session be matched to its sidebar thread). */
  conversation_id?: string | null;
}

export interface HistoryStats {
  conversations: number;
  messages: number;
}
