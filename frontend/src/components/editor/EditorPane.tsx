import { useState } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import { FileTree } from "./FileTree";
import { Editor } from "./Editor";

interface EditorPaneProps {
  root: string;
}

export function EditorPane({ root }: EditorPaneProps) {
  const [openPath, setOpenPath] = useState<string | null>(null);

  return (
    <Group orientation="horizontal" className="flex-1">
      <Panel id="file-tree" defaultSize="35%" minSize="20%" maxSize="60%" className="min-w-0 border-r border-border-default">
        <FileTree root={root} activePath={openPath} onOpenFile={setOpenPath} />
      </Panel>
      <Separator className="resize-handle resize-handle-v" />
      <Panel id="editor" minSize="30%" className="min-w-0">
        <Editor path={openPath} />
      </Panel>
    </Group>
  );
}
