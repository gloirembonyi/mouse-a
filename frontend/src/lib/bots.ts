import type { BotKind } from "../components/ui/BotAvatar";

/** Which character stands for which agent, in one place. */
export const AGENT_BOT: Record<"desktop" | "browser" | "meeting" | "helper", BotKind> = {
  desktop: "kudu", // Kudu Desktop - the kudu (the logo)
  browser: "flower", // Kudu Connect, the browser agent
  meeting: "clover", // Kudu Pilot, the meeting copilot
  helper: "square", // background helpers
};
