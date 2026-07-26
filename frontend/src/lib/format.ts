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
  if (/429|rate limit/i.test(msg)) return "Too many requests right now — wait a few seconds and try again.";
  return msg;
}
