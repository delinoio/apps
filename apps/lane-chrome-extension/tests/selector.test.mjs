// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { readFile } from "node:fs/promises";

const source = await readFile(
  new URL("../src/selector.js", import.meta.url),
  "utf8",
);
const settle = () =>
  new Promise((resolve) =>
    setImmediate(() => setImmediate(() => setImmediate(resolve))),
  );

// Builds a fresh isolated DOM double and loads the unchanged selector into it.
function fixture() {
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
    other = element("other", {
      x: 40,
      y: 50,
      width: 30,
      height: 25,
      right: 70,
      bottom: 75,
      left: 40,
      top: 50,
    }),
    body = element("body");
  body.querySelectorAll = () => [target, other];
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
  const load = () => vm.runInContext(source, context);
  // Page-dispatched events are untrusted; `trust` may also be omitted to model
  // event test doubles that carry no isTrusted property at all.
  const event = (props, trust) => {
    const e = {
      ...props,
      preventDefault() {
        this.prevented = true;
      },
      stopImmediatePropagation() {
        this.stopped = true;
      },
    };
    if (trust !== undefined) e.isTrusted = trust;
    return e;
  };
  const key = (name, trust, extra = {}) =>
    event({ key: name, ...extra }, trust);
  const fire = (name, e) => listeners.get(name)(e);
  // The highlight box is the first overlay appended by the selector.
  const highlight = () => {
    const { left, top, width, height } = overlays[0]?.style ?? {};
    return JSON.stringify({ left, top, width, height });
  };
  return {
    listeners,
    overlays,
    sent,
    target,
    other,
    body,
    load,
    event,
    key,
    fire,
    highlight,
  };
}

function assertSessionIntact(f, before) {
  assert.deepEqual([...f.listeners.keys()].sort(), [
    "click",
    "keydown",
    "pointermove",
  ]);
  assert.equal(f.overlays.length, 2);
  assert.equal(f.highlight(), before);
}

test("selector cancels without capture, supports keyboard and removes overlay before capture", async () => {
  const f = fixture();
  f.load();
  const escape = f.key("Escape", true);
  f.fire("keydown", escape);
  assert.equal(escape.prevented, true);
  assert.equal(escape.stopped, true);
  assert.equal(f.overlays.length, 0);
  assert.equal(f.listeners.size, 0);
  await settle();
  assert.equal(f.sent.length, 0);
  f.load();
  f.fire("keydown", f.key("Tab", true));
  f.fire("keydown", f.key("Tab", true));
  f.fire("keydown", f.key("Enter", true));
  await settle();
  assert.equal(f.sent.length, 1);
  assert.equal(f.sent[0].metadata.cssSelector, "#target");
  assert.equal(f.sent[0].rect.width, 20);
  assert.equal(f.listeners.size, 0);
  const parent = {
    id: "scroller",
    nodeType: 1,
    localName: "div",
    parentElement: null,
    offsetWidth: 10,
    offsetHeight: 50,
    clientWidth: 10,
    clientHeight: 50,
    clientLeft: 0,
    clientTop: 0,
    getBoundingClientRect: () => ({
      x: 10,
      y: 0,
      left: 10,
      top: 0,
      right: 20,
      bottom: 50,
      width: 10,
      height: 50,
    }),
  };
  f.target.parentElement = parent;
  f.load();
  await f.fire("click", f.event({ target: f.target }, true));
  assert.equal(f.sent[1].rect.x, 10);
  assert.equal(f.sent[1].rect.width, 10);
  assert.equal(f.sent[1].clipped, true);
});

test("selector captures exactly once from trusted pointer selection and click", async () => {
  const f = fixture();
  f.load();
  f.fire("pointermove", f.event({ target: f.other }, true));
  assert.notEqual(f.highlight(), "{}");
  const click = f.event({ target: f.other }, true);
  await f.fire("click", click);
  assert.equal(click.prevented, true);
  assert.equal(click.stopped, true);
  assert.equal(f.overlays.length, 0);
  assert.equal(f.listeners.size, 0);
  await settle();
  assert.equal(f.sent.length, 1);
  assert.equal(f.sent[0].metadata.cssSelector, "#other");
  assert.equal(f.sent[0].rect.width, 30);
});

