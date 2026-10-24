// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
export function collectNotices(root) {
  const metadata = JSON.parse(
    execFileSync(
      "cargo",
      [
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--filter-platform",
        "x86_64-unknown-linux-gnu",
      ],
      { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
    ),
  );
  const cli = metadata.packages.find((p) => p.name === "perfine");
  if (!cli)
    throw new Error("CLI package perfine must be integrated before release");
  const ids = new Set();
  function visit(id) {
    if (ids.has(id)) return;
    ids.add(id);
    const node = metadata.resolve.nodes.find((n) => n.id === id);
    for (const dep of node.deps)
      if (dep.dep_kinds.some((k) => k.kind !== "dev")) visit(dep.pkg);
  }
  visit(cli.id);
  return metadata.packages
    .filter((p) => ids.has(p.id) && p.source)
    .sort((a, b) => a.id.localeCompare(b.id))
    .map((p) => {
      const directory = dirname(p.manifest_path);
      const files = readdirSync(directory, { withFileTypes: true })
        .filter(
          (f) =>
            f.isFile() &&
            /^(licen[cs]e|copying|copyright|notice)([._-]|$)/i.test(f.name),
        )
        .map((f) => f.name);
      if (p.license_file && !files.includes(p.license_file))
        files.push(p.license_file);
      if (!files.length)
        throw new Error(
          `No license text for ${p.name} ${p.version}; review and supply notices explicitly`,
        );
      return `## ${p.name} ${p.version}\nSource: ${p.source}\nLicense: ${p.license ?? "See license file"}\n\n${files
        .sort()
        .map((f) => readFileSync(join(directory, f), "utf8"))
        .join("\n")}\n`;
    })
    .join("\n");
}
