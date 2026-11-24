// Reliable IME (Input Method Editor) composition guard, especially for macOS WebKit / WKWebView
// (Tauri on macOS) where pressing Enter to confirm a Chinese/Japanese candidate fires
// `compositionend` *before* `keydown(Enter)`, leaving `e.nativeEvent.isComposing === false`.

let composing = false;
let lastCompositionEnd = -Infinity;
let installed = false;

/** Grace window (ms) after `compositionend` during which a trailing Enter is still IME confirmation. */
export const IME_END_GRACE_MS = 80;

function ensureListeners() {
  if (installed || typeof window === "undefined") return;
  installed = true;
  window.addEventListener(
    "compositionstart",
    () => {
      composing = true;
    },
    true,
  );
  window.addEventListener(
    "compositionend",
    () => {
      composing = false;
      lastCompositionEnd = performance.now();
    },
    true,
  );
}

/** Pure decision helper, exposed for unit tests. */
export function isImeKeyDown(opts: {
  isComposing?: boolean;
  keyCode?: number;
  key?: string;
  composing: boolean;
  msSinceCompositionEnd: number;
}): boolean {
  if (opts.isComposing || opts.composing || opts.keyCode === 229) return true;
  if (opts.key === "Enter" && opts.msSinceCompositionEnd >= 0 && opts.msSinceCompositionEnd < IME_END_GRACE_MS) {
    return true;
  }
  return false;
}

/** Returns true when this React keyboard event belongs to an active or just-confirmed IME composition. */
export function isImeEvent(e: { nativeEvent: KeyboardEvent }): boolean {
  ensureListeners();
  const ne = e.nativeEvent;
  return isImeKeyDown({
    isComposing: ne.isComposing,
    keyCode: ne.keyCode,
    key: ne.key,
    composing,
    msSinceCompositionEnd: performance.now() - lastCompositionEnd,
  });
}
