import { useState } from "react";
import { ThinkingOrb } from "thinking-orbs";
import { signInWithToken, startGoogleSignIn, withTimeout } from "../../lib/tauri";
import { BotAvatar } from "../ui/BotAvatar";
import type { User } from "../../lib/types";

interface AuthGateProps {
  status: "connecting" | "signed-out" | "error";
  error: string | null;
  onRetry: () => void;
  onSignedIn: (user: User) => void;
}

export function AuthGate({ status, error, onRetry, onSignedIn }: AuthGateProps) {
  const showGoogle = status !== "connecting";
  const [googleBusy, setGoogleBusy] = useState(false);
  const [googleStatus, setGoogleStatus] = useState<string | null>(null);
  const [googleError, setGoogleError] = useState<string | null>(null);
  const [showTokenInput, setShowTokenInput] = useState(false);
  const [token, setToken] = useState("");

  const handleGoogleSignIn = async () => {
    setGoogleBusy(true);
    setGoogleStatus("Waiting for you to finish signing in in your browser…");
    setGoogleError(null);
    try {
      const result = await withTimeout(startGoogleSignIn(), 250000);
      if (result.signed_in && result.user) {
        onSignedIn(result.user);
      } else {
        setGoogleError("Sign in failed - please try again.");
      }
    } catch (err) {
      setGoogleError(err instanceof Error ? err.message : String(err));
      setShowTokenInput(true);
    } finally {
      setGoogleBusy(false);
      setGoogleStatus(null);
    }
  };

  const handleTokenSignIn = async () => {
    const extracted = extractAccessToken(token);
    if (!extracted) {
      setGoogleError("Please enter your access token");
      return;
    }
    try {
      const result = await withTimeout(signInWithToken(extracted), 30000);
      if (result.signed_in && result.user) {
        onSignedIn(result.user);
      } else {
        setGoogleError("Sign in failed. Please check your token.");
      }
    } catch (err) {
      setGoogleError(`Sign in error: ${err instanceof Error ? err.message : String(err)}`);
    }
  };

  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-4 px-6 py-10 text-center">
      <BotAvatar kind="kudu" mood="idle" size={84} variant="body" className="mb-1" />
      <h2 className="text-[22px] font-semibold tracking-[-0.02em]">Welcome to Kudu Desktop</h2>
      <p className="max-w-[320px] text-[13px] text-text-muted">
        Sign in with your Google account to use Kudu Lens, Kudu Pilot and Kudu Agent. Your plan, history and
        Kudu Connect link follow your account.
      </p>

      {status === "connecting" && (
        <div className="flex items-center gap-2 text-[12.5px]">
          <ThinkingOrb state="connecting" size={20} theme="auto" aria-label="Connecting" />
          <span className="shimmer-text font-medium">Connecting to Kudu Core…</span>
        </div>
      )}
      {error && <div className="w-full max-w-[300px] rounded-md bg-danger-soft px-3 py-2 text-xs text-danger">{error}</div>}
      {status === "error" && (
        <button onClick={onRetry} className="text-[12px] text-text-muted underline">
          Retry the connection
        </button>
      )}

      <div className="flex w-full max-w-[280px] flex-col gap-1.5">
        <Hint keys="Ctrl+Shift+Space" label="Ask Kudu at your cursor" />
        <Hint keys="Ctrl+Shift+Q" label="Toggle Kudu Lens" />
      </div>

      {showGoogle && (
        <div className="flex w-full max-w-[280px] flex-col gap-2 pt-2">
          <button
            onClick={handleGoogleSignIn}
            disabled={googleBusy}
            className="flex w-full items-center justify-center gap-2 rounded-full btn-brand py-3 text-[14px] font-semibold disabled:opacity-60"
          >
            <GoogleG /> {googleBusy ? "Finish in your browser…" : "Continue with Google"}
          </button>
          <p className="text-[11.5px] text-text-muted">
            Already signed in to Kudu Connect in your browser? Pair it in Settings and Kudu Desktop signs in with the same account.
          </p>
          {googleStatus && <div className="text-[11px] text-text-muted">{googleStatus}</div>}
          {googleError && <div className="rounded-md bg-danger-soft px-3 py-2 text-xs text-danger">{googleError}</div>}
          {showTokenInput && (
            <div className="flex flex-col gap-2 border-t border-border-default pt-3">
              <p className="text-left text-[11px] text-text-muted">
                Or paste a Google access token - a raw token or a full redirect URL containing{" "}
                <code className="rounded bg-bg-tertiary px-1">access_token=…</code> both work:
              </p>
              <input
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="Access token or redirect URL…"
                className="w-full rounded-md border border-border-default bg-bg-tertiary px-3 py-2 text-[13px] text-text-primary outline-none focus:border-border-focus"
              />
              <button
                onClick={handleTokenSignIn}
                className="w-full rounded-lg border border-border-default bg-bg-tertiary py-2.5 text-[13px] font-medium transition hover:border-accent hover:bg-accent-soft"
              >
                Sign In
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function Hint({ keys, label }: { keys: string; label: string }) {
  return (
    <div className="flex items-center justify-between rounded-lg border border-border-soft bg-bg-card px-3 py-1.5 text-xs text-text-muted">
      <kbd className="rounded border border-border-default bg-bg-tertiary px-1.5 py-0.5 font-sans text-[11px] text-text-secondary">
        {keys}
      </kbd>
      <span>{label}</span>
    </div>
  );
}

function extractAccessToken(raw: string): string {
  const text = raw.trim();
  if (!text) return "";
  const match = text.match(/access_token=([^&\s]+)/);
  if (match) {
    try {
      return decodeURIComponent(match[1]);
    } catch {
      return match[1];
    }
  }
  return text;
}

function GoogleG() {
  return (
    <svg width="16" height="16" viewBox="0 0 48 48" aria-hidden="true">
      <path fill="#FFC107" d="M43.6 20.1H42V20H24v8h11.3C33.7 32.7 29.1 36 24 36c-6.6 0-12-5.4-12-12s5.4-12 12-12c3.1 0 5.8 1.2 7.9 3.1l5.7-5.7C34 6.1 29.3 4 24 4 12.9 4 4 12.9 4 24s8.9 20 20 20 20-8.9 20-20c0-1.3-.1-2.6-.4-3.9z" />
      <path fill="#FF3D00" d="M6.3 14.7l6.6 4.8C14.7 15.1 19 12 24 12c3.1 0 5.8 1.2 7.9 3.1l5.7-5.7C34 6.1 29.3 4 24 4 16.3 4 9.7 8.3 6.3 14.7z" />
      <path fill="#4CAF50" d="M24 44c5.2 0 9.9-2 13.4-5.2l-6.2-5.2C29.2 35.1 26.7 36 24 36c-5.2 0-9.6-3.3-11.3-7.9l-6.5 5C9.5 39.6 16.2 44 24 44z" />
      <path fill="#1976D2" d="M43.6 20.1H42V20H24v8h11.3c-.8 2.2-2.2 4.2-4.1 5.6l6.2 5.2C37 39.2 44 34 44 24c0-1.3-.1-2.6-.4-3.9z" />
    </svg>
  );
}
