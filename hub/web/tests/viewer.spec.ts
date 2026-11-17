import {
  test,
  expect,
  type APIRequestContext,
  type Page,
} from "@playwright/test";

const PASSPHRASE = "web viewer test passphrase";

/** Sets up the vault and a "laptop" device with a few dictations, once. */
const seed = async (request: APIRequestContext) => {
  const status = await (await request.get("/api/status")).json();
  if (status.state !== "uninitialized") return;
  expect(
    (
      await request.post("/api/init", { data: { passphrase: PASSPHRASE } })
    ).ok(),
  ).toBeTruthy();
  const { token } = await (
    await request.post("/api/devices", {
      data: { passphrase: PASSPHRASE, name: "laptop" },
    })
  ).json();
  const now = Date.now();
  const texts = [
    "La réunion est décalée à jeudi, and the release ships Friday.",
    "Ask what you can do for your community.",
    '<img src=x onerror="window.__pwned=1"> <script>window.__pwned=2</script> plain text',
  ];
  const response = await request.post("/api/sync/push", {
    headers: { authorization: `Bearer ${token}` },
    data: {
      dictations: texts.map((text, i) => ({
        id: `seed-${i}`,
        created_at: now - (texts.length - i) * 60_000,
        updated_at: now - (texts.length - i) * 60_000,
        text,
        language: i === 0 ? "fr" : "en",
      })),
    },
  });
  expect(response.ok()).toBeTruthy();
  const meetings = await request.post("/api/sync/push", {
    headers: { authorization: `Bearer ${token}` },
    data: {
      meetings: [
        {
          id: "meeting-1",
          title: "Point produit",
          started_at: now - 3_600_000,
          duration_ms: 1_865_000,
          updated_at: now,
          summary:
            "## TL;DR\nLa démo est décalée à jeudi.\n\n## Action items\n- [ ] You: prévenir Sarah\n- <b>not bold</b>",
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
              text: "Oui, jeudi 14h.",
            },
          ],
        },
      ],
    },
  });
  expect(meetings.ok()).toBeTruthy();
};

test.beforeEach(async ({ request }) => seed(request));

/** Pairs a fresh browser through the page, as a user would. */
const pair = async (page: Page, name = "iPhone Safari") => {
  await page.goto("/");
  const form = page.locator("#pair");
  await form.getByLabel("Name for this browser").fill(name);
  await form.getByLabel("Hub passphrase").fill(PASSPHRASE);
  await form.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByText(`Paired as ${name}`)).toBeVisible();
};

test("pairs this browser, then lists and searches the archive", async ({
  page,
}) => {
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Open your archive" }),
  ).toBeVisible();
  // Only the pairing form is on screen.
  await expect(
    page.getByRole("heading", { name: "The hub is locked" }),
  ).toBeHidden();
  await pair(page);
  await expect(page.getByRole("listitem")).toHaveCount(3);
  // Newest first, with the device it came from.
  await expect(page.getByRole("listitem").first()).toContainText("laptop");

  await page.getByRole("searchbox").fill("reunion");
  await expect(page.getByRole("listitem")).toHaveCount(1);
  await expect(page.locator("mark")).toHaveText("réunion");

  await page.getByRole("searchbox").fill("nothing like this");
  await expect(page.getByText("Nothing matches.")).toBeVisible();
});

test("dictation text is shown as text, never run as HTML", async ({ page }) => {
  await pair(page, "xss check");
  await expect(page.getByRole("listitem")).toHaveCount(3);
  await expect(
    page.getByText('<img src=x onerror="window.__pwned=1">', { exact: false }),
  ).toBeVisible();
  expect(await page.evaluate(() => (window as any).__pwned)).toBeUndefined();
  expect(await page.locator("#results img, #results script").count()).toBe(0);
});

