import type { AgentStep } from "../components/chat/AgentActivity";

export type UiMessage =
  | { id: string; kind: "user" | "assistant" | "system"; content: string }
  | { id: string; kind: "thinking" }
  | { id: string; kind: "agent"; steps: AgentStep[]; status: "running" | "done" | "failed"; elapsedMs?: number };
