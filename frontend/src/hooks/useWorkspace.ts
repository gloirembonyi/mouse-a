import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { fsHomeDir } from "../lib/tauri";

export interface Project {
  path: string;
  name: string;
}

const PROJECTS_KEY = "mouse-ai-projects";
const ACTIVE_KEY = "mouse-ai-active-project";

function baseName(path: string) {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
}

function loadProjects(): Project[] {
  try {
    const raw = JSON.parse(localStorage.getItem(PROJECTS_KEY) ?? "[]");
    return Array.isArray(raw) ? raw.filter((p) => p && typeof p.path === "string") : [];
  } catch {
    return [];
  }
}

// The folders the user works in ("projects", like Codex). The active project
// is shared by everything that needs a folder - the IDE dock's file tree,
// editor, terminal cwd and git changes, plus the agent, which is told to work
// inside it. With no project selected the dock falls back to the home folder.
export function useWorkspace() {
  const [projects, setProjects] = useState<Project[]>(loadProjects);
  const [activePath, setActivePath] = useState<string | null>(() => localStorage.getItem(ACTIVE_KEY));
  const [home, setHome] = useState<string | null>(null);

  useEffect(() => {
    fsHomeDir().then(setHome).catch(() => {});
  }, []);

  useEffect(() => {
    localStorage.setItem(PROJECTS_KEY, JSON.stringify(projects));
  }, [projects]);

  useEffect(() => {
    if (activePath) localStorage.setItem(ACTIVE_KEY, activePath);
    else localStorage.removeItem(ACTIVE_KEY);
  }, [activePath]);

  const addPath = useCallback((path: string) => {
    const clean = path.trim();
    if (!clean) return;
    setProjects((prev) =>
      prev.some((p) => p.path.toLowerCase() === clean.toLowerCase()) ? prev : [...prev, { path: clean, name: baseName(clean) }],
    );
    setActivePath(clean);
  }, []);

  /** Opens the native folder picker; resolves to the chosen path (or null). */
  const addFolder = useCallback(async () => {
    const picked = await open({ directory: true, multiple: false, title: "Add a project folder" });
    if (typeof picked === "string") {
      addPath(picked);
      return picked;
    }
    return null;
  }, [addPath]);

  const removeProject = useCallback((path: string) => {
    setProjects((prev) => prev.filter((p) => p.path !== path));
    setActivePath((cur) => (cur === path ? null : cur));
  }, []);

  const activeProject = projects.find((p) => p.path === activePath) ?? null;

  return {
    projects,
    activeProject,
    selectProject: setActivePath,
    addFolder,
    addPath,
    removeProject,
    /** Folder the IDE dock is pointed at. */
    root: activeProject?.path ?? home,
    setRoot: addPath,
  };
}
