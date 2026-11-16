import { rebrandTranslations } from "./brand";
import en from "../i18n/locales/en/translation.json";
import fr from "../i18n/locales/fr/translation.json";

const assertEqual = (actual: unknown, expected: unknown, label: string) => {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) throw new Error(`${label}\n  expected ${e}\n  actual   ${a}`);
};

assertEqual(
  rebrandTranslations({
    tray: { quit: "Quit Handy" },
    list: ["Handy's models", "Handyman stays"],
    count: 3,
  }),
  {
    tray: { quit: "Quit Hikaya" },
    list: ["Hikaya's models", "Handyman stays"],
    count: 3,
  },
  "replaces the whole word in nested strings and arrays only",
);

assertEqual(
  rebrandTranslations({
    settings: {
      about: {
        version: { description: "Current version of Handy" },
        supportDevelopment: { description: "Help us continue building Handy" },
      },
    },
  }),
  {
    settings: {
      about: {
        version: { description: "Current version of Hikaya" },
        supportDevelopment: { description: "Help us continue building Handy" },
      },
    },
  },
  "keeps Handy's name on the donation prompt",
);

const original = { a: "Handy" };
rebrandTranslations(original);
assertEqual(original, { a: "Handy" }, "does not mutate its input");

// The real English and French files: no "Handy" may survive outside the
// donation prompt, and the donation prompt must still name Handy.
const collect = (value: unknown, path: string, out: [string, string][]) => {
  if (typeof value === "string") out.push([path, value]);
  else if (value !== null && typeof value === "object") {
    for (const [key, child] of Object.entries(value)) {
      collect(child, path ? `${path}.${key}` : key, out);
    }
  }
};

for (const [lang, translations] of Object.entries({ en, fr })) {
  const strings: [string, string][] = [];
  collect(rebrandTranslations(translations), "", strings);
  const leftovers = strings
    .filter(([, text]) => /\bHandy\b/.test(text))
    .map(([path]) => path);
  assertEqual(
    leftovers,
    ["settings.about.supportDevelopment.description"],
    `${lang}: only the donation prompt still says Handy`,
  );
  if (!strings.some(([, text]) => text.includes("Hikaya"))) {
    throw new Error(`${lang}: no string mentions Hikaya`);
  }
}

console.log("brand.test.ts: all assertions passed");
