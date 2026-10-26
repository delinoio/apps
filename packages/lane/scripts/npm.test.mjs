// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import test from "node:test";
import { npm, npmCommand } from "./npm.mjs";

test("Windows npm entry preserves paths with spaces and literal shell characters", (t) => {
  const directory = mkdtempSync(resolve(tmpdir(), "lane npm fixture "));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const bin = resolve(directory, "node_modules/npm/bin");
  mkdirSync(bin, { recursive: true });
  writeFileSync(
    resolve(bin, "npm-cli.js"),
    "process.stdout.write(JSON.stringify(process.argv.slice(2)));\n",
  );
  const args = [
    "pack",
    "--prefix",
    "path with spaces",
    "a&b",
    "$(echo unsafe)",
    'a"b',
  ];
  const command = npmCommand(args, {
    platform: "win32",
    commands: [
      resolve(directory, "missing/npm.cmd"),
      resolve(directory, "npm.cmd"),
    ],
  });
  assert.equal(command.file, process.execPath);
  const output = execFileSync(command.file, command.args, {
    encoding: "utf8",
    shell: false,
  });
  assert.deepEqual(JSON.parse(output), args);
});

test("Windows npm selection fails clearly when its JavaScript entry is absent", () => {
  assert.throws(
    () => npmCommand(["pack"], { platform: "win32", commands: [] }),
    /Cannot locate npm CLI/,
  );
});

test("the actual npm launcher runs without a shell on the current host", () => {
  assert.match(
    npm(["--version"], { encoding: "utf8", shell: false }).trim(),
    /^\d+\.\d+\.\d+$/,
  );
});
