// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import {
  realpathSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  rmSync,
  cpSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { supplementNotices, noticesFromMetadata } from "./notices.mjs";
import { root } from "./build.mjs";
const supplements = join(root, "packages/perfine/license-supplements");
const entries = JSON.parse(readFileSync(join(supplements, "manifest.json")));
function fixture(t, entry = entries[0]) {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "perfine-notices-")));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const pkg = {
    ...entry,
    id: "dependency",
    manifest_path: join(dir, "Cargo.toml"),
  };
  writeFileSync(
    join(dir, ".cargo_vcs_info.json"),
    JSON.stringify({ git: { sha1: entry.revision } }),
  );
  const metadata = {
    packages: [{ name: "perfine", id: "cli" }, pkg],
    resolve: {
      nodes: [
        {
          id: "cli",
          deps: [{ pkg: "dependency", dep_kinds: [{ kind: null }] }],
        },
        { id: "dependency", deps: [] },
      ],
    },
  };
  return { dir, pkg, metadata };
}
test("reviewed supplements preserve every upstream text and immutable provenance", (t) => {
  for (const entry of entries) {
    const { pkg, metadata } = fixture(t, entry);
    const notices = noticesFromMetadata(metadata);
    for (const file of entry.files) {
      assert.ok(
        notices.includes(readFileSync(join(supplements, file.path), "utf8")),
      );
      assert.ok(
        notices.includes(
          `${entry.repository}/blob/${entry.revision}/${file.upstreamPath}`,
        ),
      );
      assert.ok(notices.includes(file.sha256));
    }
    for (const field of ["version", "name", "source", "repository", "license"])
      assert.throws(
        () => supplementNotices({ ...pkg, [field]: "changed" }),
        /No license text|mismatch/,
      );
  }
});
test("missing or changed crate VCS and corrupt or missing supplement texts fail closed", (t) => {
  const { dir, pkg } = fixture(t);
  writeFileSync(
    join(dir, ".cargo_vcs_info.json"),
    JSON.stringify({ git: { sha1: "changed" } }),
  );
  assert.throws(() => supplementNotices(pkg), /revision mismatch/);
  rmSync(join(dir, ".cargo_vcs_info.json"));
  assert.throws(() => supplementNotices(pkg), /ENOENT/);
  writeFileSync(
    join(dir, ".cargo_vcs_info.json"),
    JSON.stringify({ git: { sha1: pkg.revision } }),
  );
  const copy = join(dir, "supplements");
  cpSync(supplements, copy, { recursive: true });
  const file = join(copy, entries[0].files[0].path);
  writeFileSync(file, "corrupt");
  assert.throws(() => supplementNotices(pkg, copy), /checksum mismatch/);
  rmSync(file);
  assert.throws(() => supplementNotices(pkg, copy), /ENOENT/);
});
test("unknown missing dependencies fail; shipped license texts take precedence", (t) => {
  const { dir, pkg, metadata } = fixture(t);
  pkg.name = "unknown-dependency";
  assert.throws(() => noticesFromMetadata(metadata), /No license text/);
  writeFileSync(join(dir, "LICENSE"), "Original shipped license\n");
  assert.match(noticesFromMetadata(metadata), /Original shipped license/);
  writeFileSync(join(dir, "LICENSE"), "");
  assert.throws(() => noticesFromMetadata(metadata), /Empty license text/);
});
test("default build regenerates stale notices and rejects a changed graph on the next build", (t) => {
  const { dir, pkg, metadata } = fixture(t);
  // A disposable source tree and Cargo stub exercise the actual build entry point.
  const source = join(dir, "source");
  mkdirSync(join(source, "packages"), { recursive: true });
  cpSync(join(root, "packages/perfine"), join(source, "packages/perfine"), {
    recursive: true,
    filter: (path) =>
      !["dist", "node_modules"].includes(path.split("/").at(-1)),
  });
  cpSync(join(root, "actions/perfine"), join(source, "actions/perfine"), {
    recursive: true,
    filter: (path) =>
      !["dist", "node_modules"].includes(path.split("/").at(-1)),
  });
  for (const name of ["LICENSE", "NOTICE"])
    cpSync(join(root, name), join(source, name));
  execFileSync("git", ["init", "-q", source]);
  execFileSync(
    "git",
    [
      "-c",
      "user.name=Fixture",
      "-c",
      "user.email=fixture@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "--allow-empty",
      "-qm",
      "fixture",
    ],
    { cwd: source },
  );
  const bin = join(dir, "bin");
  mkdirSync(bin);
  const graph = join(dir, "metadata.json");
  writeFileSync(graph, JSON.stringify(metadata));
  writeFileSync(
    join(bin, "cargo"),
    '#!/bin/sh\nprintf "%s\\n" "$@" > "$FIXTURE_ARGUMENTS"\ncat "$FIXTURE_METADATA"\n',
    {
      mode: 0o755,
    },
  );
  const binary = join(dir, "binary");
  const bytes = Buffer.alloc(128);
  bytes.write("\x7fELF");
  bytes[4] = 2;
  bytes[5] = 1;
  bytes.writeUInt16LE(62, 18);
  writeFileSync(binary, bytes);
  const notices = join(source, "packages/perfine/dist/THIRD_PARTY_NOTICES");
  mkdirSync(join(source, "packages/perfine/dist"));
  writeFileSync(notices, "STALE");
  const env = {
    ...process.env,
    PATH: `${bin}:${process.env.PATH}`,
    FIXTURE_METADATA: graph,
    FIXTURE_ARGUMENTS: join(dir, "arguments"),
    PERFINE_BINARY: binary,
    PERFINE_OUTPUT: join(dir, "release"),
  };
  delete env.PERFINE_NOTICES;
  const build = () =>
    execFileSync(
      process.execPath,
      [join(source, "packages/perfine/scripts/build.mjs")],
      { env, stdio: "pipe" },
    );
  build();
  assert.doesNotMatch(readFileSync(notices, "utf8"), /STALE/);
  assert.match(readFileSync(notices, "utf8"), /Upstream license:/);
  assert.match(
    readFileSync(env.FIXTURE_ARGUMENTS, "utf8"),
    /--filter-platform\nx86_64-unknown-linux-gnu/,
  );
  bytes.fill(0);
  bytes.writeUInt32LE(0xfeedfacf, 0);
  bytes.writeUInt32LE(0x0100000c, 4);
  bytes.writeUInt32LE(2, 12);
  writeFileSync(binary, bytes);
  env.PERFINE_TARGET = "aarch64-apple-darwin";
  env.PERFINE_OUTPUT = join(dir, "darwin-release");
  build();
  assert.match(
    readFileSync(env.FIXTURE_ARGUMENTS, "utf8"),
    /--filter-platform\naarch64-apple-darwin/,
  );
  pkg.version = "99.0.0";
  writeFileSync(graph, JSON.stringify(metadata));
  env.PERFINE_OUTPUT = join(dir, "second-release");
  assert.throws(build, /No license text/);
});
