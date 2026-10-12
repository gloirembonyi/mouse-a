import { EditorView } from "@codemirror/view";
import { defaultHighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { oneDarkHighlightStyle } from "@codemirror/theme-one-dark";
import type { Extension } from "@codemirror/state";
import type { Theme } from "./theme";

// One CodeMirror chrome for both appearances: colors come from the same CSS
// variables as the rest of the app (index.css), so the editor surface always
// matches the panel around it; only the syntax palette differs per theme.
const chrome = (dark: boolean) =>
  EditorView.theme(
    {
      "&": {
        color: "var(--mx-text-primary)",
        backgroundColor: "transparent",
        height: "100%",
      },
      ".cm-content": { caretColor: "var(--mx-accent)", fontFamily: "var(--font-mono)" },
      ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.6" },
      ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--mx-accent)" },
      "&.cm-focused": { outline: "none" },
      "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection":
        { backgroundColor: "var(--mx-accent-soft) !important" },
      ".cm-activeLine": { backgroundColor: dark ? "rgba(255,255,255,0.025)" : "rgba(0,0,0,0.025)" },
      ".cm-gutters": {
        backgroundColor: "transparent",
        color: "var(--mx-text-muted)",
        border: "none",
      },
      ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--mx-text-secondary)" },
      ".cm-lineNumbers .cm-gutterElement": { padding: "0 10px 0 14px" },
      ".cm-foldPlaceholder": {
        backgroundColor: "var(--mx-bg-tertiary)",
        border: "none",
        color: "var(--mx-text-secondary)",
      },
      ".cm-tooltip": {
        backgroundColor: "var(--mx-bg-elevated)",
        border: "1px solid var(--mx-border-default)",
        borderRadius: "8px",
      },
      ".cm-tooltip-autocomplete > ul > li[aria-selected]": {
        backgroundColor: "var(--mx-accent-soft)",
        color: "var(--mx-text-primary)",
      },
      ".cm-panels": { backgroundColor: "var(--mx-bg-secondary)", color: "var(--mx-text-primary)" },
      ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--mx-border-default)" },
      ".cm-search input, .cm-search button": { fontSize: "11px" },
      ".cm-matchingBracket": { backgroundColor: "var(--mx-accent-soft)", outline: "none" },
      ".cm-searchMatch": { backgroundColor: "rgba(240, 180, 90, 0.25)" },
    },
    { dark },
  );

const darkTheme: Extension = [chrome(true), syntaxHighlighting(oneDarkHighlightStyle)];
const lightTheme: Extension = [chrome(false), syntaxHighlighting(defaultHighlightStyle)];

export function editorTheme(theme: Theme): Extension {
  return theme === "dark" ? darkTheme : lightTheme;
}
