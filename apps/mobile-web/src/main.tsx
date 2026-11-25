import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { startTheme } from "./theme";
import "./styles.css";
import { App } from "./App";

startTheme();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
