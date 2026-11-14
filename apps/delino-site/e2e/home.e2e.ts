// SPDX-License-Identifier: Apache-2.0
import { describe, test } from "@e2e-dev/web";
import { expect } from "e2e";

describe("home", () => {
  test("shows the hero heading", async ({ app, screen }) => {
    await app.open("/");

    await expect(
      screen.getByRole("heading", "Delino", { level: 1 }),
    ).toBeVisible();
  });

  test("links the docs action to the documentation site", async ({
    app,
    screen,
  }) => {
    await app.open("/");

    await expect(screen.getByRole("link", "Read the docs")).toHaveAttribute(
      "href",
      "https://docs.delino.io",
    );
  });

  test(
    "presents the product features",
    { tags: ["agent"] },
    async ({ app, agent }) => {
      await app.open("/");

      await agent.assert("the page lists three feature cards");
    },
  );
});
