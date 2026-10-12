import type { BotKind } from "../components/ui/BotAvatar";

/** Which bot character stands for which agent, in one place. */
export const AGENT_BOT: Record<"desktop" | "browser" | "meeting" | "helper", BotKind> = {
  desktop: "ghost", // the Thothai logo
  browser: "flower",
  meeting: "clover",
  helper: "square",
};
