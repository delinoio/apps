// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { create } from "@bufbuild/protobuf";
import {
  EntityResultSchema,
  EntityVersionSchema,
  Progress,
  Proposal,
  TaskSchema,
} from "@repo/protos/lane/v1/lane_pb";
import { draftFor, receiveExternal, type Draft } from "../src/model.ts";
import {
  RequestAttempt,
  SpecializedAction,
  type SpecializedRequest,
} from "../src/requestAttempt.ts";

// Models the store receipt rules the editor depends on: a request ID replays its
// original result before any stale check, the same ID under a different logical
// request is rejected, and a fresh ID against a superseded base forks a second head.
class FakeStore {
  versions = new Map<
    string,
    { id: string; prompt: string; proposal: number }
  >();
  heads: string[] = [];
  receipts = new Map<string, { request: string; versionId: string }>();
  sent: { requestId: string; request: SpecializedRequest }[] = [];
  committedReplyLost = false;
  failBeforeCommit = false;
  private counter = 0;

  constructor(prompt: string, proposal = Proposal.PENDING) {
    this.commit(prompt, proposal, "v0");
  }
  private commit(prompt: string, proposal: number, versionId?: string) {
    const id = versionId ?? `v${++this.counter}`;
    this.versions.set(id, { id, prompt, proposal });
    return id;
  }
  seed(id: string, prompt: string) {
    this.versions.set(id, { id, prompt, proposal: Proposal.PENDING });
    this.heads = [id];
  }
  get counts() {
    return {
      versions: this.versions.size,
      heads: this.heads.length,
      receipts: this.receipts.size,
    };
  }
  head() {
    return this.versions.get(this.heads[0]!)!;
  }
  async call(requestId: string, request: SpecializedRequest) {
    this.sent.push({ requestId, request });
    if (this.failBeforeCommit)
      throw new Error("Native service is unavailable.");
    const key = JSON.stringify(request);
    const receipt = this.receipts.get(requestId);
    if (receipt) {
      if (receipt.request !== key) throw new Error("REQUEST_ID_REUSED");
      return this.finish(receipt.versionId);
    }
    const base = this.versions.get(request.expectedVersionId)!;
    const prompt =
      request.action === SpecializedAction.RestorePrompt
        ? this.versions.get(request.sourceVersionId)!.prompt
        : base.prompt;
    const proposal =
      request.action === SpecializedAction.SetProposal
        ? request.proposal
        : base.proposal;
    const versionId = this.commit(prompt, proposal);
    // Without a replay, a stale base keeps both branches as competing heads.
    this.heads = this.heads.includes(request.expectedVersionId)
      ? this.heads
          .filter((h) => h !== request.expectedVersionId)
          .concat(versionId)
      : this.heads.concat(versionId);
    this.receipts.set(requestId, { request: key, versionId });
    return this.finish(versionId);
  }
  private finish(versionId: string) {
    if (this.committedReplyLost) {
      this.committedReplyLost = false;
      throw new Error("Native service is unavailable. Local data is retained.");
    }
    return versionId;
  }
}

const resultOf = (store: FakeStore) => {
  const v = store.head();
  return create(EntityResultSchema, {
    version: create(EntityVersionSchema, {
      entityId: "task",
      versionId: v.id,
      body: {
        case: "task",
        value: create(TaskSchema, {
          title: "t",
          progress: Progress.BLOCKED,
          proposal: v.proposal,
          prompt: v.prompt,
        }),
      },
    }),
  });
};
const setup = (prompt = "old prompt") => {
  const store = new FakeStore(prompt);
  store.heads = ["v0"];
  let draft: Draft = draftFor(resultOf(store));
  const ids: string[] = [];
  const attempt = new RequestAttempt(() => {
    const id = `req-${ids.length + 1}`;
    ids.push(id);
    return id;
  });
  const decide = (proposal: number) => {
    const request: SpecializedRequest = {
      action: SpecializedAction.SetProposal,
      id: draft.id,
      expectedVersionId: draft.base,
      proposal,
    };
    return attempt.submit(request, (requestId) =>
      store.call(requestId, request),
    );
  };
  const restore = (sourceVersionId: string) => {
    const request: SpecializedRequest = {
      action: SpecializedAction.RestorePrompt,
      id: draft.id,
      expectedVersionId: draft.base,
      sourceVersionId,
    };
    return attempt.submit(request, (requestId) =>
      store.call(requestId, request),
    );
  };
  // Mirrors a successful completion: the editor adopts the result as its new base.
  const adopt = () => {
    draft = draftFor(resultOf(store));
  };
  // Mirrors App refresh of a clean draft between a failure and the retry.
  const refresh = () => {
    draft = receiveExternal(draft, resultOf(store));
  };
  return {
    store,
    attempt,
    decide,
    restore,
    adopt,
    refresh,
    ids,
    draft: () => draft,
  };
};

