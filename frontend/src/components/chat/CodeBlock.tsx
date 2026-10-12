import { useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { Check, Copy } from "lucide-react";
import { languageFor } from "../../lib/languages";
import { editorTheme } from "../../lib/editorTheme";
import { useTheme } from "../../lib/theme";

interface CodeBlockProps {
  code: string;
  language?: string;
}

// Reuses the CodeMirror instance already pulled in for the file editor (phase
// 3) instead of adding a separate syntax-highlighter dependency just for chat
// code blocks — one highlighting engine for the whole app.
export function CodeBlock({ code, language }: CodeBlockProps) {
  const [copied, setCopied] = useState(false);
  const { theme } = useTheme();
  const extensions = [...languageFor(language), editorTheme(theme)];

  const copy = async () => {
    await navigator.clipboard.writeText(code);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="group relative my-2.5 overflow-hidden rounded-xl border border-border-default bg-bg-secondary">
      <div className="flex items-center justify-between border-b border-border-soft px-3 py-1.5">
        <span className="font-mono text-[10px] uppercase tracking-wide text-text-muted">
          {language || "text"}
        </span>
        <button
          onClick={copy}
          className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-text-muted transition hover:bg-bg-hover hover:text-text-primary"
        >
          {copied ? <Check size={11} /> : <Copy size={11} />}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <CodeMirror
        value={code}
        theme="none"
        editable={false}
        basicSetup={{ lineNumbers: code.split("\n").length > 3, foldGutter: false, highlightActiveLine: false }}
        extensions={extensions}
        className="text-[12px] [&_.cm-editor]:bg-transparent [&_.cm-gutters]:bg-transparent"
      />
    </div>
  );
}
