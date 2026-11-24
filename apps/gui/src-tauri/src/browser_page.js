function drive(action) {
  try {
    const tag = "__crossagentsSnapshotRefs";
    if (action.action === "console") return (window.__crossagentsConsole || []).slice(-150);
    if (action.action === "snapshot") {
      const nodes = Array.from(document.querySelectorAll("a[href], button, input:not([type=hidden]), textarea, select, summary, [contenteditable=true], [role=button], [role=link]"))
        .filter((element) => element.getClientRects().length > 0 && !element.closest("[aria-hidden=true]"))
        .slice(0, 300);
      window[tag] = nodes;
      const refs = nodes.map((node, index) => {
        const name = node.getAttribute("aria-label") || node.innerText || node.getAttribute("placeholder") || node.title || node.name || "";
        return `[ref=${index + 1}] ${node.getAttribute("role") || node.tagName.toLowerCase()} ${name.trim().slice(0, 120)}`;
      });
      const content = (document.body?.innerText || "").slice(0, 16000);
      return { title: document.title, url: location.href, text: [...refs, "", content].join("\n") };
    }
    if (action.action === "scroll") {
      window.scrollBy({ top: action.dy, behavior: "instant" });
      return { ok: true };
    }
    if (action.action === "press") {
      const target = document.activeElement || document.body;
      const allowed = target?.dispatchEvent(new KeyboardEvent("keydown", { key: action.key, bubbles: true, cancelable: true }));
      if (allowed && action.key === "Enter") {
        if (target.matches("button, a[href]")) target.click();
        else if (target instanceof HTMLInputElement && target.form) target.form.requestSubmit();
      }
      target?.dispatchEvent(new KeyboardEvent("keyup", { key: action.key, bubbles: true }));
      return { ok: true };
    }
    const node = window[tag]?.[action.reference - 1];
    if (!node?.isConnected) throw new Error("页面已变化，请重新获取页面快照");
    if (action.action === "click") {
      node.scrollIntoView({ block: "nearest" });
      node.click();
      return { ok: true };
    }
    node.focus();
    if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) {
      const proto = node instanceof HTMLInputElement ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
      Object.getOwnPropertyDescriptor(proto, "value").set.call(node, action.text);
    } else if (node.isContentEditable) {
      node.textContent = action.text;
    } else {
      throw new Error("该元素不是文本输入框");
    }
    node.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText" }));
    if (action.submit && node.form) node.form.requestSubmit();
    return { ok: true };
  } catch (error) {
    return { error: String(error) };
  }
}
