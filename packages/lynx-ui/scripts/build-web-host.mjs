// SPDX-License-Identifier: Apache-2.0
import { cp, mkdir, readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import ts from "typescript";

const require = createRequire(import.meta.url);
const runtime = dirname(require.resolve("@lynx-js/web-core/package.json"));
await mkdir("dist", { recursive: true });
await cp(resolve(runtime, "dist/client_prod"), "dist/runtime", {
  recursive: true,
});
await cp(resolve(runtime, "LICENSE.txt"), "dist/runtime/LICENSE.txt");
await cp(resolve(runtime, "Notice.txt"), "dist/runtime/Notice.txt");
await cp("../../LICENSE", "dist/LICENSE");
await cp("../../NOTICE", "dist/NOTICE");
await cp("examples/index.html", "dist/index.html");
await cp("examples/host.js", "dist/host.js");
const { outputText } = ts.transpileModule(
  await readFile("src/web.ts", "utf8"),
  {
    compilerOptions: {
      target: ts.ScriptTarget.ES2022,
      module: ts.ModuleKind.ESNext,
    },
  },
);
await writeFile("dist/web.js", outputText);
console.info("lynx-ui.gallery-built", {
  targets: ["web", "lynx"],
  output: "dist",
});
