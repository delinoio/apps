// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";
import postcss from "postcss";
import config from "@packages/ui/postcss";

test("consumer CSS imports generate package utilities, themes and animations", async () => {
  const from = fileURLToPath(
    new URL("../src/styles/globals.css", import.meta.url),
  );
  const css = await readFile(from, "utf8");
  const result = await postcss(config.plugins).process(css, { from });
  assert.equal(result.warnings().length, 0);
  for (const selector of [
    ".bg-primary",
    ".text-primary-foreground",
    ".fixed",
    ".dark",
    "@keyframes enter",
  ]) {
    assert.ok(result.css.includes(selector), `Missing ${selector}`);
  }
  assert.ok(!result.css.includes('@import "tailwindcss"'));
  assert.ok(!result.css.includes("@apply"));
});

test("app stylesheet resolves the public import from outside the package", async () => {
  const from = fileURLToPath(
    new URL("../../../apps/devbase/src/ui-smoke.css", import.meta.url),
  );
  const result = await postcss(config.plugins).process(
    '@import "@packages/ui/styles.css"; @source inline("p-13");',
    { from },
  );
  assert.ok(result.css.includes(".bg-primary"));
  assert.ok(result.css.includes(".p-13"));
});
