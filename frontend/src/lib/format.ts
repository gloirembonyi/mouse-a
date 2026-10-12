// Coarse relative-time label ("just now" / "5m ago" / "3h ago" / "2d ago" / a
// plain date beyond that) — thread rows don't need second-level precision.
export function relativeTime(isoString: string): string {
  const then = new Date(isoString).getTime();
  if (Number.isNaN(then)) return "";
  const diffMs = Date.now() - then;
  const minutes = Math.floor(diffMs / 60000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(isoString).toLocaleDateString();
}

// "4m 28s" / "12s" — Codex-style run-duration footer.
export function formatDuration(ms: number): string {
  const totalSeconds = Math.max(1, Math.round(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return minutes > 0 ? `${minutes}m ${seconds}s` : `${seconds}s`;
}

// Turns a raw Rust/backend error string into something a non-technical user
// can act on.
export function friendlyError(err: unknown): string {
  const msg = err instanceof Error ? err.message : String(err);
  if (/not signed in/i.test(msg)) return "You're not signed in. Open Settings to sign in first.";
  if (/can't reach|network|timed out|connect/i.test(msg))
    return "Can't reach the Thothai backend. Check your internet or the Backend URL in Settings.";
  if (/empty response/i.test(msg))
    return "The model didn't respond after several tries — it may be under heavy load. Try again in a moment.";
  if (/step limit/i.test(msg))
    return "That task took more steps than I could finish in one go — try breaking it into smaller steps.";
  if (/daily message limit|daily limit/i.test(msg))
    return "You've hit today's free message limit. It resets daily — try again tomorrow.";
  if (/referer|permission_denied|rejected every configured api key|api key not valid/i.test(msg))
    return "The backend's Gemini API key is misconfigured (not something you can fix here) — let the app owner know it needs a working key.";
  // Distinct from a transient rate limit: a quota/billing error means the
  // account's Gemini quota is actually exhausted — retrying (which the agent
  // already tried several times before giving up) can't fix it, so this
  // needs its own message rather than the generic "wait a few seconds" one.
  if (/exceeded your current quota|check your plan and billing|quota_exceeded|resource_exhausted/i.test(msg))
    return "The Gemini API quota/billing limit has been reached — this won't resolve by retrying. Check the plan/billing for the API key in use, or switch to a different key in Settings.";
  if (/429|rate limit/i.test(msg)) return "Too many requests right now — wait a few seconds and try again.";
  return msg;
}

// Thread titles come from the first message, and hover-mode threads all start
// with the same prompt boilerplate ("The user is hovering over this …") —
// strip it so the sidebar shows what the thread was actually about.
export function threadTitle(raw?: string): string {
  const t = (raw ?? "").trim();
  if (!t) return "Untitled";
  // The boilerplate is followed by "…on their screen and asked: <question>"
  // when the question made it into the title; otherwise there's nothing
  // meaningful left to show.
  if (/^the user is hovering over/i.test(t)) {
    const asked = /asked[:\s]+["“]?([^"”]+)/i.exec(t)?.[1]?.trim();
    return asked ? `Hover · ${asked}` : "Hover question";
  }
  if (/^thothai chat$/i.test(t)) return "New chat";
  return t;
}

export type DateBucket = "Today" | "Yesterday" | "Previous 7 days" | "Previous 30 days" | "Older";

export function dateBucket(isoString: string): DateBucket {
  const then = new Date(isoString);
  if (Number.isNaN(then.getTime())) return "Older";
  const startOfToday = new Date();
  startOfToday.setHours(0, 0, 0, 0);
  const day = 86400000;
  const diff = startOfToday.getTime() - then.getTime();
  if (diff <= 0) return "Today";
  if (diff <= day) return "Yesterday";
  if (diff <= 7 * day) return "Previous 7 days";
  if (diff <= 30 * day) return "Previous 30 days";
  return "Older";
}
