// SPDX-License-Identifier: Apache-2.0

// The planner runs before dependency installation. Recognize only pnpm's
// generated importer blocks; keep global selection for any other lockfile edit.
// This can be replaced with dependency-graph selection when the planner has a
// lockfile parser available without installing workspace dependencies.
function splitImporters(source) {
  if (typeof source !== "string") return null;
  const sections = [];
  const remainder = [];
  let importers = null;
  let current = null;
  for (const line of source.split("\n")) {
    if (/^\S/u.test(line)) {
      importers = null;
      current = null;
    }
    if (line === "importers:") {
      importers = new Map();
      sections.push(importers);
      remainder.push(line);
    } else if (importers) {
      if (!line.trim()) continue;
      const match =
        /^  (\.|[a-z0-9][a-z0-9._-]*(?:\/[a-z0-9][a-z0-9._-]*)*):(?: (\{\}))?$/u.exec(
          line,
        );
      if (match) {
        current = match[1];
        if (importers.has(current)) return null;
        importers.set(current, match[2] ?? "");
        if (match[2]) current = null;
      } else if (current && /^ {4}\S|^ {5}/u.test(line)) {
        importers.set(current, importers.get(current) + line + "\n");
      } else return null;
    } else remainder.push(line);
  }
  const versions = [
    ...source.matchAll(/^lockfileVersion: ['"]?([^'"\n]+)['"]?$/gmu),
  ];
  if (
    !sections.length ||
    !versions.length ||
    versions.some((match) => match[1] !== "9.0")
  )
    return null;
  return { sections, remainder: remainder.join("\n") };
}

export function lockfilePaths(base, head) {
  const before = splitImporters(base);
  const after = splitImporters(head);
  if (
    !before ||
    !after ||
    before.remainder !== after.remainder ||
    before.sections.length !== after.sections.length
  ) {
    return ["pnpm-lock.yaml"];
  }
  const paths = new Set();
  for (let index = 0; index < before.sections.length; index++) {
    const oldImporters = before.sections[index];
    const newImporters = after.sections[index];
    for (const name of new Set([
      ...oldImporters.keys(),
      ...newImporters.keys(),
    ])) {
      if (oldImporters.get(name) === newImporters.get(name)) continue;
      if (name === ".") return ["pnpm-lock.yaml"];
      paths.add(`${name}/package.json`);
    }
  }
  return [...paths];
}
