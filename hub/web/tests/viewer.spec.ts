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
