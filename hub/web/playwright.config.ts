import { defineConfig, devices } from "@playwright/test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

// Runs the hub's web viewer against a real hub built from this repository,
// with a throwaway data directory. From the repository root:
//   npx playwright test -c hub/web/playwright.config.ts
const port = 18800;
const data = mkdtempSync(join(tmpdir(), "hikaya-web-"));

export default defineConfig({
  testDir: "./tests",
  // One hub, one vault: the tests run in order and build on each other.
  workers: 1,
  fullyParallel: false,
  reporter: "list",
  use: { baseURL: `http://127.0.0.1:${port}`, trace: "off" },
  projects: [
    {
      name: "iphone",
      use: { ...devices["iPhone 14 Plus"], browserName: "chromium" },
    },
  ],
  webServer: {
    command: "cargo run --quiet --manifest-path ../Cargo.toml",
    cwd: here,
    url: `http://127.0.0.1:${port}/api/status`,
    timeout: 600_000,
    reuseExistingServer: false,
    env: {
      HIKAYA_HUB_DATA: data,
      HIKAYA_HUB_LISTEN: `127.0.0.1:${port}`,
      RUST_LOG: "hikaya_hub=warn",
    },
  },
});
