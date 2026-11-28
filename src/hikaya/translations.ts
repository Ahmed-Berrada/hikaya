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

type Tree = Record<string, unknown>;
const branch = (tree: Tree, key: string): Tree =>
  typeof tree[key] === "object" && tree[key] !== null
    ? (tree[key] as Tree)
    : {};

/**
 * Handy's shortcut screen names each binding from
 * `settings.general.shortcut.bindings.<id>`. Hikaya's own shortcuts get their
 * names from Hikaya's files, put there at load time.
 */
const withHikayaBindings = (translations: Tree, hikaya: Tree): Tree => {
  const settings = branch(translations, "settings");
  const general = branch(settings, "general");
  const shortcut = branch(general, "shortcut");
  const bindings = branch(shortcut, "bindings");
  return {
    ...translations,
    settings: {
      ...settings,
      general: {
        ...general,
        shortcut: {
          ...shortcut,
          bindings: { ...bindings, hikaya_command: hikaya.commandBinding },
        },
      },
    },
  };
};

/** Handy's translations for one language, renamed, plus Hikaya's strings. */
export const withHikayaTranslations = (
  langCode: string,
  translations: Record<string, unknown>,
): Record<string, unknown> => {
  const hikaya = hikayaStrings[langCode] ?? hikayaStrings.en;
  return {
    ...withHikayaBindings(rebrandTranslations(translations), hikaya),
    hikaya,
  };
};
