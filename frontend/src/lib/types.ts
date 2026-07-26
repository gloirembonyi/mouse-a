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
  source?: "main" | "overlay";
}

export interface AgentRunResult {
  text?: string;
}

export interface HistoryStats {
  conversations: number;
  messages: number;
}
