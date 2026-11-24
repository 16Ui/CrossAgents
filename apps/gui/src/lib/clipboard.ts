export function writeClipboard(text: string): Promise<void> {
  return window.crossagents?.writeClipboard?.(text)
    ?? navigator.clipboard?.writeText(text)
    ?? Promise.reject(new Error("剪贴板不可用"));
}