for (const target of [Proposal.ACCEPTED, Proposal.REJECTED]) {
  test(`unchanged retry of a lost ${Proposal[target]} reply replays the committed decision`, async () => {
    const t = setup();
    t.store.committedReplyLost = true;
    await assert.rejects(t.decide(target), /unavailable/);
    const committed = { ...t.store.counts };
    t.refresh();
    assert.equal(t.draft().base, "v0");
    assert.equal(t.draft().dirty, false);
    assert.equal(t.draft().external, true);
    await t.decide(target);
    assert.equal(t.store.sent.length, 2);
    assert.deepEqual(t.store.sent[0], t.store.sent[1]);
    assert.deepEqual(t.store.counts, committed);
    assert.equal(t.store.counts.heads, 1);
    assert.equal(t.store.head().proposal, target);
    assert.equal(
      resultOf(t.store).version?.body.value?.progress,
      Progress.BLOCKED,
    );
  });
}

test("a fresh request ID per click would fork the task, which the retained ID prevents", async () => {
  const t = setup();
  t.store.committedReplyLost = true;
  await assert.rejects(t.decide(Proposal.ACCEPTED));
  // Counterfactual: the previous behavior allocated a new ID for the same request.
  const request: SpecializedRequest = {
    action: SpecializedAction.SetProposal,
    id: "task",
    expectedVersionId: "v0",
    proposal: Proposal.ACCEPTED,
  };
  await t.store.call("fresh", request);
  assert.equal(t.store.counts.heads, 2);
});

test("prompt restoration retries replay the same request and add one version", async () => {
  const t = setup("new prompt");
  t.store.seed("p", "old prompt");
  t.store.versions.set("v0", {
    id: "v0",
    prompt: "new prompt",
    proposal: Proposal.PENDING,
  });
  t.store.heads = ["v0"];
  t.store.committedReplyLost = true;
  await assert.rejects(t.restore("p"), /unavailable/);
  const committed = { ...t.store.counts };
  t.refresh();
  await t.restore("p");
  assert.deepEqual(t.store.sent[0], t.store.sent[1]);
  assert.deepEqual(t.store.counts, committed);
  assert.equal(t.store.counts.heads, 1);
  assert.equal(t.store.head().prompt, "old prompt");
});

for (const [label, source, current] of [
  ["absent", "", "new"],
  ["empty", "", ""],
  ["unchanged", "same", "same"],
] as const) {
  test(`${label} prompt source: retry adds no version and a later restoration adds one`, async () => {
    const t = setup(current);
    t.store.versions.set("p", {
      id: "p",
      prompt: source,
      proposal: Proposal.PENDING,
    });
    t.store.committedReplyLost = true;
    await assert.rejects(t.restore("p"));
    const committed = { ...t.store.counts };
    t.refresh();
    await t.restore("p");
    assert.deepEqual(t.store.counts, committed);
    assert.equal(t.store.counts.versions, 3);
    t.adopt();
    // The confirmed success ended the attempt, so this is an independent action.
    await t.restore("p");
    assert.equal(t.ids.length, 2);
    assert.equal(t.store.counts.versions, 4);
    assert.equal(t.store.counts.heads, 1);
    assert.equal(t.store.head().prompt, source);
  });
}

