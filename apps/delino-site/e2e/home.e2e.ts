// SPDX-License-Identifier: Apache-2.0
import { describe, test } from "@e2e-dev/web";
import { expect } from "e2e";

describe("home", () => {
  test("shows the hero logo", async ({ app, screen }) => {
    await app.open("/");

    // The Rspress home hero renders its name as text, not as a heading, so
    // the logo image is the stable hero landmark.
    await expect(screen.getByRole("image", "Delino logo")).toBeVisible();
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
