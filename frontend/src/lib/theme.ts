import { useSyncExternalStore } from "react";

// Light/dark appearance for the main window. The preference ("system" |
// "light" | "dark") persists in localStorage; the *resolved* theme is written
// to <html data-theme>, which index.css keys every color token off. Kept as a
// tiny external store (not React context) so non-React consumers - CodeMirror
// themes, xterm - can read and subscribe to it too.
export type ThemePref = "system" | "light" | "dark";
export type Theme = "light" | "dark";

const KEY = "mouse-ai-theme";
const media = window.matchMedia("(prefers-color-scheme: light)");
const listeners = new Set<() => void>();

function readPref(): ThemePref {
  const v = localStorage.getItem(KEY);
  return v === "light" || v === "dark" || v === "system" ? v : "dark";
}

let pref: ThemePref = readPref();

function resolve(p: ThemePref): Theme {
  if (p === "system") return media.matches ? "light" : "dark";
  return p;
}

let resolved: Theme = resolve(pref);

function apply() {
  resolved = resolve(pref);
  document.documentElement.dataset.theme = resolved;
  listeners.forEach((l) => l());
}

media.addEventListener("change", () => {
  if (pref === "system") apply();
});

/** Call once before the first render so there's no dark→light flash. */
export function initTheme() {
  apply();
}

export function setThemePref(next: ThemePref) {
  pref = next;
  localStorage.setItem(KEY, next);
  apply();
}

export function toggleTheme() {
  setThemePref(resolved === "dark" ? "light" : "dark");
}

function subscribe(cb: () => void) {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function useTheme() {
  const theme = useSyncExternalStore(subscribe, () => resolved);
  const themePref = useSyncExternalStore(subscribe, () => pref);
  return { theme, themePref, setThemePref, toggleTheme };
}
