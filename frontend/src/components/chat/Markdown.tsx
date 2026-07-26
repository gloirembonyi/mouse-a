import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { CodeBlock } from "./CodeBlock";

interface MarkdownProps {
  children: string;
  className?: string;
}

export function Markdown({ children, className }: MarkdownProps) {
  return (
    <div className={`prose-mouse ${className ?? ""}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          code(props) {
            const { children, className, ...rest } = props;
            const match = /language-(\w+)/.exec(className || "");
            const isBlock = Boolean(match) || String(children).includes("\n");
            if (isBlock) {
              return <CodeBlock code={String(children).replace(/\n$/, "")} language={match?.[1]} />;
            }
            return (
              <code className="rounded bg-white/10 px-1.5 py-0.5 font-mono text-[12px]" {...rest}>
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
