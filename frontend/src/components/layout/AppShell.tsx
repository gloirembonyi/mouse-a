import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { Group, Panel, Separator, type PanelImperativeHandle } from "react-resizable-panels";

// The dock (file tree, CodeMirror editor, xterm + a PowerShell PTY) is loaded
// and mounted the first time it's opened, not at startup - most sessions never
// open it, and it used to spawn a shell on every launch.
const RightDock = lazy(() => import("./RightDock").then((m) => ({ default: m.RightDock })));

interface AppShellProps {
  sidebar: React.ReactNode;
  sidebarOpen: boolean;
  onCloseSidebar: () => void;
  rightDockOpen: boolean;
  onCloseRightDock: () => void;
  workspaceRoot: string | null;
  projectName: string | null;
  onAddProject: () => void;
  onAskAboutFile: (path: string, name: string) => void;
  onAskAboutText: (text: string, title: string) => void;
  /** Chat panel hidden so the editor dock can use the whole width. */
  chatHidden: boolean;
  onChatHiddenChange: (hidden: boolean) => void;
  /** Some chat has an agent running (shown in the dock while chat is hidden). */
  agentBusy: boolean;
  children: React.ReactNode;
}

export function AppShell({
  sidebar,
  sidebarOpen,
  onCloseSidebar,
  rightDockOpen,
  onCloseRightDock,
  workspaceRoot,
  projectName,
  onAddProject,
  onAskAboutFile,
  onAskAboutText,
  chatHidden,
  onChatHiddenChange,
  agentBusy,
  children,
}: AppShellProps) {
  const sidebarRef = useRef<PanelImperativeHandle>(null);
  const chatRef = useRef<PanelImperativeHandle>(null);
  const rightDockRef = useRef<PanelImperativeHandle>(null);
  const [dockMounted, setDockMounted] = useState(rightDockOpen);
  if (rightDockOpen && !dockMounted) setDockMounted(true);

  // Controlled by the parent's boolean state (toggled from the title bar /
  // sidebar nav), rather than driving state purely off the panel's own
  // collapse events - keeps a single source of truth for "is it open." Runs
  // as an effect (not during render) since these are imperative layout
  // mutations on the panel instance.
  useEffect(() => {
    const panel = sidebarRef.current;
    if (!panel) return;
    if (sidebarOpen && panel.isCollapsed()) panel.expand();
    if (!sidebarOpen && !panel.isCollapsed()) panel.collapse();
  }, [sidebarOpen]);

  useEffect(() => {
    const panel = rightDockRef.current;
    if (!panel) return;
    // The right dock starts at 0% (fully collapsed) rather than a real
    // default size, so its "most recent size" - what expand() restores to -
    // would also be 0% the first time it's opened. Explicitly resizing to a
    // known-good percentage sidesteps that instead of relying on expand().
    if (rightDockOpen && panel.isCollapsed()) panel.resize("42%");
    if (!rightDockOpen && !panel.isCollapsed()) panel.collapse();
  }, [rightDockOpen]);

  useEffect(() => {
    const panel = chatRef.current;
    if (!panel) return;
    if (chatHidden && !panel.isCollapsed()) panel.collapse();
    if (!chatHidden && panel.isCollapsed()) {
      panel.expand();
      // First time (or after the dock grew into it) expand() may restore ~0.
      if (panel.getSize().asPercentage < 25) panel.resize("45%");
    }
  }, [chatHidden]);

  return (
    <Group orientation="horizontal" className="flex-1">
      <Panel
        panelRef={sidebarRef}
        id="sidebar"
        defaultSize="20%"
        minSize="14%"
        maxSize="32%"
        collapsible
        collapsedSize="0%"
        onResize={(size) => {
          if (size.asPercentage === 0 && sidebarOpen) onCloseSidebar();
        }}
        className="min-w-0"
      >
        {sidebar}
      </Panel>
      <Separator className="resize-handle resize-handle-v" />

      <Panel
        panelRef={chatRef}
        id="main-content"
        minSize="28%"
        collapsible
        collapsedSize="0%"
        onResize={(size) => {
          // Dragging the divider all the way over hides the chat too.
          const collapsed = size.asPercentage === 0;
          if (collapsed !== chatHidden && rightDockOpen) onChatHiddenChange(collapsed);
        }}
        className="min-w-0 bg-bg-primary"
      >
        {children}
      </Panel>

      <Separator className="resize-handle resize-handle-v" />
      <Panel
        panelRef={rightDockRef}
        id="right-dock"
        defaultSize="0%"
        minSize="22%"
        maxSize="100%"
        collapsible
        collapsedSize="0%"
        onResize={(size) => {
          if (size.asPercentage === 0 && rightDockOpen) onCloseRightDock();
        }}
        className="min-w-0"
      >
        {dockMounted && (
          <Suspense fallback={<div className="h-full bg-bg-secondary" />}>
            <RightDock
              root={workspaceRoot}
              projectName={projectName}
              onAddProject={onAddProject}
              onClose={onCloseRightDock}
              onAskAboutFile={onAskAboutFile}
              onAskAboutText={onAskAboutText}
              chatHidden={chatHidden}
              onToggleChat={() => onChatHiddenChange(!chatHidden)}
              agentBusy={agentBusy}
            />
          </Suspense>
        )}
      </Panel>
    </Group>
  );
}
