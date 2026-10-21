// Types for the generated vendor/grammar-rules.js (see scripts/sync-quill.mjs). Hand-written: the
// sync script only rewrites the .js, so this file survives it.
export interface QuillMatch {
  id: string;
  message: string;
  shortMessage?: string;
  offset: number;
  length: number;
  replacements: { value: string; description?: string }[];
  category: string;
  severity: "error" | "warning" | "suggestion";
  picky?: boolean;
  autocorrect?: boolean;
  source: string;
  explanationUrl?: string;
  ruleDescription?: string;
}

export interface CheckOptions {
  picky?: boolean;
  disabledRules?: Iterable<string>;
  disabledCategories?: Iterable<string>;
  dictionary?: Iterable<string>;
}

export interface TextStats {
  characters: number;
  charactersNoSpaces: number;
  words: number;
  uniqueWords: number;
  sentences: number;
  paragraphs: number;
  avgWordsPerSentence: number;
  avgSyllablesPerWord: number;
  longWordsPct: number;
  readingTimeSeconds: number;
  speakingTimeSeconds: number;
  fleschReadingEase: number;
  gradeLevel: number;
  readabilityLabel: string;
}

declare const rules: {
  check(text: string, opts?: CheckOptions): QuillMatch[];
  dedupe(matches: QuillMatch[]): QuillMatch[];
  applyAll(text: string, matches: QuillMatch[]): string;
  stats(text: string): TextStats;
  buildIgnoreMask(text: string): Array<[number, number]>;
  overlapsMask(mask: Array<[number, number]>, start: number, end: number): boolean;
};
export default rules;
