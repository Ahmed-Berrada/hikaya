import { test, expect } from "@playwright/test";
import { mockTauri } from "./tauri-mock";

/** Clicks a switch where a person would: on its visible track, the label
 * around the hidden checkbox. */
const flip = (checkbox: import("@playwright/test").Locator) =>
  checkbox.locator("xpath=..").click();

const openLearning = async (page: import("@playwright/test").Page) => {
  await page.goto("/");
  await page.getByText("Learning", { exact: true }).click();
  await expect(page.getByText("Replacement rules")).toBeVisible();
};

test.describe("Learning screen", () => {
  test("shows dictation stats and what is corrected most", async ({ page }) => {
    await mockTauri(page);
    await page.addInitScript(() => {
      (window as any).__HIKAYA_MOCK__.handlers.hikaya_stats = () => ({
        words_today: 312,
        words_week: 4821,
        words_total: 18250,
        dictations_total: 640,
        minutes_saved: 335,
        rules: 0,
        learned_rules: 0,
        top_corrections: [
          { from: "get hub", to: "GitHub", count: 7 },
          { from: "a med", to: "Ahmed", count: 3 },
        ],
      });
    });
    await openLearning(page);
    await expect(page.getByText("Your dictation")).toBeVisible();
    await expect(page.getByText("18,250")).toBeVisible();
    await expect(page.getByText("about 335 min")).toBeVisible();
    await expect(page.getByText("640 dictations")).toBeVisible();
    await expect(page.getByText("What you correct most")).toBeVisible();
    await expect(page.getByText("×7")).toBeVisible();
  });

  test("the app is named Hikaya and offers the Learning section", async ({
    page,
  }) => {
    await mockTauri(page);
    await page.goto("/");
    await expect(page.getByRole("img", { name: "Hikaya" })).toBeVisible();
    await expect(page.getByText("Learning", { exact: true })).toBeVisible();
    await expect(page.locator("body")).not.toContainText("Handy");
  });

  test("starts empty, then adds, disables and deletes a rule", async ({
    page,
  }) => {
    await mockTauri(page);
    await openLearning(page);
    await expect(page.getByText("No rules yet.")).toBeVisible();
    await expect(page.getByText("No suggestions yet.")).toBeVisible();

    const add = page.getByRole("button", { name: "Add rule" });
    await expect(add).toBeDisabled();
    await page.getByLabel("When Hikaya writes").fill("get hub");
    await expect(add).toBeDisabled();
    await page.getByLabel("Replace it with").fill("GitHub");
    await add.click();

    await expect(page.getByText("get hub", { exact: true })).toBeVisible();
    await expect(page.getByText("GitHub", { exact: true })).toBeVisible();
    await expect(page.getByLabel("When Hikaya writes")).toHaveValue("");
    await expect(page.getByText("No rules yet.")).toBeHidden();

    const toggle = page.getByRole("checkbox", {
      name: "Use the rule “get hub”",
    });
    await expect(toggle).toBeChecked();
    await flip(toggle);
    await expect(toggle).not.toBeChecked();

    await page
      .getByRole("button", { name: "Delete the rule “get hub”" })
      .click();
    await expect(page.getByText("No rules yet.")).toBeVisible();
  });

  test("Enter in a field adds the rule", async ({ page }) => {
    await mockTauri(page);
    await openLearning(page);
    await page.getByLabel("When Hikaya writes").fill("clod");
    await page.getByLabel("Replace it with").fill("Claude");
    await page.getByLabel("Replace it with").press("Enter");
    await expect(page.getByText("Claude", { exact: true })).toBeVisible();
  });

  test("accepting a suggestion turns it into a learned rule", async ({
    page,
  }) => {
    await mockTauri(page, {
      proposals: [
        { pattern: "a med", replacement: "Ahmed", count: 3 },
        { pattern: "verse cell", replacement: "Vercel", count: 2 },
      ],
    });
    await openLearning(page);
    await expect(page.getByText("Corrected 3 times")).toBeVisible();

    await page.getByRole("button", { name: "Always replace" }).first().click();
    await expect(page.getByText("Corrected 3 times")).toBeHidden();
    await expect(page.getByText("Learned", { exact: true })).toBeVisible();
    await expect(page.getByText("Ahmed", { exact: true })).toBeVisible();

    await page.getByRole("button", { name: "Ignore" }).click();
    await expect(page.getByText("No suggestions yet.")).toBeVisible();
  });

  test("a failed save shows the error and keeps what was typed", async ({
    page,
  }) => {
    await mockTauri(page);
    await openLearning(page);
    await page.evaluate(() => {
      (window as any).__HIKAYA_MOCK__.handlers.hikaya_add_rule = () => {
        throw "a rule needs at least one word to look for";
      };
    });
    await page.getByLabel("When Hikaya writes").fill("...");
    await page.getByLabel("Replace it with").fill("x");
    await page.getByRole("button", { name: "Add rule" }).click();
    await expect(
      page.getByText("Couldn't save the rule").first(),
    ).toBeVisible();
    await expect(page.getByLabel("When Hikaya writes")).toHaveValue("...");
  });

  test("is in French for a French system", async ({ page }) => {
    await mockTauri(page, {
      settings: { app_language: "fr" },
      locale: "fr-FR",
    });
    await page.goto("/");
    await page.getByText("Apprentissage", { exact: true }).click();
    await expect(page.getByText("Règles de remplacement")).toBeVisible();
    await expect(page.getByLabel("Quand Hikaya écrit")).toBeVisible();
  });
});

