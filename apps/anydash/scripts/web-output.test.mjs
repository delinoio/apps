// SPDX-License-Identifier: Apache-2.0
// Runs after `pnpm build`: the Cloudflare Pages output serves the CLI install
// scripts unchanged at https://anydash.delino.io/install.sh and /install.ps1.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url));

test("the web output serves the CLI install scripts as plain text", () => {
  for (const name of ["install.sh", "install.ps1"])
    assert.deepEqual(
      read(`../dist/${name}`),
      read(`../../../packages/anydash/install/${name}`),
      `${name} must match packages/anydash`,
    );
  const headers = read("../dist/_headers").toString("utf8");
  for (const name of ["install.sh", "install.ps1"])
    assert.match(
      headers,
      new RegExp(
        `^/${name.replace(".", "\\.")}\\n  Content-Type: text/plain; charset=utf-8$`,
        "m",
      ),
    );
});
