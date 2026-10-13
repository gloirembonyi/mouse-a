import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../index.css";
import "./copilot.css";
import { CopilotBar } from "./CopilotBar";

// The in-call bar always renders on the dark "night" palette: it floats over
// video calls and shared slides of any brightness, and a dark tinted glass
// stays readable on all of them.
document.documentElement.dataset.theme = "dark";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <CopilotBar />
  </StrictMode>,
);
