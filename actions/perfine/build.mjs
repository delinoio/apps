// SPDX-License-Identifier: Apache-2.0
import { build } from "esbuild";
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { runFormatter } from "../../scripts/ci/format-changed.mjs";
const require = createRequire(import.meta.url);
await build({
  entryPoints: ["main.mjs"],
  bundle: true,
  platform: "node",
  target: "node24",
  format: "cjs",
  outfile: "action.cjs",
  legalComments: "inline",
  // Strip esbuild's source-location comments: pnpm store paths differ by host.
  // Legal notices remain inline and in ACTION_NOTICES; formatting runs below.
  minifyWhitespace: true,
});
writeFileSync(
  "ACTION_NOTICES",
  ["js-yaml", "argparse"]
    .map((name) => {
      const manifest = require.resolve(`${name}/package.json`);
      return `${name}\n${readFileSync(join(dirname(manifest), "LICENSE"), "utf8")}`;
    })
    .join("\n"),
);

if (
  runFormatter(resolve("../.."), ["actions/perfine/action.cjs"], "write") !== 0
)
  throw new Error("Action bundle formatting failed");
