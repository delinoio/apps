// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import {
  cpSync,
  mkdirSync,
  readdirSync,
  writeFileSync,
  existsSync,
} from "node:fs";
import { resolve, relative } from "node:path";
import { platforms } from "../bin/verify.mjs";
export function collectNotices(root, destination) {
  mkdirSync(destination, { recursive: true });
  const outputs = platforms.map((key) => {
    const [GOOS, GOARCH] = key.split("-");
    return execFileSync(
      "go",
      [
        "list",
        "-deps",
        "-f",
        "{{if .Module}}{{.Module.Path}}|{{.Module.Version}}|{{.Module.Dir}}{{end}}",
        "./cmds/lane",
      ],
      {
        cwd: root,
        env: { ...process.env, CGO_ENABLED: "0", GOOS, GOARCH },
        encoding: "utf8",
      },
    );
  });
  const output = outputs.join("\n");
  const entries = [...new Set(output.split("\n").filter(Boolean))].sort();
  const inventory = [];
  for (const entry of entries) {
    const [module, version, directory] = entry.split("|");
    if (module === "github.com/delinoio/apps") continue;
    if (!version || !directory)
      throw new Error(
        `unversioned/replaced dependency requires license review: ${module}`,
      );
    const target = resolve(
      destination,
      `${module.replaceAll("/", "__")}@${version}`,
    );
    let found = 0;
    function visit(path) {
      for (const item of readdirSync(path, { withFileTypes: true })) {
        const file = resolve(path, item.name);
        if (item.isDirectory() && item.name !== ".git") visit(file);
        else if (
          item.isFile() &&
          /^(licen[cs]e|copying|notice|copyright|authors|patents)([._-].*)?$/i.test(
            item.name,
          )
        ) {
          const dest = resolve(target, relative(directory, file));
          mkdirSync(resolve(dest, ".."), { recursive: true });
          cpSync(file, dest);
          found++;
        }
      }
    }
    visit(directory);
    if (!found)
      throw new Error(`missing dependency license: ${module}@${version}`);
    inventory.push({ module, version });
  }
  const goroot = execFileSync("go", ["env", "GOROOT"], {
    cwd: root,
    encoding: "utf8",
  }).trim();
  mkdirSync(resolve(destination, "go"));
  for (const name of ["LICENSE", "PATENTS"])
    if (existsSync(resolve(goroot, name)))
      cpSync(resolve(goroot, name), resolve(destination, "go", name));
  writeFileSync(
    resolve(destination, "modules.json"),
    `${JSON.stringify(inventory, null, 2)}\n`,
  );
}