test.describe("Voice commands", () => {
  test("lists the commands and saves the switch", async ({ page }) => {
    await mockTauri(page);
    await openLearning(page);
    await expect(
      page.getByText("Voice commands", { exact: true }),
    ).toBeVisible();
    await expect(page.getByText("“delete that”, “supprime ça”")).toBeVisible();
    const toggle = page.getByRole("checkbox", {
      name: /Act on voice commands/,
    });
    await expect(toggle).toBeChecked();
    await flip(toggle);
    await expect(toggle).not.toBeChecked();
    const saved = await page.evaluate(() =>
      (window as any).__HIKAYA_MOCK__.state.calls.filter(
        (c: { cmd: string }) => c.cmd === "hikaya_set_prefs",
      ),
    );
    expect(saved).toEqual([
      {
        cmd: "hikaya_set_prefs",
        args: { prefs: { voice_commands: false, adapt_to_app: true } },
      },
    ]);
  });

  test("command mode shows its shortcut, named in Hikaya's words", async ({
    page,
  }) => {
    await mockTauri(page);
    await openLearning(page);
    await expect(page.getByText("Command mode", { exact: true })).toBeVisible();
    await expect(page.getByText("Command Mode", { exact: true })).toBeVisible();
    await expect(page.getByText(/make it more formal/)).toBeVisible();
  });

  test("command mode is in French for a French system", async ({ page }) => {
    await mockTauri(page, {
      settings: { app_language: "fr" },
      locale: "fr-FR",
    });
    await page.goto("/");
    await page.getByText("Apprentissage", { exact: true }).click();
    await expect(
      page.getByText("Mode commande", { exact: true }).first(),
    ).toBeVisible();
    await expect(page.getByText(/rends-le plus formel/)).toBeVisible();
  });

  test("adapting to the app has its own switch", async ({ page }) => {
    await mockTauri(page);
    await openLearning(page);
    await expect(
      page.getByText("Adapt to the app", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("commands stay in lowercase, as spoken"),
    ).toBeVisible();
    const toggle = page.getByRole("checkbox", {
      name: /Adapt to the app you dictate into/,
    });
    await expect(toggle).toBeChecked();
    await flip(toggle);
    const saved = await page.evaluate(() =>
      (window as any).__HIKAYA_MOCK__.state.calls.filter(
        (c: { cmd: string }) => c.cmd === "hikaya_set_prefs",
      ),
    );
    expect(saved).toEqual([
      {
        cmd: "hikaya_set_prefs",
        args: { prefs: { voice_commands: true, adapt_to_app: false } },
      },
    ]);
  });

  test("is in French for a French system", async ({ page }) => {
    await mockTauri(page, {
      settings: { app_language: "fr" },
      locale: "fr-FR",
    });
    await page.goto("/");
    await page.getByText("Apprentissage", { exact: true }).click();
    await expect(
      page.getByText("Commandes vocales", { exact: true }),
    ).toBeVisible();
    await expect(page.getByText("« à la ligne », « new line »")).toBeVisible();
  });
});

test.describe("Snippets", () => {
  test("saves a multi-line snippet, lists it and deletes it", async ({
    page,
  }) => {
    await mockTauri(page);
    await openLearning(page);
    await expect(page.getByText("No snippets yet.")).toBeVisible();
    await page.getByLabel("When I say").fill("my address");
    await page
      .getByLabel("Hikaya types")
      .fill("12 rue de la Paix\n75002 Paris");
    await page.getByRole("button", { name: "Save snippet" }).click();
    await expect(page.getByText("“my address”")).toBeVisible();
    await expect(page.getByText("No snippets yet.")).toBeHidden();
    await expect(page.getByLabel("When I say")).toHaveValue("");
    await page
      .getByRole("button", { name: "Delete the snippet “my address”" })
      .click();
    await expect(page.getByText("No snippets yet.")).toBeVisible();
  });

  test("is in French for a French system", async ({ page }) => {
    await mockTauri(page, {
      settings: { app_language: "fr" },
      locale: "fr-FR",
    });
    await page.goto("/");
    await page.getByText("Apprentissage", { exact: true }).click();
    await expect(page.getByText("Extraits", { exact: true })).toBeVisible();
    await expect(page.getByLabel("Quand je dis")).toBeVisible();
  });
});

test.describe("Correcting a dictation in History", () => {
  test("saves the corrected text and says what was learned", async ({
    page,
  }) => {
    await mockTauri(page, {
      history: [{ id: 1, text: "merci a med pour la démo" }],
    });
    await page.goto("/");
    await page.getByText("History", { exact: true }).click();
    await expect(page.getByText("merci a med pour la démo")).toBeVisible();

    await page.getByRole("button", { name: "Correct this text" }).click();
    const editor = page.getByLabel("Corrected text");
    await expect(editor).toHaveValue("merci a med pour la démo");
    const save = page.getByRole("button", { name: "Save correction" });
    await expect(save).toBeDisabled();

    await editor.fill("merci Ahmed pour la démo");
    await save.click();
    await expect(
      page.getByText("Noted 1 correction: a med → Ahmed"),
    ).toBeVisible();
    const saved = await page.evaluate(
      () =>
        (window as any).__HIKAYA_MOCK__.state.calls.find(
          (c: { cmd: string }) => c.cmd === "hikaya_correct_history_entry",
        )?.args,
    );
    expect(saved).toEqual({ id: 1, correctedText: "merci Ahmed pour la démo" });
  });

  test("Cancel closes the editor without saving", async ({ page }) => {
    await mockTauri(page, { history: [{ id: 1, text: "hello world" }] });
    await page.goto("/");
    await page.getByText("History", { exact: true }).click();
    await page.getByRole("button", { name: "Correct this text" }).click();
    await page.getByLabel("Corrected text").fill("changed");
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByLabel("Corrected text")).toBeHidden();
    await expect(page.getByText("hello world")).toBeVisible();
  });
});
