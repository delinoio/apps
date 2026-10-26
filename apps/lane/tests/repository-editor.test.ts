// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { test } from "node:test";
import { LaneService, ResultKind } from "@repo/protos/lane/v1/lane_pb";
import {
  RepositoryEditor,
  RepositorySaveOutcome,
  type RepositoryDraft,
} from "../src/repositoryEditor.ts";

const SAVE = LaneService.method.saveRepository;
const CREATE = LaneService.method.createRepository;

interface Call {
  method: string;
  input: {
    meta: { requestId: string };
    id: string;
    url: string;
    displayName?: string;
    expectedVersionId?: string;
  };
}

// A deferred adapter: each call waits until the test settles it.
function deferredAdapter() {
  const calls: Call[] = [];
  const waiting: {
    resolve: (reply: unknown) => void;
    reject: (error: Error) => void;
  }[] = [];
  const adapter = {
    call: (method: { name: string }, input: Call["input"]) => {
      calls.push({ method: method.name, input: structuredClone(input) });
      return new Promise((resolve, reject) =>
        waiting.push({ resolve, reject }),
      );
    },
  } as unknown as Parameters<RepositoryEditor["save"]>[0];
  return {
    adapter,
    calls,
    applied: (index = calls.length - 1) =>
      waiting[index].resolve({ result: { result: ResultKind.APPLIED } }),
    conflict: (index = calls.length - 1) =>
      waiting[index].resolve({ result: { result: ResultKind.CONFLICT } }),
    reject: (index = calls.length - 1) =>
      waiting[index].reject(new Error("offline")),
  };
}

function editor() {
  let sequence = 0;
  let changes = 0;
  const value = new RepositoryEditor(
    () => changes++,
    () => `request-${++sequence}`,
  );
  return { editor: value, changes: () => changes };
}

const existing = (name = "A1"): RepositoryDraft => ({
  id: "repo-a",
  base: "V0",
  url: "https://example.test/team/a",
  name,
});
const created = (): RepositoryDraft => ({
  id: "repo-new",
  url: "https://example.test/team/new",
  name: "New",
});

// A bounded model of the server rules the client depends on: receipts keyed by method and
// request ID replay the original response, a changed fingerprint is rejected, create rejects
// a used entity ID, and a save from a stale parent creates a second head.
class ServerModel {
  versions = new Set<string>();
  heads: string[] = [];
  receipts = new Map<string, { fingerprint: string; result: ResultKind }>();
  sequence = 0;
  constructor(seed?: string) {
    if (seed) {
      this.versions.add(seed);
      this.heads = [seed];
    }
  }
  commit(call: Call) {
    const key = `${call.method}:${call.input.meta.requestId}`;
    const fingerprint = JSON.stringify(call.input);
    const prior = this.receipts.get(key);
    if (prior) {
      assert.equal(prior.fingerprint, fingerprint, "REQUEST_ID_REUSED");
      return { replayed: true, result: prior.result };
    }
    if (call.method === CREATE.name) {
      if (this.versions.size) throw new Error("entity ID is already in use");
      const version = `V${++this.sequence}`;
      this.versions.add(version);
      this.heads = [version];
      this.receipts.set(key, { fingerprint, result: ResultKind.APPLIED });
      return { replayed: false, result: ResultKind.APPLIED };
    }
    assert(this.versions.has(call.input.expectedVersionId!));
    const version = `V${++this.sequence}`;
    this.versions.add(version);
    const parentIsHead = this.heads.includes(call.input.expectedVersionId!);
    this.heads = parentIsHead
      ? [
          ...this.heads.filter((h) => h !== call.input.expectedVersionId),
          version,
        ]
      : [...this.heads, version];
    const result =
      this.heads.length > 1 ? ResultKind.CONFLICT : ResultKind.APPLIED;
    this.receipts.set(key, { fingerprint, result });
    return { replayed: false, result };
  }
}

// Commits through the model but optionally drops the response, as a lost reply would.
function modelAdapter(server: ServerModel, drop: { next: boolean }) {
  const calls: Call[] = [];
  const adapter = {
    call: async (method: { name: string }, input: Call["input"]) => {
      const call = { method: method.name, input: structuredClone(input) };
      calls.push(call);
      const outcome = server.commit(call);
      if (drop.next) {
        drop.next = false;
        throw new Error("response lost after commit");
      }
      return { result: { result: outcome.result } };
    },
  } as unknown as Parameters<RepositoryEditor["save"]>[0];
  return { adapter, calls };
}

test("a pending save locks URL and name and ignores a duplicate submit", async () => {
  for (const draft of [existing(), created()]) {
    const { editor: e } = editor();
    const fake = deferredAdapter();
    e.open(draft);
    const first = e.save(fake.adapter);
    assert.equal(e.saving, true);
    e.edit({ name: "A2" });
    e.edit({ url: "https://example.test/other" });
    assert.equal(e.draft?.name, draft.name);
    assert.equal(e.draft?.url, draft.url);
    assert.equal(
      (await e.save(fake.adapter)).outcome,
      RepositorySaveOutcome.Ignored,
    );
    assert.equal(fake.calls.length, 1);
    assert.equal(fake.calls[0].method, draft.base ? SAVE.name : CREATE.name);
    assert.equal(fake.calls[0].input.displayName, draft.name);
    assert.equal(fake.calls[0].input.url, draft.url);
    assert.equal(fake.calls[0].input.expectedVersionId, draft.base);
    fake.applied();
    const result = await first;
    assert.deepEqual(
      [result.outcome, result.current],
      [RepositorySaveOutcome.Saved, true],
    );
    assert.equal(e.draft, undefined);
    assert.equal(e.saving, false);
  }
});

