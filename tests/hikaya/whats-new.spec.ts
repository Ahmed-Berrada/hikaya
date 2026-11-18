import { test, expect } from "@playwright/test";
import { mockTauri } from "./tauri-mock";

test("after an update, What's New shows Hikaya's notes, never Handy's", async ({
  page,
}) => {
  await mockTauri(page, {
    appVersion: "0.4.0",
    settings: { whats_new_last_seen_version: "0.2.0" },
  });
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Record on your phone" }),
  ).toBeVisible();
  await expect(page.getByText("Transcribing Files")).toHaveCount(0);
});

test("nothing pops up when the version was already seen", async ({ page }) => {
  await mockTauri(page, { appVersion: "0.4.0" });
  await page.goto("/");
  await expect(page.getByText("Transcribe Shortcut")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Record on your phone" }),
  ).toHaveCount(0);
});
