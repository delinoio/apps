// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { createRequire } from "node:module";
const { launch } = createRequire(import.meta.url)("../bin/perfine.cjs");
function fixture() {
  const parent = new EventEmitter();
  const child = new EventEmitter();
  const signals = [];
  child.kill = (signal) => signals.push(signal);
  const calls = [];
  const spawnProcess = (...args) => {
    calls.push(args);
    return child;
  };
  return { parent, child, signals, calls, spawnProcess };
}
test("launcher preserves arguments, inherited streams, and exact exit status", () => {
  const f = fixture();
  const args = [
    "--repository",
    "C:\\path with spaces\\한글",
    "literal;$(command)",
    "mcp",
    "serve",
  ];
  launch("/path with spaces/perfine", args, f);
  assert.deepEqual(f.calls, [
    ["/path with spaces/perfine", args, { stdio: "inherit", shell: false }],
  ]);
  f.child.emit("exit", 42, null);
  assert.equal(f.parent.exitCode, 42);
  assert.equal(f.parent.listenerCount("SIGINT"), 0);
});
test("launcher forwards repeated cancellation requests and cleans up signal handlers", () => {
  for (const signal of ["SIGINT", "SIGTERM"]) {
    const f = fixture();
    launch("perfine", [], f);
    f.parent.emit(signal);
    f.parent.emit(signal);
    assert.deepEqual(f.signals, [signal, signal]);
    f.child.emit("exit", null, signal);
    assert.equal(f.parent.exitCode, signal === "SIGINT" ? 130 : 143);
    assert.equal(f.parent.listenerCount(signal), 0);
  }
});
test("failed executable startup sets failure and removes signal handlers", (t) => {
  const log = t.mock.method(console, "error", () => {});
  const f = fixture();
  launch("missing", [], f);
  f.child.emit("error", new Error("EACCES"));
  assert.equal(f.parent.exitCode, 1);
  assert.equal(f.parent.listenerCount("SIGTERM"), 0);
  assert.match(log.mock.calls[0].arguments[0], /EACCES.*Reinstall/);
});
