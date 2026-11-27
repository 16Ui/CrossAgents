import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { startTheme } from "./theme";
import "./styles.css";
import { App } from "./App";

startTheme();

// iOS Home-Screen standalone PWAs that were added while `black-translucent` was set hit a
// WebKit bug where `100%`, `100dvh`, `position: fixed; inset: 0`, and `window.innerHeight`
// all stop ~59pt (the top status bar height) short of the physical bottom edge of the screen.
// Measure `window.screen` vs layout viewport height so BOTH existing and newly installed
// standalone icons span 100% edge-to-edge while still shrinking cleanly for the iOS keyboard.
function syncViewportHeight() {
  const root = document.documentElement;
  const vv = window.visualViewport;
  const isStandalone =
    (window.navigator as { standalone?: boolean }).standalone === true ||
    window.matchMedia("(display-mode: standalone)").matches;
  const isPortrait = window.matchMedia("(orientation: portrait)").matches;
  const physicalH = isPortrait
    ? Math.max(window.screen.height, window.screen.width)
    : Math.min(window.screen.height, window.screen.width);
  const layoutH = Math.min(window.innerHeight, root.clientHeight || window.innerHeight);
  const keyboardOpen = Boolean(vv && window.innerHeight - vv.height > 100);

  if (keyboardOpen && vv) {
    root.style.setProperty("--app-height", `${Math.round(vv.height)}px`);
    if (window.scrollY !== 0) window.scrollTo(0, 0);
  } else if (isStandalone && physicalH > layoutH && physicalH - layoutH <= 130) {
    root.style.setProperty("--app-height", `${physicalH}px`);
  } else {
    root.style.removeProperty("--app-height");
  }
}

syncViewportHeight();
window.addEventListener("resize", syncViewportHeight);
window.addEventListener("pageshow", syncViewportHeight);
window.addEventListener("orientationchange", () => setTimeout(syncViewportHeight, 80));
window.addEventListener("focusout", () => setTimeout(syncViewportHeight, 120));
window.visualViewport?.addEventListener("resize", syncViewportHeight);
window.visualViewport?.addEventListener("scroll", syncViewportHeight);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
