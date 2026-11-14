// SPDX-License-Identifier: Apache-2.0
import { describe, test } from "@e2e-dev/web";
import { expect } from "e2e";

// Must equal the PUBLIC_API_ORIGIN that `e2e.config.ts` passes to the app
// command. A value other than the production default proves that the
// build-time variable reaches the client bundle: the default would also appear
// if the variable were ignored.
const E2E_API_ORIGIN = "http://127.0.0.1:18100";

describe("home", () => {
  test("shows the account and billing sections", async ({ app, screen }) => {
    await app.open("/");

    await expect(
      screen.getByRole("heading", "Delino Accounts", { level: 1 }),
    ).toBeVisible();
    await expect(
      screen.getByRole("heading", "Account", { level: 2 }),
    ).toBeVisible();
    await expect(
      screen.getByRole("heading", "Billing", { level: 2 }),
    ).toBeVisible();
  });

  test("exposes the configured API origin on the document", async ({
    app,
    browser,
  }) => {
    await app.open("/");

    await expect(browser.locator("html")).toHaveAttribute(
      "data-api-origin",
      E2E_API_ORIGIN,
    );
  });

  test(
    "marks every section as coming soon",
    { tags: ["agent"] },
    async ({ app, agent }) => {
      await app.open("/");

      await agent.assert(
        "both the Account and the Billing sections are marked as coming soon",
      );
    },
  );
});
