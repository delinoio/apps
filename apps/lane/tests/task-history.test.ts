// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { create } from "@bufbuild/protobuf";
import {
  EntityKind,
  EntityResultSchema,
  EntityVersionSchema,
  OriginKind,
  Progress,
  Proposal,
  ResultKind,
} from "@repo/protos/lane/v1/lane_pb";
import { draftFor } from "../src/model.ts";
import { toUI } from "../src/serializable.ts";
import {
  loadTaskEditor,
  type HostNode,
  type TaskEditorBundle,
} from "./harness/task-editor.ts";

const EPOCH = Date.parse("2026-10-26T00:00:00Z") / 1000;
const TASK = "task-1";
const text = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(text).join("");
function all(
  nodes: (HostNode | string)[],
  match: (n: HostNode) => boolean,
): HostNode[] {
  const found: HostNode[] = [];
  const walk = (n: HostNode | string) => {
    if (typeof n === "string") return;
    if (match(n)) found.push(n);
    n.children.forEach(walk);
  };
  nodes.forEach(walk);
  return found;
}
const isButton = (n: HostNode) =>
  /(^| )button( |$)/.test(n.props.className ?? "");

// Synthetic snapshots are generated protobuf messages converted by the real UI
// serializer, as the adapter delivers them to the editor.
const make = (
  n: number,
  parents: string[],
  title: string,
  description: string,
  prompt = "",
) =>
  toUI(
    create(EntityVersionSchema, {
      formatVersion: 2,
      entityId: TASK,
      kind: EntityKind.TASK,
      versionId: `v${n}`,
      parentVersionIds: parents,
      createdAt: { seconds: BigInt(EPOCH + n * 60), nanos: 0 },
      origin: {
        kind: OriginKind.APP,
        deviceId: "device",
        deviceName: `editor-${n}`,
        accountId: `account-${n}`,
      },
      body: {
        case: "task",
        value: {
          repositoryId: "repo",
          title,
          description,
          prompt,
          progress: Progress.TODO,
          proposal: Proposal.REGULAR,
        },
      },
    }),
  );
type Version = ReturnType<typeof make>;
const A = make(1, [], "original", "");
const B = make(2, ["v1"], "changedByB", "");
const C = make(3, ["v1"], "original", "changedByC");
const stamp = (v: Version) =>
  new Date(Number(v.createdAt!.seconds) * 1000).toLocaleString();

let bundle: TaskEditorBundle;
test.before(async () => {
  bundle = await loadTaskEditor();
});

type Page = { versions: Version[]; nextCursor: string } | Error;
// Mounts the real Task editor on `head` and answers ListHistory from `pages`,
// keyed by the request cursor ("" is the first page).
async function open(head: Version, pages: Record<string, Page>) {
  const requests: any[] = [];
  const adapter = {
    mode: "web",
    call: async (method: { name: string }, request: any) => {
      if (method.name === "ListTasks") return { items: [], nextCursor: "" };
      if (method.name !== "ListHistory")
        throw new Error(`Unexpected call ${method.name}`);
      requests.push(request);
      const page = pages[request.page.cursor];
      if (!page) throw new Error(`Unexpected cursor ${request.page.cursor}`);
      if (page instanceof Error) throw page;
      return page;
    },
    local: () => {
      throw new Error("Unexpected local call");
    },
    subscribe: () => () => {},
  };
  const result = toUI(
    create(EntityResultSchema, {
      result: ResultKind.APPLIED,
      version: fromUIVersion(head),
    }),
  );
  const view = bundle.runtime.mount(bundle.TaskEditor, {
    adapter,
    draft: draftFor(result as never),
    setDraft: () => {},
    current: result,
    repositories: [],
    planning: { items: [], status: "ready", error: "", retry: () => {} },
    revision: 0,
    openTask: () => {},
    beginCompletion: () => ({ saved: () => {}, close: () => {} }),
    onClose: () => {},
  });
  await view.settle();
  const tap = async (label: string) => {
    const found = all(view.tree(), isButton).filter((b) => text(b) === label);
    assert.equal(found.length, 1, `button ${label} is rendered once`);
    found[0].props.bindtap();
    await view.settle();
  };
  // One entry per history card, in rendered order.
  const cards = () =>
    all(view.tree(), (n) => n.props.className === "card").map((card) => {
      const [time, origin] = all(
        [card],
        (n) => n.type === "text" && !n.props["text-selection"],
      ).filter((n) => /^(label|muted)$/.test(n.props.className));
      return {
        time: text(time),
        origin: text(origin),
        rows: all([card], (n) =>
          n.children.some(
            (c) => typeof c !== "string" && text(c).startsWith("Before:"),
          ),
        ).map((row) => row.children.map(text)),
      };
    });
  return { view, tap, cards, requests };
}
// The head version is only used to give the editor a base; rebuild a message.
function fromUIVersion(v: Version) {
  return create(EntityVersionSchema, {
    entityId: v.entityId,
    versionId: v.versionId,
    kind: EntityKind.TASK,
    body: { case: "task", value: v.body.value as never },
  });
}
const fields = (card: { rows: string[][] }) => card.rows.map((r) => r[0]);

