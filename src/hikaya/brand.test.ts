import { rebrandTranslations } from "./brand";

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

console.log("brand.test.ts: all assertions passed");