test("lists, searches and opens meetings", async ({ page }) => {
  await pair(page, "meetings check");
  await page.getByRole("tab", { name: "Meetings" }).click();
  await expect(page.getByRole("tab", { name: "Meetings" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(
    page.getByRole("button", { name: /Point produit/ }),
  ).toBeVisible();
  await expect(page.getByText(/31:05 · laptop · 2 lines/)).toBeVisible();

  await page.getByRole("searchbox").fill("sarah");
  await expect(
    page.getByRole("button", { name: /Point produit/ }),
  ).toBeVisible();
  await page.getByRole("searchbox").fill("budget");
  await expect(page.getByText("Nothing matches.")).toBeVisible();
  await page.getByRole("searchbox").fill("");

  await page.getByRole("button", { name: /Point produit/ }).click();
  await expect(
    page.getByRole("heading", { name: "Point produit" }),
  ).toBeVisible();
  await expect(page.getByRole("heading", { name: "TL;DR" })).toBeVisible();
  await expect(page.getByText("You: prévenir Sarah")).toBeVisible();
  // HTML in a summary is shown as text.
  await expect(page.getByText("<b>not bold</b>")).toBeVisible();
  expect(await page.locator("#meeting-summary b").count()).toBe(0);
  await expect(page.locator("#meeting-lines li").first()).toContainText(
    "00:00Others:On décale la démo à jeudi ?",
  );
  await expect(page.locator("#meeting-lines li").nth(1)).toContainText(
    "00:30You:Oui, jeudi 14h.",
  );

  await page.getByRole("button", { name: "← All meetings" }).click();
  await expect(
    page.getByRole("button", { name: /Point produit/ }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "Dictations" }).click();
  await expect(
    page.getByRole("listitem").filter({ hasText: "laptop" }).first(),
  ).toBeVisible();
});

test("pairing survives a reload, and a locked hub is unlocked from the page", async ({
  page,
  request,
}) => {
  await pair(page, "lock check");
  await page.reload();
  await expect(page.getByText("Paired as lock check")).toBeVisible();
  await expect(page.getByRole("listitem")).toHaveCount(3);
  // Lock it, as a reboot would.
  const { token } = await (
    await request.post("/api/devices", {
      data: { passphrase: PASSPHRASE, name: "locker" },
    })
  ).json();
  await request.post("/api/lock", {
    headers: { authorization: `Bearer ${token}` },
  });

  await page.reload();
  await expect(
    page.getByRole("heading", { name: "The hub is locked" }),
  ).toBeVisible();
  await expect(page.getByRole("listitem")).toHaveCount(0);
  await page.locator("#unlock").getByLabel("Hub passphrase").fill(PASSPHRASE);
  await page.getByRole("button", { name: "Unlock" }).click();
  // Unlocked, still paired: straight back to the archive.
  await expect(page.getByText("Paired as lock check")).toBeVisible();
  await expect(page.getByRole("listitem")).toHaveCount(3);
});

test("a wrong passphrase is refused and the field is cleared", async ({
  page,
  request,
}) => {
  const { token } = await (
    await request.post("/api/devices", {
      data: { passphrase: PASSPHRASE, name: "locker 2" },
    })
  ).json();
  await request.post("/api/lock", {
    headers: { authorization: `Bearer ${token}` },
  });
  await page.goto("/");
  const unlock = page.locator("#unlock");
  await unlock.getByLabel("Hub passphrase").fill("wrong passphrase!!");
  await unlock.getByRole("button", { name: "Unlock" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "the passphrase is wrong",
  );
  await expect(unlock.getByLabel("Hub passphrase")).toHaveValue("");
});

test("is in French for a French browser", async ({ browser }) => {
  const context = await browser.newContext({ locale: "fr-FR" });
  const page = await context.newPage();
  await page.goto("http://127.0.0.1:18800/");
  // The previous test left the hub locked, with a back-off after the wrong try.
  await expect(
    page.getByRole("heading", { name: "Le hub est verrouillé" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Déverrouiller" }),
  ).toBeVisible();
  await context.close();
});

test("records on the phone, and a computer turns it into a meeting", async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);
  // The test before locks the hub after a wrong passphrase; unlock it once
  // the wait it imposes is over.
  await expect
    .poll(
      async () => {
        const status = await (await request.get("/api/status")).json();
        if (status.state === "unlocked") return "unlocked";
        const response = await request.post("/api/unlock", {
          data: { passphrase: PASSPHRASE },
        });
        return response.ok() ? "unlocked" : String(response.status());
      },
      { timeout: 15_000 },
    )
    .toBe("unlocked");
  await pair(page, "recorder");
  await page.getByRole("tab", { name: "Record" }).click();
  await page.getByLabel("Title (optional)").fill("Café avec Sam");
  await page.getByRole("button", { name: "● Start recording" }).click();
  await expect(page.locator("#rec-status")).toContainText("Recording");
  await expect(page.locator("#rec-list li").first()).toContainText(
    "Café avec Sam",
  );
  // A little more than two chunks of audio.
  await page.waitForTimeout(11_000);
  await page.getByRole("button", { name: "■ Stop" }).click();
  await expect(page.locator("#rec-status")).toHaveText(
    "Everything is uploaded.",
    { timeout: 20_000 },
  );
  await expect(page.locator("#rec-list li").first()).toContainText(
    "Waiting for your computer",
  );

  // The computer's side, as the desktop app does it.
  const { token } = await (
    await request.post("/api/devices", {
      data: { passphrase: PASSPHRASE, name: "gpu laptop" },
    })
  ).json();
  const auth = { authorization: `Bearer ${token}` };
  const claimed = await request.post("/api/recordings/claim", {
    headers: auth,
  });
  expect(claimed.status()).toBe(200);
  const recording = await claimed.json();
  expect(recording.title).toBe("Café avec Sam");
  expect(recording.chunks).toBeGreaterThanOrEqual(2);
  const audio = await request.get(`/api/recordings/${recording.id}/audio`, {
    headers: auth,
  });
  const bytes = await audio.body();
  expect(bytes.length).toBe(recording.bytes);
  // A whole file: WebM's EBML magic or MP4's ftyp box at the start.
  const webm = bytes.subarray(0, 4).toString("hex") === "1a45dfa3";
  const mp4 = bytes.subarray(4, 8).toString("latin1") === "ftyp";
  expect(webm || mp4).toBeTruthy();

  const pushed = await request.post("/api/sync/push", {
    headers: auth,
    data: {
      meetings: [
        {
          id: "from-phone",
          title: recording.title,
          started_at: recording.created_at,
          duration_ms: 11_000,
          updated_at: Date.now(),
          summary: null,
          lines: [
            {
              start_ms: 0,
              end_ms: 11_000,
              speaker: "room",
              text: "On se retrouve jeudi.",
            },
          ],
        },
      ],
    },
  });
  expect(pushed.ok()).toBeTruthy();
  const completed = await request.post(
    `/api/recordings/${recording.id}/complete`,
    { headers: auth, data: { meeting_id: "from-phone" } },
  );
  expect(completed.ok()).toBeTruthy();

  await page.getByRole("tab", { name: "Meetings" }).click();
  await page.getByRole("tab", { name: "Record" }).click();
  const done = page.locator("#rec-list li").first();
  await expect(done).toContainText("Transcribed");
  await done.getByRole("button", { name: "Open" }).click();
  await expect(
    page.getByRole("heading", { name: "Café avec Sam" }),
  ).toBeVisible();
  await expect(page.locator("#meeting-lines")).toContainText(
    "Room:On se retrouve jeudi.",
  );
});
