import { useState } from "react";
import { ThinkingOrb } from "thinking-orbs";
import { signInWithToken, startGoogleSignIn, withTimeout } from "../../lib/tauri";
import { BotAvatar } from "../ui/BotAvatar";
import type { User } from "../../lib/types";

interface AuthGateProps {
  status: "connecting" | "error";
  error: string | null;
  onRetry: () => void;
  onSignedIn: (user: User) => void;
}

export function AuthGate({ status, error, onRetry, onSignedIn }: AuthGateProps) {
  const [showGoogle, setShowGoogle] = useState(false);
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
      <p className="max-w-[300px] text-[13px] text-text-muted">
        Your desktop AI assistant by Kudu. Hover, capture, and ask anything.
      </p>

      <div className="flex w-full max-w-[260px] flex-col gap-1.5">
        <Hint keys="Ctrl+Shift+Space" label="Ask at Cursor" />
        <Hint keys="Ctrl+Shift+Q" label="Toggle Kudu Lens" />
        <Hint keys="Ctrl+Shift+S" label="Screenshot Analyze" />
      </div>

      {status === "connecting" && (
        <div className="flex items-center gap-2 text-[12.5px]">
          <ThinkingOrb state="connecting" size={20} theme="auto" aria-label="Connecting" />
          <span className="shimmer-text font-medium">Connecting to Kudu Core…</span>
        </div>
      )}
      {error && <div className="w-full rounded-md bg-danger-soft px-3 py-2 text-xs text-danger">{error}</div>}
      {status === "error" && (
        <button
          onClick={onRetry}
          className="w-full max-w-[260px] rounded-full btn-brand py-2.5 text-[13px] font-medium"
        >
          Retry
        </button>
      )}

      {status === "error" && !showGoogle && (
        <button onClick={() => setShowGoogle(true)} className="text-[11px] text-text-muted underline">
          Advanced: sign in with a Google account instead
        </button>
      )}

      {showGoogle && (
        <div className="flex w-full max-w-[260px] flex-col gap-2 border-t border-border-default pt-4">
          <button
            onClick={handleGoogleSignIn}
            disabled={googleBusy}
            className="w-full rounded-lg border border-border-default bg-bg-tertiary py-2.5 text-[13px] font-medium text-text-primary transition hover:border-accent hover:bg-accent-soft disabled:opacity-60"
          >
            Sign in with Google
          </button>
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
