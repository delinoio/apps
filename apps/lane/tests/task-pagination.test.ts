// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { create } from "@bufbuild/protobuf";
import { Progress, Proposal, TaskSchema } from "@repo/protos/lane/v1/lane_pb";
import { toUI } from "../src/serializable.ts";
import { loadApp, type AppBundle, type HostNode } from "./harness/app.ts";

const PAGE = 100;
const LOAD_MORE = "Load more tasks";
const FAILURE = "Simulated page failure";

type Call = { name: string; request: any };
interface Held {
  cursor: string;
  request: any;
  release: () => void;
  fail: (error: Error) => void;
}

const text = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(text).join("");
function all(
  nodes: (HostNode | string)[],
  match: (n: HostNode) => boolean,
): HostNode[] {
  const found: HostNode[] = [];
  const walk = (n: HostNode | string) => {
    if (typeof n === "string" || n.props["data-active-tab"] === "false") return;
    if (match(n)) found.push(n);
    n.children.forEach(walk);
  };
  nodes.forEach(walk);
  return found;
}
const isButton = (n: HostNode) =>
  /(^| )button( |$)/.test(n.props.className ?? "");
const isDisabled = (n: HostNode) =>
  /(^| )disabled( |$)/.test(n.props.className) &&
  n.props.focusable === false &&
  n.props["accessibility-value"] === "Disabled";

let bundle: AppBundle;
test.before(async () => {
  bundle = await loadApp();
});

const taskId = (n: number) => `task-${String(n).padStart(4, "0")}`;
const task = (n: number) => ({
  version: {
    entityId: taskId(n),
    versionId: `version-${n}`,
    deleted: false,
    body: {
      case: "task",
      value: toUI(
        create(TaskSchema, {
          title: taskId(n),
          progress: Progress.TODO,
          proposal: Proposal.REGULAR,
        }),
      ),
    },
  },
  alternatives: [],
});

// Mounts the real App against a scripted adapter that serves `total` tasks in
// opaque cursor pages. Every distinct query (repository, search, progress,
// proposal) gets its own ID range, so tests can tell which query's rows are on
// screen: the n-th distinct query starts at ID 1000 * n + 1. Query ListTasks
// calls (the ones carrying `search`) whose cursor is listed in `holds` stay
// pending until the test releases them.
const REPOSITORY = "repo-a";
async function open(total: number, options: { holds?: string[] } = {}) {
  const calls: Call[] = [];
  const held: Held[] = [];
  const holds = new Set(options.holds ?? []);
  const variants: string[] = [];
  const page = (request: any) => {
    const key = JSON.stringify([
      request.repositoryId,
      request.search,
      request.progress,
      request.proposal,
    ]);
    if (!variants.includes(key)) variants.push(key);
    const base = 1000 * variants.indexOf(key);
    const offset = request.page.cursor
      ? Number(request.page.cursor.slice(1))
      : 0;
    const end = Math.min(offset + PAGE, total);
    return {
      items: Array.from({ length: end - offset }, (_, i) =>
        task(base + offset + i + 1),
      ),
      nextCursor: end < total ? `C${end}` : "",
    };
  };
  const adapter = {
    mode: "web",
    spaceId: "space",
    open: async () => {},
    close: () => {},
    subscribe: () => () => {},
    local: async () => ({}),
    call: (method: { name: string }, request: any) => {
      calls.push({ name: method.name, request });
      if (method.name === "ListRepositories")
        return Promise.resolve({
          items: [
            {
              version: {
                entityId: REPOSITORY,
                versionId: "repository-version",
                deleted: false,
                body: { case: "repository", value: { displayName: "Repo A" } },
              },
              alternatives: [],
            },
          ],
          nextCursor: "",
        });
      if (method.name !== "ListTasks" || request.search === undefined)
        return Promise.resolve({ items: [], nextCursor: "" });
      const result = page(request);
      if (!holds.has(request.page.cursor)) return Promise.resolve(result);
      return new Promise((resolve, reject) =>
        held.push({
          cursor: request.page.cursor,
          request,
          release: () => resolve(result),
          fail: reject,
        }),
      );
    },
  };
  const view = bundle.runtime.mount(bundle.App, { providedAdapter: adapter });
  await view.settle();
  const queries = () =>
    calls.filter(
      (c) => c.name === "ListTasks" && c.request.search !== undefined,
    );
  const cursors = () => queries().map((c) => c.request.page.cursor);
  const rows = () =>
    all(view.tree(), (n) => /(^| )task-row( |$)/.test(n.props.className ?? ""));
  // The row title is its task button; entity IDs are the synthetic titles.
  const titles = () =>
    rows().map((row) => text(all(row.children, isButton)[0]));
  const buttons = (label: string) =>
    all(view.tree(), (n) => isButton(n) && text(n) === label);
  const more = () => buttons(LOAD_MORE)[0];
  const button = (label: string) => {
    const [found, ...rest] = buttons(label);
    assert.ok(found, `button ${label} is rendered`);
    assert.equal(rest.length, 0, `button ${label} is unique`);
    return found;
  };
  const notices = () =>
    all(view.tree(), (n) => n.props.className === "notice").map(text);
  const tap = async (node: HostNode) => {
    node.props.bindtap();
    await view.settle();
  };
  const press = async (node: HostNode, key: string) => {
    node.props.bindkeydown({ key });
    await view.settle();
  };
  const settle = () => view.settle();
  // Option button of one Choices group, found through the group's label.
  const choice = (label: string, option: string) => {
    const [field] = all(
      view.tree(),
      (n) =>
        n.props.className === "field" &&
        n.children.some(
          (c) =>
            typeof c !== "string" &&
            c.props.className === "label" &&
            text(c) === label,
        ),
    );
    const [found] = all(
      field.children,
      (n) => isButton(n) && text(n) === option,
    );
    assert.ok(found, `choice ${label}: ${option} is rendered`);
    return found;
  };
  const type = async (label: string, value: string) => {
    const [field] = all(
      view.tree(),
      (n) =>
        n.props.className === "field" &&
        n.children.some(
          (c) =>
            typeof c !== "string" &&
            c.props.className === "label" &&
            text(c) === label,
        ),
    );
    const [input] = all(field.children, (n) => n.type === "input");
    input.props.bindinput({ detail: { value } });
    await view.settle();
  };
  return {
    adapter,
    calls,
    held,
    view,
    queries,
    cursors,
    rows,
    titles,
    more,
    button,
    buttons,
    notices,
    tap,
    press,
    settle,
    type,
    choice,
  };
}
const expectIds = (titles: string[], from: number, to: number) =>
  assert.deepEqual(
    titles,
    Array.from({ length: to - from + 1 }, (_, i) => taskId(from + i)),
  );

