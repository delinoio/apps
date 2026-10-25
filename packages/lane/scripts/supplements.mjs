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
