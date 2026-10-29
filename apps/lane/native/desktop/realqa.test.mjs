// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { RealQA } from "./realqa.mjs";
const image = {
  png: Buffer.from("fixture pixels").toString("base64"),
  width: 20,
  height: 10,
};
function setup() {
  const directory = mkdtempSync(join(tmpdir(), "lane-realqa-"));
  const calls = [],
    events = [];
  const saved = [];
  let closed = 0,
    opened = 0;
  const native = {
    capture(raw, callback) {
      const request = JSON.parse(raw);
      calls.push(request);
      if (request.accelerator === "Ctrl+Q")
        callback(
          JSON.stringify({
            ok: false,
            error: {
              reason: "ERROR_REASON_CONFLICT",
              message: "Already registered.",
              retryable: false,
            },
          }),
        );
      else
        callback(
          JSON.stringify({
            ok: true,
            value:
              request.op === "permission"
                ? { permission: "granted" }
                : request.op === "render"
                  ? {
                      ...image,
                      png: Buffer.from(JSON.stringify(request.layers)).toString(
                        "base64",
                      ),
                    }
                  : request.op === "capture"
                    ? image
                    : {},
          }),
        );
    },
  };
  const host = new RealQA({
    native,
    settingsPath: join(directory, "shortcuts.json"),
    emit: (...event) => events.push(event),
    showComposer: () => opened++,
    closeComposer: () => closed++,
    selectSource: (mode) => events.push(["select", mode]),
    onSaved: (taskId, spaceId) => saved.push({ taskId, spaceId }),
  });
  return {
    host,
    saved,
    calls,
    events,
    directory,
    closed: () => closed,
    opened: () => opened,
    cleanup: () => rmSync(directory, { recursive: true, force: true }),
    invoke: async (request) =>
      JSON.parse(await host.invoke(JSON.stringify(request), "composer")),
  };
}
test("draft export preserves identities, hashes final PNG and freezes concurrent captures until save finishes", async () => {
  const t = setup();
  try {
    await t.invoke({ op: "capture", mode: "screen", sourceId: "1" });
    const first = t.host.draft.images[0];
    await t.invoke({
      op: "updateDraft",
      title: "Bug",
      repositoryId: "repo",
      imageId: first.id,
      metadata: { pageUrl: "https://example.test/", cssSelector: ".item" },
    });
    await t.invoke({
      op: "updateDraft",
      imageId: first.id,
      metadata: { pageTitle: "Only retained metadata" },
    });
    const initial = (await t.invoke({ op: "export" })).value;
    assert.match(
      initial.id,
      /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    assert.notEqual(initial.id, initial.requestId);
    assert.notEqual(initial.id, first.id);
    assert.equal(
      initial.images[0].sha256,
      createHash("sha256")
        .update(Buffer.from(image.png, "base64"))
        .digest("hex"),
    );
    assert.deepEqual(initial.images[0].metadata, {
      pageTitle: "Only retained metadata",
    });
    assert.equal((await t.invoke({ op: "capture", mode: "region" })).ok, false);
    assert.throws(() => t.host.appendCapture(image), /save/);
    await t.invoke({ op: "saveFailed" });
    assert.equal(t.host.draft.id, initial.id);
    assert.equal(
      (await t.invoke({ op: "export" })).value.requestId,
      initial.requestId,
    );
    await t.invoke({
      op: "complete",
      taskId: initial.id,
      spaceId: "pinned-space",
    });
    assert.deepEqual(t.saved, [
      { taskId: initial.id, spaceId: "pinned-space" },
    ]);
    assert.equal(t.host.draft, null);
    assert.equal(t.host.originals.size, 0);
    assert.equal(t.closed(), 1);
  } finally {
    t.cleanup();
  }
});
test("edits, undo, redo, image order and discard are memory-only", async () => {
  const t = setup();
  try {
    t.host.appendCapture(image);
    t.host.appendCapture(image);
    const [a, b] = t.host.draft.images;
    const layers = [
      { type: "cover", x: 1, y: 2, width: 3, height: 4, color: "#000000" },
    ];
    assert.equal(
      (await t.invoke({ op: "edit", imageId: a.id, layers })).ok,
      true,
    );
    assert.equal(a.canUndo, true);
    await t.invoke({ op: "undo", imageId: a.id });
    assert.deepEqual(a.layers, []);
    assert.equal(a.canRedo, true);
    await t.invoke({ op: "redo", imageId: a.id });
    assert.deepEqual(a.layers, layers);
    await t.invoke({ op: "reorderImages", ids: [b.id, a.id] });
    assert.deepEqual(
      t.host.draft.images.map((i) => i.id),
      [b.id, a.id],
    );
    assert.equal(
      (await t.invoke({ op: "reorderImages", ids: [b.id, b.id] })).ok,
      false,
    );
    await t.invoke({ op: "removeImage", imageId: b.id });
    assert.equal(t.host.originals.has(b.id), false);
    await t.invoke({ op: "close" });
    assert.equal(t.closed(), 0);
    assert.equal(t.events.at(-1)[0], "lane.realqa.closeRequested");
    await t.invoke({ op: "discard" });
    assert.equal(t.host.originals.size, 0);
    assert.equal(t.host.draft, null);
  } finally {
    t.cleanup();
  }
});
test("the shared host owns retry identities across edits and composer lifetimes", async () => {
  const t = setup();
  try {
    t.host.appendCapture(image);
    await t.invoke({ op: "updateDraft", title: "First", repositoryId: "repo" });
    const first = (await t.invoke({ op: "export" })).value;
    await t.invoke({ op: "saveFailed" });
    await t.invoke({ op: "updateDraft", title: "Revised" });
    const revised = (await t.invoke({ op: "export" })).value;
    assert.equal(revised.id, first.id, "one draft keeps one task identity");
    assert.notEqual(
      revised.requestId,
      first.requestId,
      "changed input needs a new receipt",
    );
    await t.invoke({ op: "saveFailed" });
    // Another view has no knowledge of the previous composer's React refs.
    const retry = JSON.parse(
      await t.host.invoke(JSON.stringify({ op: "export" }), "main"),
    ).value;
    assert.deepEqual(
      retry,
      revised,
      "a replacement view replays the exact exported request",
    );
    await t.invoke({ op: "saveFailed" });
    await t.invoke({ op: "updateDraft", title: "Transient edit" });
    await t.invoke({ op: "updateDraft", title: "Revised" });
    assert.deepEqual(
      (await t.invoke({ op: "export" })).value,
      revised,
      "edits reverted before export preserve the retry identity",
    );
    await t.invoke({ op: "saveFailed" });
    await t.invoke({
      op: "updateDraft",
      imageId: revised.images[0].id,
      metadata: { pageTitle: "Retained title" },
    });
    const metadata = (await t.invoke({ op: "export" })).value;
    assert.notEqual(
      metadata.requestId,
      revised.requestId,
      "retained metadata is part of the request",
    );
    await t.invoke({ op: "saveFailed" });
    await t.invoke({ op: "discard" });
    t.host.appendCapture(image);
    await t.invoke({ op: "updateDraft", title: "First", repositoryId: "repo" });
    const next = (await t.invoke({ op: "export" })).value;
    assert.notEqual(next.id, first.id);
    assert.notEqual(next.requestId, first.requestId);
  } finally {
    t.cleanup();
  }
});
test("shortcut conflict retains prior binding, persists only device settings and restores them", async () => {
  const t = setup();
  try {
    assert.deepEqual(t.host.shortcuts, {
      screen: "",
      window: "",
      region: "",
      dom: "",
    });
    assert.equal(
      (
        await t.invoke({
          op: "shortcut",
          mode: "screen",
          accelerator: "Ctrl+Shift+S",
        })
      ).ok,
      true,
    );
    assert.equal(
      (
        await t.invoke({
          op: "shortcut",
          mode: "screen",
          accelerator: "Ctrl+Q",
        })
      ).ok,
      false,
    );
    assert.equal(t.host.shortcuts.screen, "Ctrl+Shift+S");
    assert.equal(
      JSON.parse(readFileSync(join(t.directory, "shortcuts.json"))).screen,
      "Ctrl+Shift+S",
    );
    t.host.shortcuts.screen = "";
    await t.host.start();
    assert.equal(t.host.shortcuts.screen, "Ctrl+Shift+S");
    await t.invoke({ op: "shortcut", mode: "screen", accelerator: "" });
    assert.equal(t.host.shortcuts.screen, "");
  } finally {
    t.cleanup();
  }
});
test("cancelled and failed captures preserve existing draft, source choice does not capture silently", async () => {
  const t = setup();
  try {
    t.host.appendCapture(image);
    const id = t.host.draft.id;
    assert.equal(
      (await t.invoke({ op: "capture", mode: "screen" })).value.selecting,
      true,
    );
    t.host.native.capture = (_raw, cb) =>
      cb(JSON.stringify({ ok: true, value: { cancelled: true } }));
    assert.equal(
      (await t.invoke({ op: "capture", mode: "region" })).value.cancelled,
      true,
    );
    assert.equal(t.host.draft.images.length, 1);
    assert.equal(t.host.draft.id, id);
    t.host.native.capture = (_raw, cb) =>
      cb(
        JSON.stringify({
          ok: false,
          error: {
            reason: "ERROR_REASON_PERMISSION_DENIED",
            message: "Allow capture.",
            retryable: true,
          },
        }),
      );
    assert.equal(
      (await t.invoke({ op: "capture", mode: "region" })).error.reason,
      "ERROR_REASON_PERMISSION_DENIED",
    );
    assert.equal(t.host.draft.id, id);
    assert.equal(t.host.capturePending, false);
  } finally {
    t.cleanup();
  }
});