test("rapid taps and keyboard activation start one continuation and append its page once", async () => {
  const lane = await open(200, { holds: ["C100"] });
  expectIds(lane.titles(), 1, 100);
  assert.deepEqual(lane.cursors(), [""]);
  const control = lane.more();
  assert.ok(control, "the continuation control is enabled");
  assert.equal(isDisabled(control), false);

  // Two taps, then Enter and Space, all before the continuation settles.
  await lane.tap(control);
  await lane.tap(control);
  await lane.press(control, "Enter");
  await lane.press(control, " ");
  await lane.tap(lane.more());
  assert.deepEqual(lane.cursors(), ["", "C100"], "one continuation request");
  assert.ok(isDisabled(lane.more()), "pending state uses the disabled button");

  lane.held[0].release();
  await lane.settle();
  expectIds(lane.titles(), 1, 200);
  assert.equal(new Set(lane.titles()).size, 200);
  assert.equal(lane.buttons(LOAD_MORE).length, 0, "empty cursor ends paging");
  assert.deepEqual(lane.cursors(), ["", "C100"]);
});

test("keyboard-only activation is also serialized", async () => {
  const lane = await open(200, { holds: ["C100"] });
  const control = lane.more();
  await lane.press(control, "Enter");
  await lane.press(control, "Enter");
  await lane.press(control, " ");
  await lane.press(lane.more(), " ");
  assert.deepEqual(lane.cursors(), ["", "C100"]);
  lane.held[0].release();
  await lane.settle();
  expectIds(lane.titles(), 1, 200);
});

test("a consumed page cannot be requested again or roll the cursor back", async () => {
  const lane = await open(250, { holds: ["C100"] });
  const stale = lane.more();
  for (let i = 0; i < 4; i++) await lane.tap(stale);
  await lane.press(stale, "Enter");
  assert.deepEqual(lane.cursors(), ["", "C100"]);

  lane.held[0].release();
  await lane.settle();
  expectIds(lane.titles(), 1, 200);
  assert.ok(!isDisabled(lane.more()), "the lock is released after success");

  // A control rendered for C100 is still referenced after C100 was consumed.
  // Activating it uses the latest cursor, never the consumed one.
  await lane.tap(stale);
  await lane.tap(stale);
  assert.deepEqual(lane.cursors(), ["", "C100", "C200"]);
  expectIds(lane.titles(), 1, 250);
  assert.equal(new Set(lane.titles()).size, 250);
  assert.equal(lane.buttons(LOAD_MORE).length, 0);

  // Once paging is complete, an old control cannot refetch the first page.
  await lane.tap(stale);
  await lane.press(stale, "Enter");
  assert.deepEqual(lane.cursors(), ["", "C100", "C200"]);
  expectIds(lane.titles(), 1, 250);
});

test("subsequent pages load sequentially with the latest cursor until the cursor is empty", async () => {
  const lane = await open(201);
  expectIds(lane.titles(), 1, 100);
  await lane.tap(lane.more());
  expectIds(lane.titles(), 1, 200);
  assert.ok(lane.more(), "a cursor remains after the second page");
  await lane.tap(lane.more());
  expectIds(lane.titles(), 1, 201);
  assert.equal(new Set(lane.titles()).size, 201);
  assert.deepEqual(lane.cursors(), ["", "C100", "C200"]);
  assert.equal(lane.buttons(LOAD_MORE).length, 0);
});

