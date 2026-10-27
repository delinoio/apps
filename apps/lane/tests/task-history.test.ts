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
  prompt?: string,
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
type Options = {
  restore?: (request: any) => Promise<any>;
  draftPatch?: Partial<ReturnType<typeof draftFor>>;
};
// Mounts the real Task editor on `head` and answers ListHistory from `pages`,
// keyed by the request cursor ("" is the first page).
async function open(
  head: Version,
  pages: Record<string, Page | Promise<Page>>,
  options: Options = {},
) {
  const requests: any[] = [];
  const restores: any[] = [];
  const saved: any[] = [];
  const adapter = {
    mode: "web",
    call: async (method: { name: string }, request: any) => {
      if (method.name === "ListTasks") return { items: [], nextCursor: "" };
      if (method.name === "RestorePrompt") {
        restores.push(request);
        if (!options.restore) throw new Error("Unexpected restore");
        return options.restore(request);
      }
      if (method.name !== "ListHistory")
        throw new Error(`Unexpected call ${method.name}`);
      requests.push(request);
      const page = await pages[request.page.cursor];
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
    draft: { ...draftFor(result as never), ...options.draftPatch },
    setDraft: () => {},
    current: result,
    repositories: [],
    planning: { items: [], status: "ready", error: "", retry: () => {} },
    revision: 0,
    openTask: () => {},
    beginCompletion: () => ({
      saved: (value: any) => saved.push(value),
      close: () => {},
    }),
    onClose: () => {},
  });
  await view.settle();
  const press = (label: string, key?: string) => {
    const found = all(view.tree(), isButton).filter((b) => text(b) === label);
    assert.equal(found.length, 1, `button ${label} is rendered once`);
    if (key) found[0].props.bindkeydown({ key });
    else found[0].props.bindtap();
  };
  const tap = async (label: string) => {
    press(label);
    await view.settle();
  };
  // One entry per history card, in rendered order.
  const cards = () =>
    all(view.tree(), (n) => n.props.className === "card history-card").map(
      (card) => {
        const metadata = all(
          [card],
          (n) =>
            n.type === "text" &&
            /^(history-time|history-origin)$/.test(n.props.className),
        );
        return {
          time: text(
            metadata.find((n) => n.props.className === "history-time") ?? "",
          ),
          origin: text(
            metadata.find((n) => n.props.className === "history-origin")!,
          ),
          rows: all([card], (n) => n.props.className === "history-change").map(
            (row) => {
              const values = all([row], (n) => !!n.props["text-selection"]);
              return [
                text(
                  all([row], (n) => n.props.className === "history-field")[0],
                ),
                `Before: ${text(values[0])}`,
                `After: ${text(values[1])}`,
              ];
            },
          ),
        };
      },
    );
  return { view, tap, press, cards, requests, restores, saved };
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
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const hasText = (view: { tree: () => HostNode[] }, value: string) =>
  all(view.tree(), (n) => n.type === "text").some((n) => text(n) === value);
const restoreReply = (version: Version) => ({
  result: toUI(
    create(EntityResultSchema, {
      result: ResultKind.APPLIED,
      version: fromUIVersion(version),
    }),
  ),
});

test("a sibling branch never becomes the before state of the displayed version", async () => {
  const { tap, cards } = await open(C, {
    "": { versions: [A, B, C], nextCursor: "" },
  });
  await tap("History");
  const shown = cards();
  assert.equal(shown.length, 3);
  // Creation keeps its existing absent-before rows and formatting.
  assert.deepEqual(shown[0].rows, [
    ["Title", "Before: (absent)", "After: original"],
    ["Description", "Before: (absent)", "After: "],
    ["Progress", "Before: (absent)", "After: To do"],
    ["Proposal", "Before: (absent)", "After: Regular"],
    ["Repository", "Before: (absent)", "After: repo"],
  ]);
  // B: only its own title change against A.
  assert.deepEqual(shown[1].rows, [
    ["Title", "Before: original", "After: changedByB"],
  ]);
  // C: only description against its declared parent A, not B's title.
  assert.deepEqual(shown[2].rows, [
    ["Description", "Before: ", "After: changedByC"],
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
    "Title,Description,Progress,Proposal,Repository",
  );
  assert.deepEqual(shown[1].rows, [
    ["Title", "Before: original", "After: changedByB"],
  ]);
  assert.deepEqual(shown[2].rows, [
    ["Description", "Before: ", "After: described"],
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
    ["Title", "Before: original", "After: changedByB"],
  ]);
  assert.deepEqual(shown[1].rows, [
    ["Description", "Before: ", "After: changedByC"],
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
  const { tap, cards, view } = await open(R, {
    "": { versions: [A, B, C, R], nextCursor: "" },
  });
  await tap("History");
  const shown = cards();
  assert.equal(shown.length, 4);
  assert.deepEqual(shown[3].rows, []);
  assert.equal(shown[3].time, stamp(R));
  assert.match(shown[3].origin, /editor-4/);
  assert.ok(
    text(view.tree()[0]).includes(
      "This resolution has multiple parent versions.",
    ),
  );
  // The other cards are unchanged by the resolution.
  assert.deepEqual(shown[2].rows, [
    ["Description", "Before: ", "After: changedByC"],
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

test("missing origin fields do not fabricate a date, device, or account", async () => {
  const missing = { ...A, createdAt: undefined, origin: undefined };
  const { tap, cards } = await open(A, {
    "": { versions: [missing], nextCursor: "" },
  });
  await tap("History");
  assert.equal(cards()[0].time, "");
  assert.equal(cards()[0].origin, "Unknown · Local account");
});

test("history opens before its read, retries an initial failure, and shows mode-specific empty states", async () => {
  const first = deferred<Page>();
  const { view, press, tap, requests } = await open(A, {
    "": first.promise,
  });
  press("History");
  await view.settle();
  assert.ok(hasText(view, "Loading history…"));
  assert.equal(requests[0].promptsOnly, false);
  assert.equal(requests[0].page.size, 100);
  assert.ok(all(view.tree(), isButton).some((n) => text(n) === "Close"));
  first.reject(new Error("read unavailable"));
  await view.settle();
  assert.ok(text(view.tree()[0]).includes("read unavailable"));
  assert.ok(!hasText(view, "No task history available."));
  await tap("Retry loading history");
  assert.equal(requests.length, 2);
  await tap("Close");
  const empty = await open(A, {
    "": { versions: [], nextCursor: "" },
  });
  await empty.tap("Prompt versions");
  assert.equal(empty.requests[0].promptsOnly, true);
  assert.ok(hasText(empty.view, "No prompt versions available."));
  assert.ok(
    !all(empty.view.tree(), isButton).some((n) => text(n).includes("Restore")),
  );
});

test("a rapid second launcher and a closed read cannot replace the next dialog", async () => {
  const first = deferred<Page>();
  const second = deferred<Page>();
  let calls = 0;
  const { view, press, tap, requests, cards } = await open(A, {
    get ""() {
      return calls++ === 0 ? first.promise : second.promise;
    },
  });
  press("History");
  press("Prompt versions");
  await view.settle();
  assert.equal(requests.length, 1);
  assert.equal(requests[0].promptsOnly, false);
  await tap("Close");
  // The old read is still pending when the second mode acquires ownership.
  press("Prompt versions");
  await view.settle();
  assert.equal(requests.length, 2);
  first.resolve({ versions: [A], nextCursor: "" });
  await view.settle();
  assert.equal(cards().length, 0);
  assert.ok(hasText(view, "Loading history…"));
  second.resolve({ versions: [A], nextCursor: "" });
  await view.settle();
  assert.equal(cards().length, 1);
  assert.ok(text(view.tree()[0]).includes("Prompt versions"));
  assert.equal(requests[1].promptsOnly, true);
});

test("a held continuation accepts one page, retains cards, and retries a failed cursor", async () => {
  const continuation = deferred<Page>();
  const versions = Array.from({ length: 100 }, (_, i) =>
    make(i + 10, [], `task-${i}`, ""),
  );
  const { view, press, cards, requests } = await open(A, {
    "": { versions, nextCursor: "C2" },
    C2: continuation.promise,
  });
  press("History");
  await view.settle();
  assert.equal(cards().length, 100);
  press("Load more history");
  press("Load more history", "Enter");
  await view.settle();
  assert.equal(requests.length, 2);
  assert.equal(requests[1].page.cursor, "C2");
  assert.ok(hasText(view, "Loading more history…"));
  assert.equal(cards().length, 100);
  continuation.resolve({ versions: [A], nextCursor: "" });
  await view.settle();
  assert.equal(cards().length, 101);
  assert.ok(
    !all(view.tree(), isButton).some((n) => text(n) === "Load more history"),
  );

  const failure = deferred<Page>();
  const retry = await open(A, {
    "": { versions: [B], nextCursor: "C2" },
    C2: failure.promise,
  });
  await retry.tap("History");
  retry.press("Load more history");
  failure.reject(new Error("page unavailable"));
  await retry.view.settle();
  assert.equal(retry.cards().length, 1);
  assert.ok(text(retry.view.tree()[0]).includes("page unavailable"));
  await retry.tap("Retry loading history");
  assert.deepEqual(
    retry.requests.map((r) => r.page.cursor),
    ["", "C2", "C2"],
  );
  assert.equal(retry.cards().length, 1);
});

test("prompt versions distinguish absent, empty, and safe Markdown content", async () => {
  const absent = make(6, [], "absent", "");
  const empty = make(7, [], "empty", "", "");
  const markdown = make(
    8,
    [],
    "markdown",
    "",
    "## Check\n\n<script>literal</script>",
  );
  const { view, tap, cards } = await open(markdown, {
    "": { versions: [absent, empty, markdown], nextCursor: "" },
  });
  await tap("Prompt versions");
  assert.equal(cards().length, 3);
  assert.ok(hasText(view, "(No prompt)"));
  assert.ok(hasText(view, "Empty prompt"));
  assert.ok(text(view.tree()[0]).includes("<script>literal</script>"));
  assert.equal(all(view.tree(), (n) => n.type === "script").length, 0);
});

test("restore blocks repeated activation and replays the same request after reply loss", async () => {
  const pending = deferred<any>();
  let replyLost = true;
  const { view, press, tap, restores, saved, cards } = await open(
    A,
    {
      "": { versions: [A], nextCursor: "" },
    },
    {
      restore: async () => {
        if (replyLost) {
          replyLost = false;
          await pending.promise;
          throw new Error("reply lost");
        }
        return restoreReply(A);
      },
    },
  );
  await tap("Prompt versions");
  press("Restore this prompt as a new version");
  press("Restore this prompt as a new version", "Enter");
  await view.settle();
  assert.equal(restores.length, 1);
  assert.ok(hasText(view, "Restoring…"));
  assert.equal(cards().length, 1);
  pending.resolve(undefined);
  await view.settle();
  assert.ok(text(view.tree()[0]).includes("reply lost"));
  assert.equal(saved.length, 0);
  await tap("Retry restoring this prompt");
  assert.equal(restores.length, 2);
  assert.deepEqual(restores[0], restores[1]);
  assert.equal(restores[0].expectedVersionId, "v1");
  assert.equal(restores[0].sourceVersionId, "v1");
  assert.ok(restores[0].meta.requestId);
  assert.equal(saved.length, 1);
  assert.equal(cards().length, 0);
});

test("dirty and deleted drafts keep prompt restoration disabled", async () => {
  for (const patch of [{ dirty: true }, { deleted: true }]) {
    const { view, tap, restores } = await open(
      A,
      {
        "": { versions: [A], nextCursor: "" },
      },
      { draftPatch: patch },
    );
    await tap("Prompt versions");
    const restore = all(view.tree(), isButton).find((n) =>
      text(n).includes("Restore this prompt"),
    )!;
    assert.match(restore.props.className, /disabled/);
    restore.props.bindtap();
    await view.settle();
    assert.equal(restores.length, 0);
    assert.ok(
      text(view.tree()[0]).includes(
        patch.dirty
          ? "Save your changes before restoring a prompt."
          : "This task was deleted.",
      ),
    );
  }
});

test("permission failures stay visible without an ineligible retry", async () => {
  const denied = new bundle.LaneError({
    reason: "PERMISSION_DENIED",
    message: "Permission denied. Reconnect this account.",
    retryable: false,
  });
  const read = await open(A, { "": denied });
  await read.tap("History");
  assert.ok(text(read.view.tree()[0]).includes("Permission denied"));
  assert.ok(
    !all(read.view.tree(), isButton).some(
      (n) => text(n) === "Retry loading history",
    ),
  );

  const mutation = await open(
    A,
    {
      "": { versions: [A], nextCursor: "" },
    },
    {
      restore: async () => {
        throw denied;
      },
    },
  );
  await mutation.tap("Prompt versions");
  await mutation.tap("Restore this prompt as a new version");
  const retry = all(mutation.view.tree(), isButton).find(
    (n) => text(n) === "Retry restoring this prompt",
  )!;
  assert.match(retry.props.className, /disabled/);
  assert.ok(text(mutation.view.tree()[0]).includes("Permission denied"));
});
