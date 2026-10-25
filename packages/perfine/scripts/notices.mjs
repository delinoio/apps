// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
const supplementDirectory = fileURLToPath(
  new URL("../license-supplements/", import.meta.url),
);

// These published archives omit upstream license files. Remove each supplement
// when an upgraded crate includes its license texts; never extend it implicitly.
export function supplementNotices(p, directory = supplementDirectory) {
  const supplements = JSON.parse(
    readFileSync(join(directory, "manifest.json"), "utf8"),
  );
  const entry = supplements.find(
    (entry) => entry.name === p.name && entry.version === p.version,
  );
  if (!entry)
    throw new Error(
      `No license text for ${p.name} ${p.version}; review missing upstream texts`,
    );
  for (const field of ["source", "repository", "license"])
    if (p[field] !== entry[field])
      throw new Error(`License supplement ${field} mismatch for ${p.name}`);
  const vcs = JSON.parse(
    readFileSync(
      join(dirname(p.manifest_path), ".cargo_vcs_info.json"),
      "utf8",
    ),
  );
  if (vcs.git?.sha1 !== entry.revision)
    throw new Error(`License supplement revision mismatch for ${p.name}`);
  return entry.files
    .map((file) => {
      const bytes = readFileSync(join(directory, file.path));
      if (createHash("sha256").update(bytes).digest("hex") !== file.sha256)
        throw new Error(
          `License supplement checksum mismatch for ${p.name}: ${file.path}`,
        );
      return `Upstream license: ${entry.repository}/blob/${entry.revision}/${file.upstreamPath}\nSHA256: ${file.sha256}\n\n${bytes.toString("utf8")}`;
    })
    .join("\n");
}

export function collectNotices(root, target = "x86_64-unknown-linux-gnu") {
  const metadata = JSON.parse(
    execFileSync(
      "cargo",
      [
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--filter-platform",
        target,
      ],
      { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
    ),
  );
  return noticesFromMetadata(metadata);
}

export function noticesFromMetadata(metadata) {
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
      const text = files.length
        ? files
            .sort()
            .map((f) => readFileSync(join(directory, f), "utf8"))
            .join("\n")
        : supplementNotices(p);
      if (!text.trim())
        throw new Error(`Empty license text for ${p.name} ${p.version}`);
      return `## ${p.name} ${p.version}\nSource: ${p.source}\nLicense: ${p.license ?? "See license file"}\n\n${text}\n`;
    })
    .join("\n");
}
