import { test, expect, type Page } from "@playwright/test";
import { mockTauri } from "./tauri-mock";

const PAST = {
  id: 1,
  title: "Point produit",
  started_at: 1763290800000,
  duration_ms: 1_865_000,
  status: "done",
  system_audio: true,
  lines: [
    {
      start_ms: 0,
      end_ms: 30000,
      speaker: "others",
      text: "On décale la démo à jeudi ?",
    },
    {
      start_ms: 30000,
      end_ms: 60000,
      speaker: "you",
      text: "Oui, jeudi 14h. Je préviens Sarah.",
    },
  ],
};

const openMeetings = async (page: Page) => {
  await page.goto("/");
  await page.getByText("Meetings", { exact: true }).click();
};

test.describe("Meetings screen", () => {
  test("starts and stops a recording, with the consent reminder", async ({
    page,
  }) => {
    await mockTauri(page);
    await openMeetings(page);
    await expect(
      page.getByText("Tell everyone the meeting is recorded."),
    ).toBeVisible();
    await expect(page.getByText("No meetings yet.")).toBeVisible();
    await expect(
      page.getByText(/Meeting audio follows Advanced/),
    ).toBeVisible();

    await page.getByLabel("Meeting title (optional)").fill("Revue hebdo");
    await page.getByRole("button", { name: "Start recording" }).click();

    await expect(page.getByRole("status", { name: "Recording" })).toContainText(
      "Recording",
    );
    await expect(page.getByRole("status", { name: "Recording" })).toContainText(
      /01:0\d/,
    );
    await expect(page.getByText("Revue hebdo")).toBeVisible();
    await expect(
      page.getByText(/Other people: Monitor of Speakers/),
    ).toBeVisible();
    await expect(page.getByText("Bonjour, on commence ?")).toBeVisible();

    await page.getByRole("button", { name: "Stop" }).click();
    await expect(
      page.getByRole("button", { name: "Start recording" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: /Revue hebdo/ }),
    ).toBeVisible();
  });

  test("opens a past meeting, summarizes it and saves an export", async ({
    page,
  }) => {
    await mockTauri(page, { meetings: [PAST] });
    await openMeetings(page);
    await expect(page.getByText("2 lines")).toBeVisible();
    await page.getByRole("button", { name: /Point produit/ }).click();

    await expect(page.getByText("31:05")).toBeVisible();
    await expect(page.getByText("On décale la démo à jeudi ?")).toBeVisible();
    await expect(page.getByText("Others:")).toBeVisible();
    await expect(page.getByText("You:")).toBeVisible();
    await expect(page.getByText(/No summary yet/)).toBeVisible();

    await page.getByRole("button", { name: "Summarize" }).click();
    await expect(page.getByRole("heading", { name: "TL;DR" })).toBeVisible();
    await expect(page.getByText("La démo est décalée à jeudi.")).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Summarize again" }),
    ).toBeVisible();

    await page.getByRole("button", { name: "Save as Markdown…" }).click();
    await expect
      .poll(() =>
        page.evaluate(() => (window as any).__HIKAYA_MOCK__.state.saved),
      )
      .toEqual([
        {
          path: "/home/user/Documents/meeting.md",
          contents: "# Point produit\n\n## Transcript\n",
        },
      ]);
  });

  test("renames and deletes a meeting after confirmation", async ({ page }) => {
    await mockTauri(page, { meetings: [PAST] });
    await openMeetings(page);
    await page.getByRole("button", { name: /Point produit/ }).click();

    const rename = page.getByLabel("Rename");
    await rename.fill("Point produit – novembre");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await page.getByRole("button", { name: "All meetings" }).click();
    await expect(page.getByRole("button", { name: /novembre/ })).toBeVisible();

    await page.getByRole("button", { name: /novembre/ }).click();
    page.once("dialog", (d) => d.dismiss());
    await page.getByRole("button", { name: "Delete" }).click();
    await expect(page.getByText("On décale la démo à jeudi ?")).toBeVisible();
    page.once("dialog", (d) => d.accept());
    await page.getByRole("button", { name: "Delete" }).click();
    await expect(page.getByText("No meetings yet.")).toBeVisible();
  });

  test("warns when only the microphone is recorded", async ({ page }) => {
    await mockTauri(page);
    await openMeetings(page);
    await page.evaluate(() => {
      const mock = (window as any).__HIKAYA_MOCK__;
      mock.handlers.hikaya_meeting_start = () => {
        mock.state.activeMeeting = {
          id: 7,
          title: "Sur place",
          started_at: Date.now(),
          microphone: "Laptop microphone",
          system_audio: null,
          system_audio_error: "no monitor source",
        };
        return mock.state.activeMeeting;
      };
    });
    await page.getByRole("button", { name: "Start recording" }).click();
    await expect(
      page.getByText(/Only your microphone is recorded/),
    ).toBeVisible();
  });

  test("is in French for a French system", async ({ page }) => {
    await mockTauri(page, {
      settings: { app_language: "fr" },
      locale: "fr-FR",
      meetings: [PAST],
    });
    await page.goto("/");
    await page.getByText("Réunions", { exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Démarrer l'enregistrement" }),
    ).toBeVisible();
    await expect(page.getByText("2 lignes")).toBeVisible();
    await page.getByRole("button", { name: /Point produit/ }).click();
    await expect(
      page.getByText("Autres :").or(page.getByText("Autres:")),
    ).toBeVisible();
  });
});
