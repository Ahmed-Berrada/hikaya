import { test, expect, type Page } from "@playwright/test";
import { mockTauri } from "./tauri-mock";

const PASSPHRASE = "correct horse battery staple";

const status = (page: Page, name = "Sync status") =>
  page.getByRole("status", { name });

const openSync = async (page: Page) => {
  await page.goto("/");
  await page.getByText("Sync", { exact: true }).click();
  await expect(page.getByText("Your hub")).toBeVisible();
};

test.describe("Sync screen", () => {
  test("connects to a hub and forgets the passphrase", async ({ page }) => {
    await mockTauri(page);
    await openSync(page);
    await expect(status(page)).toHaveText("Not connected");

    const connect = page.getByRole("button", { name: "Connect" });
    await expect(connect).toBeDisabled();
    await page.getByLabel("Hub address").fill("https://hub.example.ts.net/");
    await page.getByLabel("This device's name").fill("Laptop");
    await page.getByLabel("Hub passphrase").fill(PASSPHRASE);
    await connect.click();

    await expect(status(page)).toHaveText("Up to date");
    await expect(
      page.getByText("Connected to https://hub.example.ts.net as “Laptop”"),
    ).toBeVisible();
    await expect(page.getByText("Nothing waiting to be sent")).toBeVisible();
    await expect(page.getByLabel("Hub passphrase")).toBeHidden();
    const sent = await page.evaluate(
      () =>
        (window as any).__HIKAYA_MOCK__.state.calls.find(
          (c: { cmd: string }) => c.cmd === "hikaya_hub_connect",
        )?.args,
    );
    expect(sent).toEqual({
      url: "https://hub.example.ts.net/",
      deviceName: "Laptop",
      passphrase: PASSPHRASE,
    });
  });

  test("a wrong passphrase shows the hub's reason and clears the field", async ({
    page,
  }) => {
    await mockTauri(page);
    await openSync(page);
    await page.getByLabel("Hub address").fill("https://hub.example.ts.net");
    await page.getByLabel("This device's name").fill("Laptop");
    await page.getByLabel("Hub passphrase").fill("not the passphrase");
    await page.getByRole("button", { name: "Connect" }).click();
    await expect(
      page.getByText("Couldn't reach the hub").first(),
    ).toBeVisible();
    await expect(
      page.getByText("the passphrase is wrong").first(),
    ).toBeVisible();
    await expect(page.getByLabel("Hub passphrase")).toHaveValue("");
    await expect(page.getByLabel("Hub address")).toHaveValue(
      "https://hub.example.ts.net",
    );
    await expect(status(page)).toHaveText("Not connected");
  });

  test("a locked hub asks for the passphrase and unlocks", async ({ page }) => {
    await mockTauri(page, {
      hub: {
        state: "locked",
        url: "https://hub.example.ts.net",
        device_name: "Laptop",
        pending: 3,
        last_sync_at: 1763200000000,
      },
    });
    await openSync(page);
    await expect(status(page)).toHaveText("Hub locked");
    await expect(page.getByText("The hub restarted.")).toBeVisible();
    await expect(page.getByText("3 changes waiting to be sent")).toBeVisible();

    await page.getByLabel("Hub passphrase").fill(PASSPHRASE);
    await page.getByRole("button", { name: "Unlock" }).click();
    await expect(status(page)).toHaveText("Up to date");
    await expect(page.getByText("Nothing waiting to be sent")).toBeVisible();
  });

  test("an unreachable hub explains that nothing is lost", async ({ page }) => {
    await mockTauri(page, {
      hub: {
        state: "offline",
        url: "https://hub.example.ts.net",
        device_name: "Laptop",
        pending: 1,
      },
    });
    await openSync(page);
    await expect(status(page)).toHaveText("Hub unreachable");
    await expect(page.getByText("1 change waiting to be sent")).toBeVisible();
    await expect(page.getByText(/wait on this computer/)).toBeVisible();
    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(status(page)).toHaveText("Up to date");
  });

  test("disconnecting asks first", async ({ page }) => {
    await mockTauri(page, {
      hub: {
        state: "up_to_date",
        url: "https://hub.example.ts.net",
        device_name: "Laptop",
      },
    });
    await openSync(page);
    page.once("dialog", (dialog) => dialog.dismiss());
    await page.getByRole("button", { name: "Disconnect" }).click();
    await expect(status(page)).toHaveText("Up to date");
    page.once("dialog", (dialog) => dialog.accept());
    await page.getByRole("button", { name: "Disconnect" }).click();
    await expect(status(page)).toHaveText("Not connected");
  });

  test("is in French for a French system", async ({ page }) => {
    await mockTauri(page, {
      settings: { app_language: "fr" },
      locale: "fr-FR",
    });
    await page.goto("/");
    await page.getByText("Synchronisation", { exact: true }).click();
    await expect(page.getByText("Votre hub")).toBeVisible();
    await expect(status(page, "État de la synchronisation")).toHaveText(
      "Non connecté",
    );
  });
});
