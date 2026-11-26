import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { startTheme } from "./theme";
import "./styles.css";
import { App } from "./App";

startTheme();

// iOS standalone PWA (`viewport-fit=cover` + `black-translucent`) shrinks `100dvh` by the
// safe-area inset (~60px), leaving an empty black band under the bottom tab bar. We keep
// `#root` / `.app-shell` pinned at `height: 100%` (full physical screen) and ONLY override
// `--app-height` when the software keyboard is actually open so the chat composer rises cleanly.
function syncViewportHeight() {
  const vv = window.visualViewport;
  const root = document.documentElement;
  if (!vv) {
    root.style.removeProperty("--app-height");
    return;
  }
  const keyboardOpen = window.innerHeight - vv.height > 100;
  if (keyboardOpen) {
    root.style.setProperty("--app-height", `${Math.round(vv.height)}px`);
    if (window.scrollY !== 0) window.scrollTo(0, 0);
  } else {
    root.style.removeProperty("--app-height");
  }
}

syncViewportHeight();
window.visualViewport?.addEventListener("resize", syncViewportHeight);
window.visualViewport?.addEventListener("scroll", syncViewportHeight);
window.addEventListener("orientationchange", syncViewportHeight);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
