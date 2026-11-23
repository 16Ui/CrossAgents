import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { RunDetail } from "@bindings/RunDetail";
import { ChatAvatar } from "../ChatAvatar";
import { CopyButton, Prose, time } from "../components";
import { DEFAULT_CHAT_PROFILE } from "../lib/chatProfile";
import { FOLD_INPUT, foldText, KICKOFF_SEPARATOR } from "../lib/messageText";
import { noticeLines } from "../lib/notices";
import type { TranscriptTurn } from "../session";
import "./sessionmessage.css";

/** A person's message, or one recorded before senders were. */
export const fromPerson = (turn: TranscriptTurn) => !turn.from || turn.from.kind === "human";

export const MessageAppearance = createContext(DEFAULT_CHAT_PROFILE);

/** Inserts selected assistant text as a markdown quote into the session composer. */
export const CiteContext = createContext<((text: string) => void) | undefined>(undefined);

export function PersonMessage({ children }: { children: ReactNode }) {
  const { name, avatar, showAvatars } = useContext(MessageAppearance);
  return (
    <div className={showAvatars ? "person-message with-avatar" : "person-message"}>
      <div className="person-message-content">
        {showAvatars && <span className="person-message-name">{name.trim() || "我"}</span>}
        {children}
      </div>
      {showAvatars && <ChatAvatar image={avatar} />}
    </div>
  );
}

/** What the message carried to the agent besides the person's words, kept out of the bubble's text. */
export function RideAlong({ notices }: { notices?: string[] }) {
  const lines = noticeLines(notices);
  if (!lines.length) return null;
  return (
    <details className="bubble-notices">
      <summary>随这条消息送达 {lines.length} 条提示</summary>
      <ul>
        {lines.map((line, i) => (
          <li key={i}>{line}</li>
        ))}
      </ul>
    </details>
  );
}

export function Bubble({ turn, tree }: { turn: TranscriptTurn; tree: RunDetail[] }) {
  const [open, setOpen] = useState(false);
  // Still the raw input: the fold below is what gets cut, never this.
  const request = tree.find(({ run }) => turn.input.startsWith(run.request.trim() + KICKOFF_SEPARATOR))?.run.request.trim();
  const visible = request === undefined || open ? turn.input : request;
  const long = request === undefined && turn.input.length > FOLD_INPUT;
  // Cut before parsing, so a fold in the middle of a fence or a link stays text (lib/messageText).
  const shown = long && !open ? foldText(turn.input) : visible;
  return (
    <div className="bubble bubble-md" title={time(turn.sent_at)}>
      <div className="bubble-text">
        <Prose text={shown} />
      </div>
      <RideAlong notices={turn.notices} />
      <div className="bubble-foot">
        {request !== undefined && (
          <button className="tools-more" aria-expanded={open} onClick={() => setOpen(!open)}>
            {open ? "收起 run 上下文 ▾" : "已附上 run 上下文（成员、校验、会话）▸"}
          </button>
        )}
        {long && (
          <button className="tools-more" onClick={() => setOpen(!open)}>
            {open ? "收起" : `展开全部（${turn.input.length} 字）`}
          </button>
        )}
        <CopyButton className="msg-copy" text={turn.input} title="复制这条消息的 markdown 原文" />
      </div>
    </div>
  );
}

/** One stretch of the agent's reply, finished or still streaming (`text` is the whole of it so far). */
export function AgentText({ text }: { text: string }) {
  const cite = useContext(CiteContext);
  const boxRef = useRef<HTMLDivElement>(null);
  const [pop, setPop] = useState<{ x: number; y: number; text: string } | null>(null);

  useEffect(() => {
    if (!pop) return;
    const hide = () => setPop(null);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("mousedown", hide);
    return () => {
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("mousedown", hide);
    };
  }, [pop]);

  const onMouseUp = () => {
    if (!cite || !boxRef.current) return;
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !sel.rangeCount) {
      setPop(null);
      return;
    }
    const picked = sel.toString().trim();
    if (!picked) {
      setPop(null);
      return;
    }
    const range = sel.getRangeAt(0);
    if (!boxRef.current.contains(range.commonAncestorContainer)) {
      setPop(null);
      return;
    }
    const rect = range.getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) return;
    setPop({ x: rect.left + rect.width / 2, y: Math.max(40, rect.top - 8), text: picked });
  };

  return (
    <div className="agent-text" ref={boxRef} onMouseUp={onMouseUp}>
      <Prose text={text} />
      <div className="copy-row">
        {cite && (
          <button
            type="button"
            className="msg-copy"
            title="引用这段回复到输入框"
            onClick={() => cite(text)}
          >
            引用
          </button>
        )}
        <CopyButton className="msg-copy" text={text} title="复制这段回复的 markdown 原文" />
      </div>
      {pop &&
        cite &&
        createPortal(
          <button
            type="button"
            className="cite-pop"
            style={{ left: pop.x, top: pop.y }}
            onMouseDown={(e) => {
              e.preventDefault();
              e.stopPropagation();
              cite(pop.text);
              window.getSelection()?.removeAllRanges();
              setPop(null);
            }}
          >
            引用到输入框
          </button>,
          document.body,
        )}
    </div>
  );
}
