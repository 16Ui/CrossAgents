import { useState, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";

type Block =
  | { kind: "prose"; text: string }
  | { kind: "code"; lang: string; code: string };

function parseBlocks(text: string): Block[] {
  const lines = text.split("\n");
  const blocks: Block[] = [];
  let inCode = false;
  let lang = "";
  let buf: string[] = [];

  const flushProse = () => {
    const joined = buf.join("\n").trim();
    if (joined) blocks.push({ kind: "prose", text: joined });
    buf = [];
  };

  const flushCode = () => {
    blocks.push({ kind: "code", lang, code: buf.join("\n") });
    buf = [];
    lang = "";
  };

  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith("```")) {
      if (inCode) {
        flushCode();
        inCode = false;
      } else {
        flushProse();
        lang = trimmed.slice(3).trim();
        inCode = true;
      }
    } else {
      buf.push(line);
    }
  }
  if (inCode) flushCode();
  else flushProse();
  return blocks;
}

function renderInline(text: string): ReactNode[] {
  const parts: ReactNode[] = [];
  const regex = /(`[^`]+`|\*\*[^*]+\*\*|https?:\/\/[^\s)<>]+)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  let key = 0;

  while ((match = regex.exec(text)) !== null) {
    if (match.index > lastIndex) {
      parts.push(text.slice(lastIndex, match.index));
    }
    const token = match[0];
    if (token.startsWith("`") && token.endsWith("`")) {
      parts.push(
        <code key={key++} className="md-inline-code">
          {token.slice(1, -1)}
        </code>,
      );
    } else if (token.startsWith("**") && token.endsWith("**")) {
      parts.push(<strong key={key++}>{token.slice(2, -2)}</strong>);
    } else {
      parts.push(
        <a key={key++} href={token} target="_blank" rel="noreferrer" style={{ color: "var(--accent)" }}>
          {token}
        </a>,
      );
    }
    lastIndex = match.index + token.length;
  }
  if (lastIndex < text.length) parts.push(text.slice(lastIndex));
  return parts;
}

function CodeCard({ lang, code }: { lang: string; code: string }) {
  const [copied, setCopied] = useState(false);

  const copy = () => {
    navigator.clipboard?.writeText(code).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="code-card">
      <div className="code-card-head">
        <span className="mono">{lang || "code"}</span>
        <button
          type="button"
          onClick={copy}
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 4,
            fontSize: 11.5,
            color: copied ? "#4ade80" : "#9ca3af",
          }}
        >
          {copied ? <Check size={13} /> : <Copy size={13} />}
          {copied ? "已复制" : "复制"}
        </button>
      </div>
      <pre className="mono">{code}</pre>
    </div>
  );
}

export function MarkdownView({ text }: { text: string }) {
  const blocks = parseBlocks(text);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
      {blocks.map((block, idx) =>
        block.kind === "code" ? (
          <CodeCard key={idx} lang={block.lang} code={block.code} />
        ) : (
          <p key={idx} className="md-prose">
            {renderInline(block.text)}
          </p>
        ),
      )}
    </div>
  );
}
