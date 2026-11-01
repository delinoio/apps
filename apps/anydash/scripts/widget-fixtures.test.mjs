// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { test } from "node:test";

test("committed widget fixtures match the TypeScript bindings", () => {
  const result = spawnSync(
    process.execPath,
    [
      "--experimental-transform-types",
      "--no-warnings",
      "--input-type=module",
      "-e",
      `import { fixtures } from "./scripts/widget-fixtures.ts"; for (const [name, bytes] of Object.entries(fixtures())) console.log(name, Buffer.from(bytes).toString("base64"));`,
    ],
    { cwd: new URL("../", import.meta.url), encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  for (const line of result.stdout.trim().split("\n")) {
    const [name, base64] = line.split(" ");
    assert.equal(
      readFileSync(
        new URL(`../native/widgets/fixtures/${name}`, import.meta.url),
      ).toString("base64"),
      base64,
      `${name} is stale; run pnpm fixtures:widgets`,
    );
  }
});
