import { useEffect, useState } from "react";
import { Keyboard, Info, LogOut, Network, Power, Puzzle, User as UserIcon, X } from "lucide-react";
import {
  getBackendUrl,
  getMcpServers,
  quitApp,
  setBackendUrl,
  setMcpServers,
  signOut,
  testBackend,
  withTimeout,
} from "../../lib/tauri";
import type { User } from "../../lib/types";

interface SettingsPanelProps {
  user?: User;
  onClose: () => void;
  onSignedOut: () => void;
}

export function SettingsPanel({ user, onClose, onSignedOut }: SettingsPanelProps) {
  const [backendUrl, setBackendUrlInput] = useState("");
  const [backendStatus, setBackendStatus] = useState("");
  const [mcpJson, setMcpJson] = useState("");
  const [mcpStatus, setMcpStatus] = useState("");

  useEffect(() => {
    getBackendUrl().then(setBackendUrlInput).catch(() => {});
    getMcpServers()
      .then((json) => {
        if (json && json.trim() !== "[]") setMcpJson(json);
      })
      .catch(() => {});
  }, []);

  const handleSaveBackendUrl = async () => {
    if (!backendUrl.trim()) return;
    try {
      const saved = await setBackendUrl(backendUrl.trim());
      setBackendStatus(`✓ Saved: ${saved}`);
    } catch (err) {
      setBackendStatus(`Failed to save: ${err}`);
    }
  };

  const handleTestBackend = async () => {
    setBackendStatus("Testing connection…");
    try {
      if (backendUrl.trim()) await setBackendUrl(backendUrl.trim());
      const base = await withTimeout(testBackend(), 15000);
      setBackendStatus(`✓ Connected to ${base}`);
    } catch (err) {
      setBackendStatus(`✗ ${err instanceof Error ? err.message : String(err)}`);
    }
  };

  const handleSaveMcp = async () => {
    try {
      const msg = await setMcpServers(mcpJson || "[]");
      setMcpStatus(`✓ ${msg} MCP tools load next time the agent runs.`);
    } catch (err) {
      setMcpStatus(`✗ ${err instanceof Error ? err.message : String(err)}`);
    }
  };

  const handleSignOut = async () => {
    try {
      await signOut();
      onSignedOut();
    } catch {
      /* ignore */
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-shrink-0 items-center justify-between border-b border-border-default px-3.5 py-3">
        <h2 className="text-base font-semibold">Settings</h2>
        <button onClick={onClose} className="rounded-md p-1.5 text-text-secondary hover:bg-bg-hover hover:text-text-primary">
          <X size={16} />
        </button>
      </div>

      <div className="scrollbar-thin flex flex-1 flex-col gap-3.5 overflow-y-auto p-4">
        <Section icon={<UserIcon size={14} />} title="Account">
          <div className="flex items-center gap-3 rounded-lg border border-border-default bg-bg-secondary p-2.5">
            {user?.picture ? (
              <img src={user.picture} alt="" className="h-10 w-10 flex-shrink-0 rounded-full border border-border-default object-cover" />
            ) : (
              <div className="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-full border border-border-default bg-gradient-to-br from-accent-soft to-transparent text-[17px]">
                🖱️
              </div>
            )}
            <div className="min-w-0 flex-1">
              <div className="truncate text-[13.5px] font-semibold">{user?.name || user?.email || "Guest"}</div>
              <div className="truncate text-[11.5px] text-text-muted">{user?.email || ""}</div>
            </div>
            <button
              onClick={handleSignOut}
              className="flex flex-shrink-0 items-center gap-1.5 rounded-md border border-border-default px-3 py-1.5 text-xs text-text-secondary transition hover:border-danger/40 hover:bg-danger-soft hover:text-danger"
            >
              <LogOut size={13} />
              Sign Out
            </button>
          </div>
        </Section>

        <Section icon={<Keyboard size={14} />} title="Shortcuts">
          <div className="flex flex-col gap-1.5">
            <ShortcutRow keys="Ctrl+Shift+Space" label="Ask at Cursor (anywhere)" />
            <ShortcutRow keys="Ctrl+Shift+Q" label="Toggle Hover Mode" />
            <ShortcutRow keys="Ctrl+Shift+S" label="Screenshot Analyze" />
            <ShortcutRow keys="Esc" label="Dismiss" />
          </div>
        </Section>

        <Section icon={<Network size={14} />} title="Backend URL">
          <input
            value={backendUrl}
            onChange={(e) => setBackendUrlInput(e.target.value)}
            placeholder="https://backend-thothai.vercel.app"
            className="w-full rounded-md border border-border-default bg-bg-tertiary px-3 py-2 text-[13px] outline-none focus:border-border-focus"
          />
          <div className="flex gap-2">
            <button onClick={handleSaveBackendUrl} className="flex-1 rounded-md border border-border-default bg-bg-tertiary py-2 text-xs font-medium transition hover:border-accent hover:bg-accent-soft">
              Save
            </button>
            <button onClick={handleTestBackend} className="flex-1 rounded-md border border-border-default bg-bg-tertiary py-2 text-xs font-medium transition hover:border-accent hover:bg-accent-soft">
              Test connection
            </button>
          </div>
          {backendStatus && <p className="text-xs text-text-muted">{backendStatus}</p>}
        </Section>

        <Section icon={<Puzzle size={14} />} title="MCP servers (advanced)">
          <p className="text-[11.5px] text-text-muted">
            A JSON array of stdio MCP servers. Their tools are added to the agent automatically. Each entry: <code>name</code>,{" "}
            <code>command</code>, <code>args</code>, optional <code>env</code>.
          </p>
          <textarea
            value={mcpJson}
            onChange={(e) => setMcpJson(e.target.value)}
            rows={6}
            spellCheck={false}
            placeholder={`[\n  { "name": "filesystem", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "C:/Users/you"] }\n]`}
            className="w-full rounded-md border border-border-default bg-bg-tertiary px-3 py-2 font-mono text-[12px] outline-none focus:border-border-focus"
          />
          <button onClick={handleSaveMcp} className="rounded-md border border-border-default bg-bg-tertiary py-2 text-xs font-medium transition hover:border-accent hover:bg-accent-soft">
            Save MCP servers
          </button>
          {mcpStatus && <p className="text-xs text-text-muted">{mcpStatus}</p>}
        </Section>

        <Section icon={<Info size={14} />} title="About">
          <p className="text-xs text-text-muted">Mouse AI v0.2.0 — Powered by Thothai &amp; Gemini</p>
          <p className="text-xs text-text-muted">
            The ✕ button minimizes the window to your system tray icon rather than exiting — Mouse AI keeps listening for its
            shortcuts in the background. Use the button below to fully quit.
          </p>
          <button
            onClick={() => quitApp().catch(() => {})}
            className="flex w-full items-center justify-center gap-1.5 rounded-md border border-border-default px-3 py-2 text-xs text-text-secondary transition hover:border-danger/40 hover:bg-danger-soft hover:text-danger"
          >
            <Power size={13} />
            Quit Mouse AI
          </button>
        </Section>
      </div>
    </div>
  );
}

function Section({ icon, title, children }: { icon: React.ReactNode; title: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-2.5 rounded-xl border border-border-soft bg-bg-card p-3.5">
      <label className="flex items-center gap-2 text-xs font-semibold text-text-primary">
        <span className="text-accent">{icon}</span>
        {title}
      </label>
      {children}
    </div>
  );
}

function ShortcutRow({ keys, label }: { keys: string; label: string }) {
  return (
    <div className="flex items-center justify-between rounded-md border border-transparent bg-bg-secondary px-3 py-2 text-xs transition hover:border-border-default">
      <kbd className="rounded border border-border-default bg-bg-tertiary px-1.5 py-0.5 font-sans text-[11px] text-text-secondary">
        {keys}
      </kbd>
      <span className="text-text-secondary">{label}</span>
    </div>
  );
}
