import { useEffect, useRef } from "react";
import { Group, Panel, Separator, type PanelImperativeHandle } from "react-resizable-panels";
import { Sidebar } from "./Sidebar";
import { RightDock } from "./RightDock";
import type { Conversation } from "../../lib/types";

interface AppShellProps {
  sidebarOpen: boolean;
  onCloseSidebar: () => void;
  rightDockOpen: boolean;
  onCloseRightDock: () => void;
  onToggleRightDock: () => void;
  workspaceRoot: string | null;
  onChangeWorkspaceRoot: (path: string) => void;
  threads: Conversation[];
  activeThreadId: string | null;
  onNewChat: () => void;
  onOpenThread: (id: string) => void;
  onDeleteThread: (id: string) => void;
  onOpenSettings: () => void;
  onToggleHoverMode: () => void;
  aiActive: boolean;
  children: React.ReactNode;
}

export function AppShell({
  sidebarOpen,
  onCloseSidebar,
  rightDockOpen,
  onCloseRightDock,
  onToggleRightDock,
  workspaceRoot,
  onChangeWorkspaceRoot,
  threads,
  activeThreadId,
  onNewChat,
  onOpenThread,
  onDeleteThread,
  onOpenSettings,
  onToggleHoverMode,
  aiActive,
  children,
}: AppShellProps) {
  const sidebarRef = useRef<PanelImperativeHandle>(null);
  const rightDockRef = useRef<PanelImperativeHandle>(null);

  // Controlled by the parent's boolean state (toggled from the title bar /
  // sidebar nav), rather than driving state purely off the panel's own
  // collapse events — keeps a single source of truth for "is it open." Runs
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
    // default size, so its "most recent size" — what expand() restores to —
    // would also be 0% the first time it's opened. Explicitly resizing to a
    // known-good percentage sidesteps that instead of relying on expand().
    if (rightDockOpen && panel.isCollapsed()) panel.resize("38%");
    if (!rightDockOpen && !panel.isCollapsed()) panel.collapse();
  }, [rightDockOpen]);

  return (
    <Group orientation="horizontal" className="flex-1">
      <Panel
        panelRef={sidebarRef}
        id="sidebar"
        defaultSize="22%"
        minSize="16%"
        maxSize="34%"
        collapsible
        collapsedSize="0%"
        onResize={(size) => {
          if (size.asPercentage === 0 && sidebarOpen) onCloseSidebar();
        }}
        className="min-w-0"
      >
        <Sidebar
          threads={threads}
          activeId={activeThreadId}
          onNewChat={onNewChat}
          onOpenThread={onOpenThread}
          onDeleteThread={onDeleteThread}
          onOpenSettings={onOpenSettings}
          onToggleHoverMode={onToggleHoverMode}
          onToggleIdeDock={onToggleRightDock}
          ideDockOpen={rightDockOpen}
          aiActive={aiActive}
        />
      </Panel>
      <Separator className="resize-handle resize-handle-v" />

      <Panel id="main-content" minSize="30%" className="min-w-0">
        {children}
      </Panel>

      <Separator className="resize-handle resize-handle-v" />
      <Panel
        panelRef={rightDockRef}
        id="right-dock"
        defaultSize="0%"
        minSize="20%"
        maxSize="55%"
        collapsible
        collapsedSize="0%"
        onResize={(size) => {
          if (size.asPercentage === 0 && rightDockOpen) onCloseRightDock();
        }}
        className="min-w-0"
      >
        <RightDock root={workspaceRoot} onChangeRoot={onChangeWorkspaceRoot} onClose={onCloseRightDock} />
      </Panel>
    </Group>
  );
}
