// SPDX-License-Identifier: Apache-2.0
import { build } from "esbuild";
export async function buildOnline(outdir = "dist") {
  await build({
    entryPoints: ["web/online.ts"],
    bundle: true,
    format: "esm",
    platform: "browser",
    target: "es2022",
    outfile: `${outdir}/online.js`,
    sourcemap: true,
  });
}
if (process.argv[1]?.endsWith("/build.mjs")) await buildOnline();
