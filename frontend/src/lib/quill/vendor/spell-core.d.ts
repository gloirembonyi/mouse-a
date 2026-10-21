// Types for the generated vendor/spell-core.js (see scripts/sync-quill.mjs).
import type { QuillMatch } from "./grammar-rules.js";

export interface SpellLike {
  correct(word: string): boolean;
  suggest(word: string): string[];
  add(word: string): unknown;
}

declare const core: {
  spellMatches(
    spell: SpellLike,
    text: string,
    opts?: { dictionary?: Set<string>; mask?: Array<[number, number]> },
  ): QuillMatch[];
  CUSTOM_WORDS: string[];
};
export default core;
