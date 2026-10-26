// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { readFile } from "node:fs/promises";
test("selector cancels without capture, supports keyboard and removes overlay before capture", async () => {
  const listeners = new Map(),
    overlays = [],
    sent = [];
  const element = (
    id,
    rect = {
      x: 5,
      y: 6,
      width: 20,
      height: 15,
      right: 25,
      bottom: 21,
      left: 5,
      top: 6,
    },
  ) => ({
    id,
    nodeType: 1,
    localName: "div",
    parentElement: null,
    style: {},
    getBoundingClientRect: () => rect,
    setAttribute() {},
    remove() {
      overlays.splice(overlays.indexOf(this), 1);
    },
  });
  const target = element("target"),
    body = element("body");
  body.querySelectorAll = () => [target];
  const document = {
    body,
    title: "Safe title",
    createElement: () => element(""),
    documentElement: { append: (...items) => overlays.push(...items) },
    addEventListener: (name, fn) => listeners.set(name, fn),
    removeEventListener: (name) => listeners.delete(name),
  };
  const context = vm.createContext({
    document,
    getComputedStyle: () => ({ overflowX: "hidden", overflowY: "visible" }),
    scrollX: 0,
    scrollY: 0,
    innerWidth: 100,
    innerHeight: 100,
    location: { href: "https://example.com/test" },
    CSS: { escape: (value) => value },
    requestAnimationFrame: (fn) => setImmediate(fn),
    chrome: {
      runtime: {
        sendMessage: async (value) => {
          assert.equal(overlays.length, 0);
          sent.push(value);
        },
      },
    },
  });
  const source = await readFile(
    new URL("../src/selector.js", import.meta.url),
    "utf8",
  );
  const event = (key) => ({
    key,
    preventDefault() {
      this.prevented = true;
    },
    stopImmediatePropagation() {},
  });
  vm.runInContext(source, context);
  const escape = event("Escape");
  listeners.get("keydown")(escape);
  assert.equal(escape.prevented, true);
  assert.equal(overlays.length, 0);
  assert.equal(sent.length, 0);
  vm.runInContext(source, context);
  listeners.get("keydown")(event("Tab"));
  listeners.get("keydown")(event("Tab"));
  listeners.get("keydown")(event("Enter"));
  await new Promise((resolve) =>
    setImmediate(() => setImmediate(() => setImmediate(resolve))),
  );
  assert.equal(sent.length, 1);
  assert.equal(sent[0].metadata.cssSelector, "#target");
  assert.equal(sent[0].rect.width, 20);
  assert.equal(listeners.size, 0);
  const parent = element("scroller", {
    x: 10,
    y: 0,
    left: 10,
    top: 0,
    right: 20,
    bottom: 50,
    width: 10,
    height: 50,
  });
  Object.assign(parent, {
    offsetWidth: 10,
    offsetHeight: 50,
    clientWidth: 10,
    clientHeight: 50,
    clientLeft: 0,
    clientTop: 0,
  });
  target.parentElement = parent;
  vm.runInContext(source, context);
  await listeners.get("click")({
    target,
    preventDefault() {},
    stopImmediatePropagation() {},
  });
  assert.equal(sent[1].rect.x, 10);
  assert.equal(sent[1].rect.width, 10);
  assert.equal(sent[1].clipped, true);
});
