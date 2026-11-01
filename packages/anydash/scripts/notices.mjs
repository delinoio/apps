// SPDX-License-Identifier: Apache-2.0
// Collects the license and notice files of every crate linked into the CLI for
// the selected targets, plus the Rust standard library notices. A crate without
// any license text stops packaging instead of shipping an incomplete notice set.
import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { targets } from "./targets.mjs";

const noticeFile =
  /^(licen[cs]e|copying|notice|copyright|authors|patents)([._-].*)?$/i;

export function collectNotices(root, destination, selected) {
  mkdirSync(destination, { recursive: true });
  const packages = new Map();
  for (const key of selected) {
    const metadata = JSON.parse(
      execFileSync(
        "cargo",
        [
          "metadata",
          "--locked",
          "--format-version",
          "1",
          "--filter-platform",
          targets[key],
        ],
        { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
      ),
    );
    const nodes = new Map(
      metadata.resolve.nodes.map((node) => [node.id, node]),
    );
    const ids = new Set();
    const visit = (id) => {
      if (ids.has(id)) return;
      ids.add(id);
      for (const dep of nodes.get(id).deps)
        if (dep.dep_kinds.some((kind) => kind.kind !== "dev")) visit(dep.pkg);
    };
    visit(metadata.packages.find((p) => p.name === "anydash" && !p.source).id);
    for (const p of metadata.packages)
      if (ids.has(p.id) && p.source) packages.set(p.id, p);
  }
  const inventory = [];
  for (const p of [...packages.values()].sort((a, b) =>
    a.id.localeCompare(b.id),
  )) {
    const directory = dirname(p.manifest_path);
    const target = resolve(destination, `${p.name}@${p.version}`);
    let found = 0;
    const visit = (path) => {
      for (const item of readdirSync(path, { withFileTypes: true })) {
        const file = resolve(path, item.name);
        if (item.isDirectory() && item.name !== ".git") visit(file);
        else if (
          item.isFile() &&
          (noticeFile.test(item.name) ||
            relative(directory, file) === p.license_file)
        ) {
          const copy = resolve(target, relative(directory, file));
          mkdirSync(dirname(copy), { recursive: true });
          cpSync(file, copy);
          found++;
        }
      }
    };
    visit(directory);
    if (!found)
      throw new Error(
        `No license text for ${p.name} ${p.version}; review the upstream license before packaging`,
      );
    inventory.push({
      module: p.name,
      version: p.version,
      source: p.source,
      license: p.license,
    });
  }
  const sysroot = execFileSync("rustc", ["--print", "sysroot"], {
    cwd: root,
    encoding: "utf8",
  }).trim();
  cpSync(
    resolve(sysroot, "share/doc/rust/licenses"),
    resolve(destination, "rust"),
    { recursive: true },
  );
  cpSync(
    resolve(sysroot, "share/doc/rust/COPYRIGHT-library.html"),
    resolve(destination, "rust/COPYRIGHT-library.html"),
  );
  writeFileSync(
    resolve(destination, "modules.json"),
    `${JSON.stringify(inventory, null, 2)}\n`,
  );
  return inventory;
}
