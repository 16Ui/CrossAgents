import { useEffect } from "react";
import { api } from "./api";
import { onHubEvent } from "./hub";
import { planWaitsForPerson } from "./lib/policy";

const route = (...parts: string[]) => "#/" + parts.map(encodeURIComponent).join("/");

/** The first human-readable line is enough for the Windows notification. */
export function questionPreview(text: string): string {
  const first = text.split(/\r?\n/).find((line) => line.trim()) ?? "";
  const plain = first.replace(/^#+\s*|^\s*[-*]\s+/g, "").replace(/[*]/g, "").trim();
  if (!plain || plain.includes(String.fromCharCode(96)) || /[A-Fa-f0-9]{16,}|[A-Za-z]:[\\/]/.test(plain)) {
    return "请打开 CrossAgents，查看这轮工作需要你决定什么。";
  }
  const chars = Array.from(plain);
  return chars.length <= 105 ? plain : chars.slice(0, 104).join("") + "…";
}

/** Desktop notices are reserved for finished work and actions that require the operator. */
export function useSystemNotices() {
  useEffect(() => {
    const bridge = window.crossagents;
    if (!bridge?.notify) return;
    const shown = new Set<string>();
    const show = (key: string, title: string, body: string, href: string) => {
      if (shown.has(key)) return;
      shown.add(key);
      bridge.notify?.({ title, body, href });
    };

    return onHubEvent((event) => {
      if (event.type === "human.asked") {
        const { item } = event.data;
        if (item.kind !== "question" || item.status !== "open" || item.action === "work") return;
        // An approval can expire to a denial, so it still needs a human decision.
        if (item.timeout_at && item.action !== "approval" && item.action !== "budget") return;

        const href = route("s", item.from, "questions", item.id);
        const approval = item.action === "approval";
        const body = approval && !item.detail
          ? "请打开这张审批卡，查看本次操作。"
          : questionPreview(item.text);
        const question = (title?: string) => show(
          "question:" + item.id,
          (title || "CrossAgents") + (approval ? " · 等待审批" : " · 需要你"),
          body,
          href,
        );
        if (item.run_id) void api.getRun(item.run_id).then(({ run }) => question(run.title), () => question());
        else question();
        return;
      }

      if (event.type !== "run.status_changed") return;
      const { id, from, to } = event.data;
      if (from === to || (to !== "completed" && to !== "awaiting_approval")) return;

      if (to === "completed") {
        const finished = (title?: string) =>
          show("completed:" + id, title || "CrossAgents", "本轮工作已完成", route("runs", id));
        void api.getRun(id).then(({ run }) => finished(run.title), () => finished());
      } else {
        // Automatically approved plans briefly pass through this same status.
        void api.getRun(id).then(({ run }) => {
          if (run.status === "awaiting_approval" && planWaitsForPerson(run)) {
            show("plan:" + id, run.title || "CrossAgents", "这轮工作需要你确认计划后才能继续。", route("runs", id));
          }
        }, () => {});
      }
    });
  }, []);

  useEffect(
    () => window.crossagents?.onNoticeOpen?.((href) => { window.location.hash = href; }),
    [],
  );
}
