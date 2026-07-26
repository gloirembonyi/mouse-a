import { useCallback, useEffect, useState } from "react";
import { ensureSession, verifySession, withTimeout } from "../lib/tauri";
import type { User } from "../lib/types";

export type AuthStatus = "connecting" | "signed-in" | "error";

export function useAuth() {
  const [status, setStatus] = useState<AuthStatus>("connecting");
  const [user, setUser] = useState<User | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);

  const connect = useCallback(async () => {
    setStatus("connecting");
    setError(null);
    try {
      const state = await withTimeout(ensureSession(), 20000);
      if (state.signed_in && state.user) {
        setUser(state.user);
        setStatus("signed-in");
        // Best-effort background revalidation of a real (non-guest) session.
        verifySession().catch(() => {});
        return;
      }
      throw new Error("Couldn't start a session.");
    } catch (err) {
      setError(`Can't reach the Thothai backend: ${err instanceof Error ? err.message : String(err)}`);
      setStatus("error");
    }
  }, []);

  useEffect(() => {
    connect();
  }, [connect]);

  const onSignedIn = useCallback((u: User) => {
    setUser(u);
    setStatus("signed-in");
  }, []);

  return { status, user, error, retry: connect, onSignedIn };
}
