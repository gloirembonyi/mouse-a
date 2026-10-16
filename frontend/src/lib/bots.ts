import type { BotKind } from "../components/ui/BotAvatar";

/** Which Kudu character stands for which agent, in one place. */
export const AGENT_BOT: Record<"desktop" | "browser" | "meeting" | "helper", BotKind> = {
  desktop: "kudu", // the brand kudu - Kudu Desktop itself
  browser: "connect", // Kudu Connect, the browser agent
  meeting: "pilot", // Kudu Pilot, the meeting copilot
  helper: "agent", // background workers - Kudu Agent
};