test("a response from a closed editor never closes or changes a newer editor", async () => {
  // Plain close and the dialog's Escape path both call close().
  for (const reopen of ["other", "same-id"] as const) {
    const { editor: e } = editor();
    const fake = deferredAdapter();
    e.open(existing());
    const first = e.save(fake.adapter);
    e.close();
    assert.equal(e.draft, undefined);
    e.open(
      reopen === "other"
        ? { id: "repo-b", base: "B0", url: "https://example.test/b", name: "B" }
        : existing("A-reopened"),
    );
    e.edit({ name: "typed after reopening" });
    const before = structuredClone(e.draft);
    assert.equal(e.saving, false);
    fake.applied();
    const result = await first;
    assert.equal(result.outcome, RepositorySaveOutcome.Saved);
    assert.equal(result.current, false);
    // An ID-only comparison would have closed the reopened editor for the same repository.
    assert.deepEqual(e.draft, before);
    assert.equal(e.saving, false);
  }
});

test("a late response after close leaves the editor closed", async () => {
  const { editor: e } = editor();
  const fake = deferredAdapter();
  e.open(existing());
  const first = e.save(fake.adapter);
  e.close();
  fake.applied();
  const result = await first;
  assert.deepEqual(
    [result.outcome, result.current],
    [RepositorySaveOutcome.Saved, false],
  );
  assert.equal(e.draft, undefined);
  // A late failure of the closed editor is reported as not current, so no error is shown.
  const second = deferredAdapter();
  e.open(existing());
  const pending = e.save(second.adapter);
  e.close();
  second.reject();
  assert.equal((await pending).current, false);
  assert.equal(e.draft, undefined);
});

test("a stale failure does not unlock or report into a newer saving editor", async () => {
  const { editor: e } = editor();
  const fake = deferredAdapter();
  e.open(existing());
  const stale = e.save(fake.adapter);
  e.close();
  e.open(existing("B1"));
  const active = e.save(fake.adapter);
  assert.equal(e.saving, true);
  fake.reject(0);
  const result = await stale;
  assert.equal(result.current, false);
  assert.equal(e.saving, true);
  e.edit({ name: "ignored" });
  assert.equal(e.draft?.name, "B1");
  fake.applied(1);
  assert.equal((await active).current, true);
  assert.equal(e.draft, undefined);
});

test("failure and conflict retain the active input and allow an explicit retry", async () => {
  const { editor: e } = editor();
  const fake = deferredAdapter();
  e.open(existing());
  const failed = e.save(fake.adapter);
  fake.reject();
  const failure = await failed;
  assert.deepEqual(
    [failure.outcome, failure.current],
    [RepositorySaveOutcome.Failed, true],
  );
  assert.equal((failure.error as Error).message, "offline");
  assert.equal(e.saving, false);
  assert.equal(e.draft?.name, "A1");
  e.edit({ name: "A2" });
  assert.equal(e.draft?.name, "A2");

  const conflicted = e.save(fake.adapter);
  fake.conflict();
  const conflict = await conflicted;
  assert.deepEqual(
    [conflict.outcome, conflict.current],
    [RepositorySaveOutcome.Conflict, true],
  );
  assert.equal(e.draft?.name, "A2");
  assert.equal(e.saving, false);

  e.edit({ name: "A3" });
  const retry = e.save(fake.adapter);
  fake.applied();
  assert.equal((await retry).outcome, RepositorySaveOutcome.Saved);
  assert.equal(e.draft, undefined);
  assert.equal(fake.calls[2].input.displayName, "A3");
});

test("an unchanged retry after a lost edit response replays instead of conflicting", async () => {
  const server = new ServerModel("V0");
  const { editor: e } = editor();
  const { adapter, calls } = modelAdapter(server, { next: true });
  e.open(existing("updated"));
  const first = await e.save(adapter);
  assert.equal(first.outcome, RepositorySaveOutcome.Failed);
  assert.equal(e.draft?.name, "updated");
  const second = await e.save(adapter);
  assert.equal(second.outcome, RepositorySaveOutcome.Saved);
  assert.deepEqual(calls[0], calls[1]);
  assert.equal(calls[0].input.expectedVersionId, "V0");
  assert.equal(server.versions.size, 2);
  assert.deepEqual(server.heads, ["V1"]);
  assert.equal(e.draft, undefined);

  // Counterfactual: a fresh request ID misses the receipt and conflicts with the first commit.
  const control = new ServerModel("V0");
  control.commit(calls[0]);
  const fresh = structuredClone(calls[1]);
  fresh.input.meta.requestId = "fresh";
  assert.equal(control.commit(fresh).result, ResultKind.CONFLICT);
  assert.equal(control.heads.length, 2);
});

