import { useEffect, useState } from "react";
import { fsHomeDir } from "../lib/tauri";

// Shared "which folder is the IDE dock pointed at" state — the file tree,
// editor, terminal cwd, and git diff viewer all read the same root instead of
// each independently guessing a directory.
export function useWorkspace() {
  const [root, setRoot] = useState<string | null>(null);

  useEffect(() => {
    fsHomeDir().then(setRoot).catch(() => {});
  }, []);

  return { root, setRoot };
}
