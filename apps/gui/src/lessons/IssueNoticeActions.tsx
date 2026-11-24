import { useState } from "react";
import "./lessons.css";
import { api, errorText } from "../api";
import { triagePrompt } from "./lessons";

/** Closing a notice preserves the issue and never approves an operation. */
export function IssueNoticeActions({ run, issues, lead, onClosed }: {
  run: string; issues: string[]; lead?: string; onClosed?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState<string>();
  const close = async () => {
    setBusy(true); setError(undefined);
    try { await api.dismissRunIssues(run, issues); onClosed?.(); }
    catch (err) { setError(errorText(err)); }
    finally { setBusy(false); }
  };
  const delegate = async () => {
    if (!lead) return;
    setBusy(true); setError(undefined);
    try { await api.sendMessage(lead, { text: triagePrompt(issues.length) }); setSent(true); }
    catch (err) { setError(errorText(err)); }
    finally { setBusy(false); }
  };
  return <div className="run-issue-actions issue-notice-actions">
    <button disabled={busy} onClick={close} title="仅关闭提示并保留问题记录，不代表问题已解决">
      {busy ? "处理中…" : "关闭通知"}
    </button>
    {lead && <button disabled={busy || sent} onClick={delegate}>{sent ? "已交主管" : "交主管处理"}</button>}
    {error && <div role="alert" className="error">{error}</div>}
  </div>;
}
