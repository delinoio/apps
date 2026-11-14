// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from "node:fs";
import { describe, expect, test } from "@rstest/core";

const template = readFileSync(
  new URL("../index.html", import.meta.url),
  "utf8",
);

describe("HTML document template", () => {
  test("declares English as the document language", () => {
    expect(template).toContain('<html lang="en">');
  });

  test("keeps the mount node Rsbuild fills in", () => {
    expect(template).toContain('<div id="<%= mountId %>"></div>');
  });
});
