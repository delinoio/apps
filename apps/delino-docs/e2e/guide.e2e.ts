// SPDX-License-Identifier: Apache-2.0
import { describe, test } from "@e2e-dev/web";
import { expect } from "e2e";

describe("guide", () => {
  test("opens the introduction from the home page", async ({
    app,
    browser,
    screen,
  }) => {
    await app.open("/");

    await screen.getByRole("link", "Get started").tap();

    await expect(browser).toHaveURL("/guide/introduction");
    await expect(
      screen.getByRole("heading", "Introduction", { level: 1 }),
    ).toBeVisible();
  });

  test(
    "navigates to the getting started guide",
    { tags: ["agent"] },
    async ({ app, agent, browser, screen }) => {
      await app.open("/guide/introduction");

      await agent.act("open the Getting started guide");

      await expect(browser).toHaveURL("/guide/getting-started");
      await expect(
        screen.getByRole("heading", "Getting started", { level: 1 }),
      ).toBeVisible();
    },
  );
});
