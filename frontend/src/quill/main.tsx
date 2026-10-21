import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../index.css";
import { QuillPopup } from "./QuillPopup";
import { startQuillSync } from "../lib/quill/sync";

// The popup floats over whatever app the user is writing in; it always uses the dark palette.
document.documentElement.dataset.theme = "dark";
document.body.style.background = "var(--mx-bg-primary)";

startQuillSync();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QuillPopup />
  </StrictMode>,
);
