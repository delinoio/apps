// SPDX-License-Identifier: Apache-2.0
import { cp, mkdir, rm, readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
const root = fileURLToPath(new URL("../", import.meta.url));
const output = join(root, "dist");
const manifest = JSON.parse(
  await readFile(join(root, "src/manifest.json"), "utf8"),
);
if (
  manifest.incognito !== "not_allowed" ||
  manifest.host_permissions ||
  manifest.externally_connectable
)
  throw new Error("UNEXPECTED_EXTENSION_PERMISSIONS");
await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
await cp(join(root, "src"), output, { recursive: true });
await cp(join(root, "../../LICENSE"), join(output, "LICENSE"));
await cp(join(root, "../../NOTICE"), join(output, "NOTICE"));
await cp(
  join(root, "../lane/assets/realqa-icon.png"),
  join(output, "icon.png"),
);
manifest.icons = { 128: "icon.png" };
await writeFile(
  join(output, "manifest.json"),
  JSON.stringify(manifest, null, 2) + "\n",
);
for (const source of ["worker.js", "selector.js", "popup.js", "protocol.js"])
  execFileSync(process.execPath, ["--check", join(output, source)], {
    stdio: "inherit",
  });
console.info(
  JSON.stringify({
    event: "lane_chrome_extension_built",
    version: manifest.version,
  }),
);