test("a sibling branch never becomes the before state of the displayed version", async () => {
  const { tap, cards } = await open(C, {
    "": { versions: [A, B, C], nextCursor: "" },
  });
  await tap("History");
  const shown = cards();
  assert.equal(shown.length, 3);
  // Creation keeps its existing absent-before rows and formatting.
  assert.deepEqual(shown[0].rows, [
    ["title", "Before: (absent)", "After: original"],
    ["description", "Before: (absent)", "After: "],
    ["progress", "Before: undefined", "After: To do"],
    ["proposal", "Before: undefined", "After: Regular"],
    ["repositoryId", "Before: (absent)", "After: repo"],
  ]);
  // B: only its own title change against A.
  assert.deepEqual(shown[1].rows, [
    ["title", "Before: original", "After: changedByB"],
  ]);
  // C: only description against its declared parent A, not B's title.
  assert.deepEqual(shown[2].rows, [
    ["description", "Before: ", "After: changedByC"],
  ]);
  for (const [i, v] of [A, B, C].entries()) {
    assert.equal(shown[i].time, stamp(v));
    assert.match(shown[i].origin, new RegExp(`editor-${i + 1}`));
    assert.match(shown[i].origin, new RegExp(`account-${i + 1}`));
  }
});

test("linear history compares each version with its declared parent", async () => {
  const D = make(4, ["v2"], "changedByB", "described");
  const { tap, cards } = await open(D, {
    "": { versions: [A, B, D], nextCursor: "" },
  });
  await tap("History");
  const shown = cards();
  assert.equal(
    fields(shown[0]).join(),
    "title,description,progress,proposal,repositoryId",
  );
  assert.deepEqual(shown[1].rows, [
    ["title", "Before: original", "After: changedByB"],
  ]);
  assert.deepEqual(shown[2].rows, [
    ["description", "Before: ", "After: described"],
  ]);
});

test("a missing parent keeps the card, defers the delta and resolves after load more", async () => {
  const { tap, cards, requests } = await open(C, {
    "": { versions: [B, C], nextCursor: "page-2" },
    "page-2": { versions: [A], nextCursor: "" },
  });
  await tap("History");
  let shown = cards();
  assert.equal(shown.length, 2);
  // Neither a creation delta nor the neighbouring branch's title delta.
  assert.deepEqual(shown[0].rows, []);
  assert.deepEqual(shown[1].rows, []);
  assert.equal(shown[1].time, stamp(C));
  assert.match(shown[1].origin, /editor-3/);
  await tap("Load more history");
  assert.equal(requests[1].page.cursor, "page-2");
  shown = cards();
  // Response order is preserved; ancestry is resolved from accumulated pages.
  assert.deepEqual(
    shown.map((c) => c.time),
    [stamp(B), stamp(C), stamp(A)],
  );
  assert.deepEqual(shown[0].rows, [
    ["title", "Before: original", "After: changedByB"],
  ]);
  assert.deepEqual(shown[1].rows, [
    ["description", "Before: ", "After: changedByC"],
  ]);
  assert.equal(shown[2].rows.length, 5);
});

test("a failed load more keeps loaded cards and invents no baseline", async () => {
  const { tap, cards, view } = await open(C, {
    "": { versions: [B, C], nextCursor: "page-2" },
    "page-2": new Error("network down"),
  });
  await tap("History");
  const before = cards();
  await tap("Load more history");
  assert.deepEqual(cards(), before);
  assert.deepEqual(
    cards().map((c) => c.rows),
    [[], []],
  );
  assert.ok(
    all(view.tree(), (n) => n.props.className === "notice").some((n) =>
      text(n).includes("network down"),
    ),
  );
});

test("a resolution with several parents keeps its metadata and shows no single-baseline delta", async () => {
  const R = make(4, ["v2", "v3"], "resolved", "changedByC");
  const { tap, cards } = await open(R, {
    "": { versions: [A, B, C, R], nextCursor: "" },
  });
  await tap("History");
  const shown = cards();
  assert.equal(shown.length, 4);
  assert.deepEqual(shown[3].rows, []);
  assert.equal(shown[3].time, stamp(R));
  assert.match(shown[3].origin, /editor-4/);
  // The other cards are unchanged by the resolution.
  assert.deepEqual(shown[2].rows, [
    ["description", "Before: ", "After: changedByC"],
  ]);
});

test("Prompt versions keep their separate presentation", async () => {
  const P = make(5, ["v1"], "original", "", "prompt text");
  const { tap, cards } = await open(P, {
    "": { versions: [P], nextCursor: "" },
  });
  await tap("Prompt versions");
  const shown = cards();
  assert.equal(shown.length, 1);
  assert.deepEqual(shown[0].rows, []);
});
