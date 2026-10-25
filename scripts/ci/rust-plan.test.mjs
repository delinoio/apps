// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { mkdtempSync, rmSync, writeFileSync, chmodSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { selectRust } from "./rust-plan.mjs";

const base = "a".repeat(40);
function planner(paths, direct, affected, mergeBase = base) {
  const calls = [];
  const result = selectRust(base, paths, (...args) => {
    calls.push(args);
    if (args[0] === "list")
      return { packages: [{ name: "core" }, { name: "app" }] };
    return {
      merge_base: mergeBase,
      packages: args.includes("--direct-only") ? direct : affected,
    };
  });
  assert.deepEqual(calls.slice(0, 2), [
    ["changed", "--base", base, "--direct-only"],
    ["changed", "--base", base],
  ]);
  return result;
}

test("Clippy uses direct changes while Test includes reverse dependencies", () => {
  assert.deepEqual(
    planner(["crates/core/src/lib.rs"], ["core"], ["app", "core"]),
    {
      rust_clippy: ["core"],
      rust_test: ["app", "core"],
    },
  );
});

test("empty Rust changes skip both checks", () => {
  assert.deepEqual(planner(["apps/app/src/App.tsx"], [], []), {
    rust_clippy: [],
    rust_test: [],
  });
});

for (const path of [
  ".github/workflows/CI.yml",
  "scripts/ci/rust-plan.mjs",
  ".cargo/config.toml",
  "rust-toolchain.toml",
  "prebuilt-dependencies.lock.json",
]) {
  test(`shared Rust input selects every package: ${path}`, () => {
    assert.deepEqual(planner([path], [], []), {
      rust_clippy: ["core", "app"],
      rust_test: ["core", "app"],
    });
  });
}

test("rewritten history conservatively validates the workspace", () => {
  assert.deepEqual(planner([], [], [], "b".repeat(40)), {
    rust_clippy: ["core", "app"],
    rust_test: ["core", "app"],
  });
});

test("cargo-mono failures fail selection", () => {
  assert.throws(
    () =>
      selectRust(base, [], () => {
        throw new Error("cargo-mono failed");
      }),
    /cargo-mono failed/u,
  );
});

test("cargo-mono JSON parsing suppresses stdout tracing logs", (t) => {
  const cwd = mkdtempSync(join(tmpdir(), "rust-plan-json-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const cargo = join(cwd, "cargo");
  writeFileSync(
    cargo,
    `#!/usr/bin/env node
if (process.env.RUST_LOG !== "off") console.log(" INFO cargo-mono tracing");
console.log(JSON.stringify({merge_base: "${base}", packages: ["core"]}));
`,
  );
  chmodSync(cargo, 0o755);
  const result = spawnSync(
    process.execPath,
    [
      "--input-type=module",
      "-e",
      `
    import { selectRust } from ${JSON.stringify(new URL("./rust-plan.mjs", import.meta.url).href)};
    console.log(JSON.stringify(selectRust("${base}", [])));
  `,
    ],
    {
      encoding: "utf8",
      env: { ...process.env, PATH: `${cwd}:${process.env.PATH}` },
    },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), {
    rust_clippy: ["core"],
    rust_test: ["core"],
  });
});

test("Go checksums do not select Rust packages", () => {
  assert.deepEqual(planner(["go.sum"], [], []), {
    rust_clippy: [],
    rust_test: [],
  });
});

test("Cargo lockfiles preserve cargo-mono global-impact handling", () => {
  assert.deepEqual(planner(["Cargo.lock"], ["core", "app"], ["core", "app"]), {
    rust_clippy: ["core", "app"],
    rust_test: ["core", "app"],
  });
});

test("instruction-only changes skip Rust allocation without cargo-mono", () => {
  assert.deepEqual(
    selectRust(
      base,
      [
        "AGENTS.md",
        "scripts/ci/AGENTS.md",
        "apps/delitools/src-tauri/CLAUDE.md",
      ],
      () => {
        assert.fail("instruction-only changes must not invoke cargo-mono");
      },
    ),
    { rust_clippy: [], rust_test: [] },
  );
});

test("instruction files do not force global Rust selection with other changes", () => {
  assert.deepEqual(
    planner(
      ["scripts/ci/AGENTS.md", "crates/core/src/lib.rs"],
      ["core"],
      ["app", "core"],
    ),
    { rust_clippy: ["core"], rust_test: ["app", "core"] },
  );
});

test("Perfine core selects its root CLI reverse dependency", () => {
  assert.deepEqual(
    planner(
      ["crates/perfine-core/src/lib.rs"],
      ["perfine-core"],
      ["perfine-core", "perfine"],
    ),
    { rust_clippy: ["perfine-core"], rust_test: ["perfine-core", "perfine"] },
  );
});

test("Perfine contracts and distribution select available core and CLI consumers only", () => {
  for (const path of [
    "actions/perfine/src/main.ts",
    "packages/perfine/package.json",
    "docs/crates-perfine-adapters-contract.md",
    "protos/perfine/v1/perfine.proto",
  ]) {
    const result = selectRust(base, [path], (command) =>
      command === "list"
        ? {
            packages: ["perfine-core", "perfine", "delitools-shell"].map(
              (name) => ({ name }),
            ),
          }
        : { merge_base: base, packages: [] },
    );
    assert.deepEqual(result, {
      rust_clippy: ["perfine-core", "perfine"],
      rust_test: ["perfine-core", "perfine"],
    });
  }
});

test("Lane contracts, schemas and packages select the root Rust CLI", () => {
  for (const path of [
    "packages/lane/scripts/build.mjs",
    "protos/lane/v1/lane.proto",
    "apps/lane/native/store/schema.sql",
    "docs/cmds-lane-cli-contract.md",
  ]) {
    const result = selectRust(base, [path], (command) =>
      command === "list"
        ? {
            packages: ["lane", "perfine", "delitools-shell"].map((name) => ({
              name,
            })),
          }
        : { merge_base: base, packages: [] },
    );
    assert.deepEqual(result, { rust_clippy: ["lane"], rust_test: ["lane"] });
  }
});
