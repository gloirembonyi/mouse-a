import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import clsx from "clsx";
import { LogOut, Monitor, Moon, Power, Sun, X } from "lucide-react";
import {
  getMcpServers,
  quitApp,
  setMcpServers,
  signOut,
} from "../../lib/tauri";
import { DesktopSection } from "./DesktopSection";
import { UpdateSection } from "./UpdateSection";
import { useTheme, type ThemePref } from "../../lib/theme";
import type { User } from "../../lib/types";
import { IconButton, Kbd } from "../ui/primitives";
import { LinkCard } from "../agents/AgentsView";
import type { useAgents } from "../../hooks/useAgents";

interface SettingsPanelProps {
  user?: User;
  agents: ReturnType<typeof useAgents>;
  onClose: () => void;
  onSignedOut: () => void;
}

const SHORTCUTS: { keys: string[]; label: string }[] = [
  { keys: ["Ctrl", "Shift", "Space"], label: "Kudu Lens: ask at cursor (anywhere)" },
  { keys: ["Ctrl", "Shift", "Q"], label: "Toggle Kudu Lens" },
  { keys: ["Ctrl", "Shift", "S"], label: "Screenshot & analyze" },
  { keys: ["Ctrl", "Shift", "Enter"], label: "Kudu Pilot: what should I say?" },
  { keys: ["Ctrl", "Shift", "\\"], label: "Show / hide the Pilot bar (anywhere)" },
  { keys: ["Ctrl", "N"], label: "New chat" },
  { keys: ["Ctrl", "K"], label: "Search chats" },
  { keys: ["Ctrl", "B"], label: "Toggle sidebar" },
  { keys: ["Ctrl", "J"], label: "Toggle editor / terminal" },
  { keys: ["Ctrl", "\\"], label: "Hide / show the chat (full-width editor)" },
  { keys: ["Ctrl", "P"], label: "Go to file (in the editor)" },
  { keys: ["Ctrl", "Shift", "E"], label: "Hide / show the editor's file tree" },
  { keys: ["Esc"], label: "Dismiss" },
];

export function SettingsPanel({ user, agents, onClose, onSignedOut }: SettingsPanelProps) {
  const { themePref, setThemePref } = useTheme();
  const [mcpJson, setMcpJson] = useState("");
  const [mcpStatus, setMcpStatus] = useState<{ ok: boolean; text: string } | null>(null);
  const [appVersion, setAppVersion] = useState("0.0.0");

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
    getMcpServers()
      .then((json) => {
        if (json && json.trim() !== "[]") setMcpJson(json);
      })
      .catch(() => {});
  }, []);

  const handleSaveMcp = async () => {
    try {
      const msg = await setMcpServers(mcpJson || "[]");
      setMcpStatus({ ok: true, text: `${msg} MCP tools load next time the agent runs.` });
    } catch (err) {
      setMcpStatus({ ok: false, text: err instanceof Error ? err.message : String(err) });
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

  const themes: { id: ThemePref; label: string; icon: React.ReactNode }[] = [
    { id: "system", label: "System", icon: <Monitor size={14} /> },
    { id: "light", label: "Light", icon: <Sun size={14} /> },
    { id: "dark", label: "Dark", icon: <Moon size={14} /> },
  ];

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-12 flex-shrink-0 items-center justify-between px-6">
        <h2 className="text-[15px] font-semibold">Settings</h2>
        <IconButton title="Close settings" onClick={onClose}>
          <X size={16} />
        </IconButton>
      </div>

      <div className="scrollbar-thin flex-1 overflow-y-auto px-6 pb-10">
        <div className="mx-auto flex max-w-[640px] flex-col gap-8 pt-2">
          <Section title="Account">
            <div className="flex items-center gap-3">
              {user?.picture ? (
                <img src={user.picture} alt="" className="h-10 w-10 flex-shrink-0 rounded-full object-cover" />
              ) : (
                <span className="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-full bg-accent-soft text-[15px] font-semibold text-accent">
                  {(user?.name || user?.email || "G").slice(0, 1).toUpperCase()}
                </span>
              )}
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13.5px] font-medium">{user?.name || user?.email || "Guest"}</div>
                <div className="truncate text-[12px] text-text-muted">{user?.email || "Not signed in with Google"}</div>
              </div>
              <Button onClick={handleSignOut} danger>
                <LogOut size={13} />
                Sign out
              </Button>
            </div>
          </Section>

          <DesktopSection />

          <Section title="Appearance" description="Applies to the main window. The hover pill adapts to whatever is behind it.">
            <div className="grid grid-cols-3 gap-2">
              {themes.map((t) => (
                <button
                  key={t.id}
                  onClick={() => setThemePref(t.id)}
                  className={clsx(
                    "flex flex-col overflow-hidden rounded-xl border text-left transition",
                    themePref === t.id ? "border-accent ring-2 ring-accent-soft" : "border-border-default hover:border-border-strong",
                  )}
                >
                  <ThemePreview kind={t.id} />
                  <span className="flex items-center gap-1.5 px-3 py-2 text-[12.5px] font-medium">
                    {t.icon}
                    {t.label}
                  </span>
                </button>
              ))}
            </div>
          </Section>

          <Section
            title="Browser link"
            description="Pair Kudu Connect, the browser extension, so the browser agent and Kudu Desktop can hand each other tasks."
          >
            <LinkCard agents={agents} />
          </Section>

          <Section title="Keyboard shortcuts">
            <div className="divide-y divide-border-soft rounded-xl border border-border-default">
              {SHORTCUTS.map((s) => (
                <div key={s.label} className="flex items-center justify-between px-3.5 py-2.5 text-[12.5px]">
                  <span className="text-text-secondary">{s.label}</span>
                  <span className="flex gap-1">
                    {s.keys.map((k) => (
                      <Kbd key={k}>{k}</Kbd>
                    ))}
                  </span>
                </div>
              ))}
            </div>
          </Section>

          <Section
            title="MCP servers"
            description={'Connect other apps to the agent. Paste a "mcpServers" block from Claude Desktop, Cursor or a server\'s README, or a JSON array. Local servers use command + args (npx/uvx work); remote ones use url + optional headers. Add "disabled": true to switch one off.'}
          >
            <textarea
              value={mcpJson}
              onChange={(e) => setMcpJson(e.target.value)}
              rows={8}
              spellCheck={false}
              placeholder={`{\n  "mcpServers": {\n    "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "C:/Users/you/Documents"] },\n    "github": { "url": "https://api.githubcopilot.com/mcp/", "headers": { "Authorization": "Bearer <token>" } }\n  }\n}`}
              className="w-full rounded-lg border border-border-default bg-bg-primary px-3 py-2 font-mono text-[12px] outline-none focus:border-border-focus"
            />
            <div>
              <Button onClick={handleSaveMcp}>Save MCP servers</Button>
            </div>
            {mcpStatus && <Status {...mcpStatus} />}
          </Section>

          <Section title="About">
            <p className="text-[12.5px] text-text-muted">
              Kudu Desktop v{appVersion} - the assistant for your whole desktop. The ✕ button hides the window to the system tray; Kudu keeps
              listening for its shortcuts in the background.
            </p>
            <UpdateSection appVersion={appVersion} />
            <div>
              <Button onClick={() => quitApp().catch(() => {})} danger>
                <Power size={13} />
                Quit Kudu Desktop
              </Button>
            </div>
          </Section>
        </div>
      </div>
    </div>
  );
}

