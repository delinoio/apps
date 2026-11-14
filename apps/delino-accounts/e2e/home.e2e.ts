// SPDX-License-Identifier: Apache-2.0
import { describe, expect, test } from "e2e";

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
