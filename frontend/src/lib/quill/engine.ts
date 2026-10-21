// Kudu Quill, offline. The same rule engine and spelling core the browser extension and Kudu Core
// run, executed in the webview - no network, no sign-in. The engines are vendored from
// backend-Thothai by `npm run sync:quill`, so a match here is the match the server would give.
import nspell from "nspell";
import rules, { type QuillMatch, type TextStats } from "./vendor/grammar-rules.js";
import spellCore, { type SpellLike } from "./vendor/spell-core.js";
import affText from "./dict/en.aff?raw";
import dicText from "./dict/en.dic?raw";

export type { QuillMatch, TextStats };

export interface QuillOptions {
  picky: boolean;
  disabledRules: string[];
  disabledCategories: string[];
  /** Lowercased words the user added. */
  dictionary: Set<string>;
}

// Hunspell with the en_US dictionary (~550 KB) takes a moment to parse; do it once, on first use,
// off the typing path.
let spellPromise: Promise<SpellLike> | null = null;
export function loadSpell(): Promise<SpellLike> {
  if (!spellPromise) {
    spellPromise = new Promise<SpellLike>((resolve) => {
      // Yield first so opening a file never waits for the dictionary.
      setTimeout(() => {
        const spell = nspell(affText, dicText) as unknown as SpellLike;
        for (const w of spellCore.CUSTOM_WORDS) {
          try { spell.add(w); } catch { /* ignore */ }
        }
        resolve(spell);
      }, 0);
    });
  }
  return spellPromise;
}

/** Rules + spelling for `text`, merged the way Kudu Core merges them. */
export async function checkText(text: string, opts: QuillOptions): Promise<QuillMatch[]> {
  if (!text.trim()) return [];
  const mask = rules.buildIgnoreMask(text);
  const ruleMatches = rules.check(text, {
    picky: opts.picky,
    disabledRules: opts.disabledRules,
    disabledCategories: opts.disabledCategories,
    dictionary: opts.dictionary,
  });
  let spellMatches: QuillMatch[] = [];
  if (!opts.disabledCategories.includes("spelling")) {
    const spell = await loadSpell();
    spellMatches = spellCore.spellMatches(spell, text, { dictionary: opts.dictionary, mask });
  }
  return rules.dedupe([...ruleMatches, ...spellMatches]);
}

export const textStats = (text: string): TextStats => rules.stats(text);
export const applyAll = (text: string, matches: QuillMatch[]) => rules.applyAll(text, matches);

// ── Presentation: the same five groups and colours as the extension ─────────
export type QuillGroup = "correctness" | "grammar" | "clarity" | "style" | "delivery";

const GROUP_OF: Record<string, QuillGroup> = {
  spelling: "correctness",
  grammar: "grammar", punctuation: "grammar", capitalization: "grammar", typography: "grammar",
  clarity: "clarity", wordiness: "clarity",
  style: "style",
  tone: "delivery", inclusive: "delivery",
};
export const groupOf = (category: string): QuillGroup => GROUP_OF[category] ?? "correctness";

export const GROUP_LABEL: Record<QuillGroup, string> = {
  correctness: "Spelling", grammar: "Grammar", clarity: "Clarity", style: "Style", delivery: "Tone",
};
export const GROUP_COLOR: Record<QuillGroup, string> = {
  correctness: "#e5484d", grammar: "#e0a100", clarity: "#3b82f6", style: "#6366f1", delivery: "#8b5cf6",
};

export const CATEGORY_LABEL: Record<string, string> = {
  spelling: "Spelling", grammar: "Grammar", punctuation: "Punctuation", capitalization: "Capitalization",
  typography: "Typography", clarity: "Clarity", wordiness: "Wordiness", style: "Style", tone: "Tone",
  inclusive: "Inclusive language",
};

/** A fix is "safe" for Fix-all when it is an objective correction with a concrete replacement. */
export const isSafeFix = (m: QuillMatch) =>
  (m.source === "rules" || m.source === "spell" || m.source === "languagetool") &&
  m.replacements.length > 0 &&
  (m.severity === "error" || m.severity === "warning") &&
  (groupOf(m.category) === "correctness" || groupOf(m.category) === "grammar");