function Section({ title, description, children }: { title: string; description?: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-3">
      <div>
        <h3 className="text-[13.5px] font-semibold">{title}</h3>
        {description && <p className="mt-0.5 text-[12px] text-text-muted">{description}</p>}
      </div>
      {children}
    </section>
  );
}

function Button({ children, onClick, danger }: { children: React.ReactNode; onClick: () => void; danger?: boolean }) {
  return (
    <button
      onClick={onClick}
      className={clsx(
        "inline-flex items-center gap-1.5 rounded-lg border border-border-default px-3 py-1.5 text-[12.5px] font-medium transition",
        danger ? "text-text-secondary hover:border-danger/40 hover:bg-danger-soft hover:text-danger" : "hover:bg-bg-hover",
      )}
    >
      {children}
    </button>
  );
}

function Status({ ok, text }: { ok: boolean; text: string }) {
  return <p className={clsx("text-[12px]", ok ? "text-text-muted" : "text-danger")}>{text}</p>;
}

/** Tiny static mock of the app in each appearance. */
function ThemePreview({ kind }: { kind: ThemePref }) {
  const pane = (dark: boolean) => (
    <div className={clsx("flex h-full flex-1", dark ? "bg-[#161616]" : "bg-white")}>
      <div className={clsx("w-[30%] space-y-1 p-1.5", dark ? "bg-[#111]" : "bg-[#f4f4f4]")}>
        {[70, 50, 60].map((w) => (
          <div key={w} className={clsx("h-1 rounded", dark ? "bg-white/15" : "bg-black/10")} style={{ width: `${w}%` }} />
        ))}
      </div>
      <div className="flex flex-1 flex-col justify-end p-1.5">
        <div className={clsx("h-3 rounded-md border", dark ? "border-white/15 bg-[#1f1f1f]" : "border-black/10 bg-white")} />
      </div>
    </div>
  );
  return (
    <div className="flex h-16 border-b border-border-soft">
      {kind === "system" ? (
        <>
          {pane(false)}
          {pane(true)}
        </>
      ) : (
        pane(kind === "dark")
      )}
    </div>
  );
}
