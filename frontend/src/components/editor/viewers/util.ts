import { useEffect, useRef, useState } from "react";

export function formatSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** 1 -> "A", 27 -> "AA" (spreadsheet column letters). */
export function columnName(n: number) {
  let s = "";
  while (n > 0) {
    const m = (n - 1) % 26;
    s = String.fromCharCode(65 + m) + s;
    n = Math.floor((n - 1) / 26);
  }
  return s;
}

export function errorText(err: unknown) {
  return err instanceof Error ? err.message : String(err);
}

export function cannotOpenDetail(error: string) {
  return /denied|os error 5/i.test(error) ? "Windows won't let apps read it (it's protected or in use)." : error;
}

/** Status-bar items a viewer reports up to the Editor (dimensions, counts…). */
export type StatusReporter = (items: string[]) => void;

/** Loads something for `key` (a file path), dropping stale results. */
export function useLoaded<T>(key: string, load: (key: string) => Promise<T>) {
  const [state, setState] = useState<{ key: string; data: T | null; error: string | null }>({ key: "", data: null, error: null });
  const loadRef = useRef(load);
  loadRef.current = load;
  useEffect(() => {
    let cancelled = false;
    loadRef
      .current(key)
      .then((data) => !cancelled && setState({ key, data, error: null }))
      .catch((err) => !cancelled && setState({ key, data: null, error: errorText(err) }));
    return () => {
      cancelled = true;
    };
  }, [key]);
  const fresh = state.key === key;
  return { data: fresh ? state.data : null, error: fresh ? state.error : null, loading: !fresh };
}
