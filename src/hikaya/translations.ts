/**
 * Hikaya's own UI strings. They live outside Handy's locale files (so those
 * merge cleanly from upstream) and are added under the `hikaya` key when the
 * translations load. A language with no Hikaya file falls back to English.
 */
import { rebrandTranslations } from "./brand";

const hikayaModules = import.meta.glob<{ default: Record<string, unknown> }>(
  "./locales/*.json",
  { eager: true },
);

const hikayaStrings: Record<string, Record<string, unknown>> = {};
for (const [path, module] of Object.entries(hikayaModules)) {
  const langCode = path.match(/\.\/locales\/(.+)\.json/)?.[1];
  if (langCode) hikayaStrings[langCode] = module.default;
}

/** Handy's translations for one language, renamed, plus Hikaya's strings. */
export const withHikayaTranslations = (
  langCode: string,
  translations: Record<string, unknown>,
): Record<string, unknown> => ({
  ...rebrandTranslations(translations),
  hikaya: hikayaStrings[langCode] ?? hikayaStrings.en,
});