test("a changed action, target, source, task or base gets a fresh ID", async () => {
  const t = setup("new prompt");
  t.store.versions.set("p", {
    id: "p",
    prompt: "one",
    proposal: Proposal.PENDING,
  });
  t.store.versions.set("q", {
    id: "q",
    prompt: "two",
    proposal: Proposal.PENDING,
  });
  t.store.failBeforeCommit = true;
  await assert.rejects(t.decide(Proposal.ACCEPTED));
  await assert.rejects(t.decide(Proposal.REJECTED));
  await assert.rejects(t.restore("p"));
  await assert.rejects(t.restore("q"));
  assert.deepEqual(t.ids, ["req-1", "req-2", "req-3", "req-4"]);
  assert.equal(t.store.counts.receipts, 0);
  // Same logical request as the last failure, but another base.
  const other: SpecializedRequest = {
    action: SpecializedAction.RestorePrompt,
    id: "task",
    expectedVersionId: "other",
    sourceVersionId: "q",
  };
  await assert.rejects(
    t.attempt.submit(other, () => Promise.reject(new Error("x"))),
  );
  await assert.rejects(
    t.attempt.submit({ ...other, id: "task-2" }, () =>
      Promise.reject(new Error("x")),
    ),
  );
  assert.deepEqual(t.ids, [
    "req-1",
    "req-2",
    "req-3",
    "req-4",
    "req-5",
    "req-6",
  ]);
});

test("a refresh that advances the base never reuses the earlier attempt", async () => {
  const t = setup();
  t.store.failBeforeCommit = true;
  await assert.rejects(t.decide(Proposal.ACCEPTED));
  const draft = t.draft();
  assert.equal(draft.base, "v0");
  // A real competing writer moved the task; the user reviews and reopens it.
  t.store.failBeforeCommit = false;
  t.store.versions.set("w", {
    id: "w",
    prompt: "other",
    proposal: Proposal.PENDING,
  });
  t.store.heads = ["w"];
  t.adopt();
  await t.decide(Proposal.ACCEPTED);
  assert.deepEqual(t.ids, ["req-1", "req-2"]);
  assert.notEqual(t.store.sent[0]!.requestId, t.store.sent[1]!.requestId);
});

test("a failure before commit keeps the ID, which then commits once", async () => {
  const t = setup();
  t.store.failBeforeCommit = true;
  await assert.rejects(t.decide(Proposal.ACCEPTED));
  assert.equal(t.store.counts.receipts, 0);
  t.store.failBeforeCommit = false;
  await t.decide(Proposal.ACCEPTED);
  assert.equal(t.store.sent[0]!.requestId, t.store.sent[1]!.requestId);
  assert.equal(t.store.counts.receipts, 1);
  assert.equal(t.store.counts.heads, 1);
  // Cleanup after confirmed success: a later independent action is a new request.
  t.adopt();
  await t.decide(Proposal.REJECTED);
  assert.equal(t.ids.length, 2);
  assert.equal(t.store.counts.receipts, 2);
});

test("a real competing writer still forks the task with a fresh ID", async () => {
  const t = setup();
  // Another writer committed a head after this editor loaded its base.
  t.store.versions.set("w", {
    id: "w",
    prompt: "other",
    proposal: Proposal.PENDING,
  });
  t.store.heads = ["w"];
  await t.decide(Proposal.ACCEPTED);
  assert.equal(t.store.counts.heads, 2);
});

test("an older reply does not end a newer attempt", async () => {
  const ids: string[] = [];
  const attempt = new RequestAttempt(() => {
    ids.push(`r${ids.length + 1}`);
    return ids[ids.length - 1]!;
  });
  const first: SpecializedRequest = {
    action: SpecializedAction.SetProposal,
    id: "task",
    expectedVersionId: "v0",
    proposal: Proposal.ACCEPTED,
  };
  let finishFirst!: () => void;
  const slow = attempt.submit(
    first,
    () => new Promise<void>((r) => (finishFirst = r)),
  );
  await assert.rejects(
    attempt.submit({ ...first, proposal: Proposal.REJECTED }, () =>
      Promise.reject(new Error("x")),
    ),
  );
  finishFirst();
  await slow;
  // The newer rejected attempt is still pending and keeps its ID on retry.
  await assert.rejects(
    attempt.submit({ ...first, proposal: Proposal.REJECTED }, () =>
      Promise.reject(new Error("x")),
    ),
  );
  assert.deepEqual(ids, ["r1", "r2"]);
});