test("a failed continuation keeps rows, shows the error, never retries itself and can be retried explicitly", async () => {
  const lane = await open(150, { holds: ["C100"] });
  await lane.tap(lane.more());
  await lane.tap(lane.more());
  assert.deepEqual(lane.cursors(), ["", "C100"]);

  lane.held[0].fail(new Error(FAILURE));
  await lane.settle();
  expectIds(lane.titles(), 1, 100);
  assert.ok(
    lane.notices().some((n) => n.includes(FAILURE)),
    "the error is shown",
  );
  assert.deepEqual(lane.cursors(), ["", "C100"], "no automatic retry");
  assert.ok(!isDisabled(lane.more()), "pending state is released");

  // Explicit retry succeeds with IDs 101-150 and the empty cursor.
  await lane.tap(lane.more());
  assert.deepEqual(lane.cursors(), ["", "C100", "C100"]);
  lane.held[1].release();
  await lane.settle();
  expectIds(lane.titles(), 1, 150);
  assert.equal(new Set(lane.titles()).size, 150);
  assert.equal(lane.buttons(LOAD_MORE).length, 0);
  assert.deepEqual(
    lane.notices().filter((n) => n.includes(FAILURE)),
    [],
  );
});

// Each change starts a new first-page generation for a different query.
const changes: [string, (lane: Lane) => Promise<void>][] = [
  ["search", (lane) => lane.type("Search title or description", "other")],
  ["repository", (lane) => lane.tap(lane.button("Repo A"))],
  [
    "progress filter",
    (lane) => lane.tap(lane.choice("Progress filter", "Done")),
  ],
  [
    "proposal filter",
    (lane) => lane.tap(lane.choice("Proposal filter", "Regular")),
  ],
  ["Tasks to Proposals", (lane) => lane.tap(lane.button("Proposals · 0"))],
];
type Lane = Awaited<ReturnType<typeof open>>;

for (const [name, change] of changes) {
  test(`a ${name} change keeps an older continuation from changing the new list, cursor or lock`, async () => {
    const lane = await open(250, { holds: ["C100"] });
    await lane.tap(lane.more());
    assert.equal(lane.held.length, 1);
    const old = lane.held[0];

    // The replacement first page resolves before the old continuation.
    await change(lane);
    assert.equal(lane.cursors().length, 3, "replacement first page requested");
    assert.equal(lane.cursors()[2], "");
    expectIds(lane.titles(), 1001, 1100);

    // A replacement continuation starts before the old work settles. Both are
    // held on C100: the old request keeps its identity, the new one owns the lock.
    await lane.tap(lane.more());
    assert.equal(lane.held.length, 2, "replacement continuation is pending");
    const replacement = lane.held[1];
    assert.notDeepEqual(replacement.request, old.request);
    assert.ok(isDisabled(lane.more()));

    old.release();
    await lane.settle();
    expectIds(lane.titles(), 1001, 1100);
    assert.ok(isDisabled(lane.more()), "old completion keeps the new lock");
    await lane.tap(lane.more());
    await lane.press(lane.more(), "Enter");
    assert.equal(lane.held.length, 2, "the replacement owns the only request");

    replacement.release();
    await lane.settle();
    expectIds(lane.titles(), 1001, 1200);
    assert.equal(new Set(lane.titles()).size, 200);
    assert.ok(!isDisabled(lane.more()));
    await lane.tap(lane.more());
    expectIds(lane.titles(), 1001, 1250);
    assert.equal(lane.buttons(LOAD_MORE).length, 0);
  });

  test(`a ${name} change drops an old continuation that settles after the replacement`, async () => {
    const lane = await open(250, { holds: ["C100"] });
    await lane.tap(lane.more());
    const old = lane.held[0];
    await change(lane);
    old.release();
    await lane.settle();
    expectIds(lane.titles(), 1001, 1100);
    assert.deepEqual(lane.cursors().slice(-1), [""]);
    assert.ok(!isDisabled(lane.more()), "the lock was never taken");
    await lane.tap(lane.more());
    assert.equal(lane.held.length, 2, "the next continuation starts normally");
    assert.equal(lane.held[1].cursor, "C100");
  });
}

test("a failed old continuation does not replace the new list or show an obsolete error", async () => {
  const lane = await open(300, { holds: ["C100"] });
  await lane.tap(lane.more());
  const old = lane.held[0];
  await lane.type("Search title or description", "next");
  old.fail(new Error(FAILURE));
  await lane.settle();
  expectIds(lane.titles(), 1001, 1100);
  assert.deepEqual(
    lane.notices().filter((n) => n.includes(FAILURE)),
    [],
  );
  assert.ok(!isDisabled(lane.more()));
});