test("selector supports trusted Tab, Shift+Tab and Enter selection", async () => {
  const f = fixture();
  f.load();
  // candidates: body, target, other; the first Tab selects body.
  for (let i = 0; i < 3; i += 1) f.fire("keydown", f.key("Tab", true));
  assert.equal(f.highlight(), JSON.stringify(styleFor(f.other)));
  const back = f.key("Tab", true, { shiftKey: true });
  f.fire("keydown", back);
  assert.equal(back.prevented, true);
  assert.equal(f.highlight(), JSON.stringify(styleFor(f.target)));
  f.fire("keydown", f.key("Enter", true));
  await settle();
  assert.equal(f.sent.length, 1);
  assert.equal(f.sent[0].metadata.cssSelector, "#target");
  assert.equal(f.overlays.length, 0);
  assert.equal(f.listeners.size, 0);
});

function styleFor(element) {
  const r = element.getBoundingClientRect();
  return {
    left: `${r.x}px`,
    top: `${r.y}px`,
    width: `${r.width}px`,
    height: `${r.height}px`,
  };
}

test("selector ignores untrusted click without side effects and keeps the session", async () => {
  for (const trust of [false, undefined]) {
    const f = fixture();
    f.load();
    f.fire("pointermove", f.event({ target: f.other }, true));
    const before = f.highlight();
    const click = f.event({ target: f.target }, trust);
    await f.fire("click", click);
    await settle();
    assert.equal(click.prevented, undefined);
    assert.equal(click.stopped, undefined);
    assert.equal(f.sent.length, 0);
    assertSessionIntact(f, before);
    // The user's next genuine click still captures exactly once.
    await f.fire("click", f.event({ target: f.target }, true));
    await settle();
    assert.equal(f.sent.length, 1);
    assert.equal(f.sent[0].metadata.cssSelector, "#target");
    assert.equal(f.listeners.size, 0);
  }
});

test("selector ignores untrusted pointermove, Tab, Enter and Escape in a fresh session", async () => {
  for (const trust of [false, undefined]) {
    const f = fixture();
    f.load();
    const before = f.highlight();
    const events = [
      ["pointermove", f.event({ target: f.target }, trust)],
      ["keydown", f.key("Tab", trust)],
      ["keydown", f.key("Tab", trust, { shiftKey: true })],
      ["keydown", f.key("Enter", trust)],
      ["keydown", f.key("Escape", trust)],
      ["click", f.event({ target: f.target }, trust)],
    ];
    for (const [name, e] of events) {
      await f.fire(name, e);
      assert.equal(e.prevented, undefined);
      assert.equal(e.stopped, undefined);
    }
    await settle();
    assert.equal(f.sent.length, 0);
    assertSessionIntact(f, before);
    assert.equal(f.highlight(), "{}");
  }
});

test("selector keeps a trusted selection when an untrusted pointermove or Tab follows", async () => {
  for (const trust of [false, undefined]) {
    const f = fixture();
    f.load();
    f.fire("pointermove", f.event({ target: f.target }, true));
    const before = f.highlight();
    f.fire("pointermove", f.event({ target: f.other }, trust));
    f.fire("keydown", f.key("Tab", trust));
    assert.equal(f.highlight(), before);
    f.fire("keydown", f.key("Enter", true));
    await settle();
    assert.equal(f.sent.length, 1);
    assert.equal(f.sent[0].metadata.cssSelector, "#target");
    assert.equal(f.sent[0].rect.width, 20);
  }
});

test("selector does not capture on untrusted Enter after a trusted selection", async () => {
  const f = fixture();
  f.load();
  f.fire("pointermove", f.event({ target: f.target }, true));
  const before = f.highlight();
  f.fire("keydown", f.key("Enter", false));
  await settle();
  assert.equal(f.sent.length, 0);
  assertSessionIntact(f, before);
});

test("selector trusted Escape cancels after selection with zero messages", async () => {
  const f = fixture();
  f.load();
  f.fire("pointermove", f.event({ target: f.target }, true));
  f.fire("keydown", f.key("Escape", true));
  await settle();
  assert.equal(f.sent.length, 0);
  assert.equal(f.overlays.length, 0);
  assert.equal(f.listeners.size, 0);
});
