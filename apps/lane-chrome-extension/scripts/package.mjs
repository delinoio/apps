// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { readdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
const root = fileURLToPath(new URL("../", import.meta.url));
execFileSync(process.execPath, [join(root, "scripts/build.mjs")], {
  stdio: "inherit",
});
const output = join(root, "dist");
// Python's standard library writes a portable archive without external packages.
execFileSync(
  "python3",
  [
    "-c",
    'import pathlib,zipfile; p=pathlib.Path.cwd(); z=zipfile.ZipFile(p/"realqa-chrome.zip","w",zipfile.ZIP_DEFLATED); [z.write(f,f.relative_to(p)) for f in sorted(p.rglob("*")) if f.is_file() and f.suffix!=".zip"]; z.close()',
  ],
  { cwd: output, stdio: "inherit" },
);
console.info(
  JSON.stringify({
    event: "lane_chrome_extension_packaged",
    files: (await readdir(output)).length,
  }),
);