test("an unchanged retry after a lost create response replays instead of colliding", async () => {
  const server = new ServerModel();
  const { editor: e } = editor();
  const { adapter, calls } = modelAdapter(server, { next: true });
  e.open(created());
  assert.equal((await e.save(adapter)).outcome, RepositorySaveOutcome.Failed);
  assert.equal((await e.save(adapter)).outcome, RepositorySaveOutcome.Saved);
  assert.deepEqual(calls[0], calls[1]);
  assert.equal(calls[0].method, CREATE.name);
  assert.equal(server.versions.size, 1);
  assert.equal(e.draft, undefined);

  const control = new ServerModel();
  control.commit(calls[0]);
  const fresh = structuredClone(calls[1]);
  fresh.input.meta.requestId = "fresh";
  assert.throws(() => control.commit(fresh), /already in use/);
});

test("a failure before commit keeps the request ID and commits once on retry", async () => {
  const server = new ServerModel("V0");
  const { editor: e } = editor();
  const calls: Call[] = [];
  let reject = true;
  const adapter = {
    call: async (method: { name: string }, input: Call["input"]) => {
      calls.push({ method: method.name, input: structuredClone(input) });
      if (reject) {
        reject = false;
        throw new Error("rejected before commit");
      }
      return { result: { result: server.commit(calls.at(-1)!).result } };
    },
  } as unknown as Parameters<RepositoryEditor["save"]>[0];
  e.open(existing("kept"));
  assert.equal((await e.save(adapter)).outcome, RepositorySaveOutcome.Failed);
  assert.equal(e.draft?.name, "kept");
  assert.equal((await e.save(adapter)).outcome, RepositorySaveOutcome.Saved);
  assert.deepEqual(calls[0], calls[1]);
  assert.equal(server.versions.size, 2);
  assert.deepEqual(server.heads, ["V1"]);
});

test("changed content, a new editor or a different operation never reuses a request ID", async () => {
  const { editor: e } = editor();
  const fake = deferredAdapter();
  const failAndWait = async (index: number) => {
    const pending = e.save(fake.adapter);
    fake.reject(index);
    await pending;
  };
  e.open(existing("name"));
  await failAndWait(0);
  await failAndWait(1);
  assert.equal(
    fake.calls[0].input.meta.requestId,
    fake.calls[1].input.meta.requestId,
  );
  e.edit({ url: "https://example.test/team/changed" });
  await failAndWait(2);
  e.edit({ name: "other name" });
  await failAndWait(3);
  const ids = fake.calls.map((c) => c.input.meta.requestId);
  assert.equal(new Set(ids).size, 3);
  assert(fake.calls.every((c) => c.input.id === "repo-a"));
  assert.equal(fake.calls[3].input.displayName, "other name");

  // An empty display name is submitted as absent, so clearing and retrying stays consistent.
  e.edit({ name: "" });
  await failAndWait(4);
  await failAndWait(5);
  assert.equal(fake.calls[4].input.displayName, undefined);
  assert.equal(
    fake.calls[4].input.meta.requestId,
    fake.calls[5].input.meta.requestId,
  );

  // Close and reopen the identical form: a new editor session starts a new attempt.
  const previous = fake.calls[5].input.meta.requestId;
  e.close();
  e.open({ ...existing(""), url: "https://example.test/team/changed" });
  await failAndWait(6);
  assert.notEqual(fake.calls[6].input.meta.requestId, previous);

  // A different operation or base for the same entity never shares an attempt.
  e.open({ id: "repo-a", url: fake.calls[6].input.url, name: "" });
  await failAndWait(7);
  assert.equal(fake.calls[7].method, CREATE.name);
  assert.notEqual(
    fake.calls[7].input.meta.requestId,
    fake.calls[6].input.meta.requestId,
  );
  e.open({ ...existing(""), base: "V9", url: fake.calls[6].input.url });
  await failAndWait(8);
  assert.equal(fake.calls[8].input.expectedVersionId, "V9");
  assert.notEqual(
    fake.calls[8].input.meta.requestId,
    fake.calls[7].input.meta.requestId,
  );
});

test("an external edit accepted before the first save keeps the original base and conflicts", async () => {
  const server = new ServerModel("V0");
  // The independent writer commits V1 on top of V0 before this client saves.
  server.commit({
    method: SAVE.name,
    input: {
      meta: { requestId: "external" },
      id: "repo-a",
      url: "https://example.test/team/a",
      displayName: "external",
      expectedVersionId: "V0",
    },
  });
  const { editor: e } = editor();
  const { adapter, calls } = modelAdapter(server, { next: false });
  e.open(existing("mine"));
  const result = await e.save(adapter);
  assert.equal(calls[0].input.expectedVersionId, "V0");
  assert.equal(result.outcome, RepositorySaveOutcome.Conflict);
  assert.equal(server.heads.length, 2);
  assert.equal(e.draft?.name, "mine");
});
