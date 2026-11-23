import { test, expect } from "@playwright/test";
import { mockTauri } from "./tauri-mock";

test.describe("Background problems", () => {
  test("a meeting with untranscribed parts is reported and stays in the footer", async ({
    page,
  }) => {
    await mockTauri(page);
    await page.goto("/");
    await expect(page.getByText("Transcribe Shortcut")).toBeVisible();
    await page.evaluate(() =>
      (window as any).__HIKAYA_MOCK__.emit("hikaya-problem", {
        kind: "meeting_incomplete",
        count: 2,
        detail: "Point produit",
      }),
    );
    await expect(
      page.getByText("Parts of a Meeting Not Transcribed").first(),
    ).toBeVisible();
    await expect(
      page.getByText(/2 parts could not be transcribed/).first(),
    ).toBeVisible();
  });
});
