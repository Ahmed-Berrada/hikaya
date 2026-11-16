/**
 * Hikaya is a fork of Handy and keeps Handy's translation files unchanged, so
 * upstream updates to them merge without conflicts. The product name is
 * swapped here, when the translations are loaded.
 */

export const APP_NAME = "Hikaya";
export const REPO_URL = "https://github.com/Ahmed-Berrada/hikaya";
export const RELEASES_URL = `${REPO_URL}/releases/latest`;

/** Upstream project, credited in the About page. */
export const UPSTREAM_NAME = "Handy";
export const UPSTREAM_DONATE_URL = "https://handy.computer/donate";

/**
 * Translation subtrees that really are about Handy and must keep its name:
 * the donation prompt sends money to Handy's author, not to Hikaya.
 */
const KEEP_UPSTREAM_NAME = ["settings.about.supportDevelopment"];

const UPSTREAM_WORD = /\bHandy\b/g;

const rebrandValue = (value: unknown, path: string): unknown => {
  if (KEEP_UPSTREAM_NAME.includes(path)) return value;
  if (typeof value === "string") return value.replace(UPSTREAM_WORD, APP_NAME);
  if (Array.isArray(value)) {
    return value.map((item, index) => rebrandValue(item, `${path}.${index}`));
  }
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, child]) => [
        key,
        rebrandValue(child, path ? `${path}.${key}` : key),
      ]),
    );
  }
  return value;
};

/** Returns a copy of a translation tree with the product name replaced. */
export const rebrandTranslations = (
  translations: Record<string, unknown>,
): Record<string, unknown> =>
  rebrandValue(translations, "") as Record<string, unknown>;
