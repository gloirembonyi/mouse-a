import type { AgentStep } from "../components/chat/AgentActivity";

export type Attachment =
  | { id: string; kind: "image"; dataUrl: string; name: string }
  | { id: string; kind: "file"; path: string; name: string }
  | { id: string; kind: "conversation"; conversationId: string; title: string };

export type UiMessage =
  | { id: string; kind: "user" | "assistant" | "system"; content: string; attachments?: Attachment[] }
  | { id: string; kind: "thinking" }
  | { id: string; kind: "agent"; steps: AgentStep[]; status: "running" | "done" | "failed"; elapsedMs?: number };
