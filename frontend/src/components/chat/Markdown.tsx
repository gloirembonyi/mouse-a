import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { FileText } from "lucide-react";
import { CodeBlock } from "./CodeBlock";
import { FileCard, FileCardPending } from "./FileCard";
import { MermaidBlock } from "./MermaidBlock";
import { looksLikeFilePath, requestOpenFile } from "../../lib/openFile";
import { parseKuduFile } from "../../lib/kuduFiles";

interface MarkdownProps {
  children: string;
  className?: string;
}

export function Markdown({ children, className }: MarkdownProps) {
  const source = children;
  return (
    <div className={`prose-mouse ${className ?? ""}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          // Fenced blocks render their own frame (CodeBlock / diagram / file card);
          // a <pre> around them would only add browser margins and monospace text.
          pre(props) {
            return <>{props.children}</>;
          },
          code(props) {
            const { children, className, node, ...rest } = props;
            const match = /language-([\w-]+)/.exec(className || "");
            const isBlock = Boolean(match) || String(children).includes("\n");
            if (isBlock) {
              const code = String(children).replace(/\n$/, "");
              const language = match?.[1];
              const lang = language?.toLowerCase();
              if (lang === "kudu-file" || lang === "mermaid") {
                // While an answer streams, a fence stays open until its closing ```
                // arrives: never parse a half-sent file or redraw a diagram per token.
                const end = node?.position?.end.offset;
                const closed = end == null || /(^|\n)[ \t]*(`{3,}|~{3,})[ \t]*$/.test(source.slice(0, end));
                if (lang === "kudu-file") {
                  if (!closed) return <FileCardPending />;
                  const file = parseKuduFile(code);
                  if (file) return <FileCard file={file} />;
                } else if (closed) {
                  return <MermaidBlock code={code} />;
                }
              }
              return <CodeBlock code={code} language={language} />;
            }
            // A file path the agent wrote or mentioned → one click opens it.
            const text = String(children);
            if (looksLikeFilePath(text)) {
              return (
                <button
                  type="button"
                  onClick={() => requestOpenFile(text.trim())}
                  title="Open in the editor"
                  className="inline-flex max-w-full items-center gap-1 rounded bg-accent-soft px-1.5 py-0.5 align-baseline font-mono text-[12px] text-accent transition hover:underline"
                >
                  <FileText size={11} className="flex-shrink-0" />
                  <span className="break-all text-left">{text}</span>
                </button>
              );
            }
            return (
              <code className="rounded bg-bg-tertiary px-1.5 py-0.5 font-mono text-[12px]" {...rest}>
                {children}
              </code>
            );
          },
          a(props) {
            return <a {...props} target="_blank" rel="noreferrer" className="text-accent hover:underline" />;
          },
          ul(props) {
            return <ul className="my-1.5 list-disc space-y-0.5 pl-5" {...props} />;
          },
          ol(props) {
            return <ol className="my-1.5 list-decimal space-y-0.5 pl-5" {...props} />;
          },
          h1(props) {
            return <h1 className="mb-1.5 mt-3 text-base font-semibold" {...props} />;
          },
          h2(props) {
            return <h2 className="mb-1.5 mt-3 text-[14px] font-semibold" {...props} />;
          },
          h3(props) {
            return <h3 className="mb-1 mt-2.5 text-[13px] font-semibold" {...props} />;
          },
          p(props) {
            return <p className="my-1 leading-[1.65]" {...props} />;
          },
          blockquote(props) {
            return <blockquote className="my-2 border-l-2 border-accent pl-2.5 text-text-secondary" {...props} />;
          },
          table(props) {
            return (
              <div className="my-2 overflow-x-auto">
                <table className="w-full border-collapse text-[12px]" {...props} />
              </div>
            );
          },
          th(props) {
            return <th className="border border-border-default bg-bg-tertiary px-2 py-1 text-left" {...props} />;
          },
          td(props) {
            return <td className="border border-border-default px-2 py-1" {...props} />;
          },
          strong(props) {
            return <strong className="font-semibold" {...props} />;
          },
        }}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}
