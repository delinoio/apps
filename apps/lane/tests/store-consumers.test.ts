// SPDX-License-Identifier: Apache-2.0
// Mounted consumers of the native store subscription: URL connection settings
// and the capture composer's cached URL rules. The adapter is the real
// NativeAdapter over a synthetic store module and an event source that retains
// every callback, with a parent subscription registered first as in the app.
// These are synthetic fixtures, not native desktop acceptance.
import assert from "node:assert/strict";
import test from "node:test";
import { create, toJson } from "@bufbuild/protobuf";
import {
  EntityKind,
  EntityResultSchema,
  ListURLConnectionsResponseSchema,
  ResultKind,
} from "@repo/protos/lane/v1/lane_pb";
import {
  NativeAdapter,
  type EventSource,
  type LaneStoreModule,
} from "../src/adapter.ts";
import { toUI } from "../src/serializable.ts";
import {
  loadStoreConsumers,
  type HostNode,
  type StoreConsumersBundle,
} from "./harness/store-consumers.ts";

interface Rule {
  entityId: string;
  prefix: string;
  repositoryId: string;
}
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
const entity = (version: Record<string, unknown>) =>
  create(EntityResultSchema, {
    result: ResultKind.APPLIED,
    version: { formatVersion: 3, ...version },
  });
const repository = (entityId: string, displayName: string) =>
  entity({
    entityId,
    versionId: `${entityId}-v1`,
    kind: EntityKind.REPOSITORY,
    body: {
      case: "repository",
      value: { normalizedUrl: `example.test/${entityId}`, displayName },
    },
  });
const ruleResult = (rule: Rule) =>
  entity({
    entityId: rule.entityId,
    versionId: `${rule.entityId}-v1`,
    kind: EntityKind.URL_CONNECTION,
    body: {
      case: "urlConnection",
      value: { prefix: rule.prefix, repositoryId: rule.repositoryId },
    },
  });

// Synthetic store and host bridge shared by the parent subscription and the
// mounted consumers.
function fixture() {
  const callbacks = new Set<
    (event: { spaceId?: string; sequence?: string }) => void
  >();
  const state = {
    rules: [] as Rule[],
    pending: "0",
    listCalls: 0,
    statusCalls: 0,
    creates: 0,
  };
  const reply = (cb: (raw: string) => void, response: unknown) =>
    cb(JSON.stringify({ ok: true, response }));
  const module: LaneStoreModule = {
    open(_request, cb) {
      reply(cb, {
        spaceId: "S",
        deviceId: "device",
        formatVersion: 3,
        schemaVersion: 3,
        sequence: "0",
      });
    },
    invoke(method, json, cb) {
      const { request } = JSON.parse(json);
      if (method === "ListURLConnections") {
        state.listCalls++;
        reply(
          cb,
          toJson(
            ListURLConnectionsResponseSchema,
            create(ListURLConnectionsResponseSchema, {
              items: state.rules.map(ruleResult),
            }),
          ),
        );
      } else if (method === "GetLocalStatus") {
        state.statusCalls++;
        reply(cb, {
          spaceId: "S",
          deviceId: "device",
          syncEnabled: true,
          connected: true,
          pendingItemCount: state.pending,
        });
      } else if (method === "CreateURLConnection") {
        state.creates++;
        const rule = {
          entityId: request.id,
          prefix: request.connection.prefix,
          repositoryId: request.connection.repositoryId,
        };
        state.rules.push(rule);
        reply(cb, { result: toJson(EntityResultSchema, ruleResult(rule)) });
      } else {
        throw new Error(`Unexpected store call ${method}`);
      }
    },
    close(_space, cb) {
      cb({ ok: true, response: { completed: true } });
    },
  };
  const events: EventSource = {
    addListener(_name, cb) {
      callbacks.add(cb);
    },
    removeListener(_name, cb) {
      callbacks.delete(cb);
    },
  };
  const adapter = new NativeAdapter(module, events);
  let parentRefreshes = 0;
  // The app registers its refresh before opening and sharing the adapter.
  const stopParent = adapter.subscribe(() => parentRefreshes++);
  return {
    adapter,
    state,
    callbacks,
    parentRefreshes: () => parentRefreshes,
    stopParent,
    emit: (sequence: string) => {
      for (const cb of [...callbacks]) cb({ spaceId: "S", sequence });
    },
  };
}

let bundle: StoreConsumersBundle;
test.before(async () => {
  bundle = await loadStoreConsumers();
});

// Records the values written to text inputs so tests can check retained input.
function recordInputs() {
  const values = new Map<string, string>();
  const query: any = {
    select: (selector: string) => ({
      invoke: (call: { method: string; params?: { value?: string } }) => ({
        exec() {
          if (call.method === "setValue")
            values.set(selector, call.params?.value ?? "");
        },
      }),
    }),
  };
  (globalThis as any).lynx = { createSelectorQuery: () => query };
  return values;
}

test("an external URL-rule change refreshes the mounted list and status without remounting or losing unsaved input", async () => {
  const inputs = recordInputs();
  const f = fixture();
  f.state.rules = [
    { entityId: "rule-a", prefix: "https://a.example", repositoryId: "repo-a" },
  ];
  await f.adapter.open();
  const repositories = [
    toUIResult(repository("repo-a", "Alpha")),
    toUIResult(repository("repo-b", "Beta")),
  ];
  const view = bundle.runtime.mount(bundle.URLConnections, {
    adapter: f.adapter,
    repositories,
    dirty: () => {},
  });
  await view.settle();
  assert.equal(f.state.listCalls, 1);
  const shown = () => text({ type: "root", props: {}, children: view.tree() });
  assert.match(shown(), /https:\/\/a\.example/);
  assert.match(shown(), /Synchronized/);

  // Start an unsaved edit and type into the prefix field.
  const add = all(view.tree(), isButton).find(
    (n) => text(n) === "Add connection",
  );
  add!.props.bindtap();
  await view.settle();
  const [prefixInput] = all(view.tree(), (n) => n.type === "input");
  assert.ok(prefixInput, "prefix input is rendered");
  prefixInput.props.bindinput({ detail: { value: "https://draft.example" } });
  await view.settle();
  assert.equal(inputs.get(`#${prefixInput.props.id}`), "https://draft.example");

  // Another process commits a rule and a pending synchronization item.
  f.state.rules.push({
    entityId: "rule-b",
    prefix: "https://b.example",
    repositoryId: "repo-b",
  });
  f.state.pending = "2";
  f.emit("1");
  await view.settle();
  assert.equal(f.parentRefreshes(), 1);
  assert.equal(f.state.listCalls, 2);
  assert.equal(f.state.statusCalls, 2);
  assert.match(shown(), /https:\/\/b\.example/);
  assert.match(shown(), /Synchronization pending/);
  // The editor stays open with the same input and its unsaved value.
  const [sameInput] = all(view.tree(), (n) => n.type === "input");
  assert.equal(sameInput.props.id, prefixInput.props.id);
  assert.equal(inputs.get(`#${prefixInput.props.id}`), "https://draft.example");
  assert.doesNotMatch(shown(), /Enter an HTTP or HTTPS URL prefix/);

  // Duplicate and older events stay silent; the next newer event refreshes once.
  f.emit("1");
  await view.settle();
  assert.equal(f.state.listCalls, 2);
  f.emit("2");
  await view.settle();
  assert.equal(f.state.listCalls, 3);

  view.unmount();
  f.stopParent();
  assert.equal(f.callbacks.size, 0);
});

test("a successful local save still refreshes the list", async () => {
  recordInputs();
  const f = fixture();
  await f.adapter.open();
  const view = bundle.runtime.mount(bundle.URLConnections, {
    adapter: f.adapter,
    repositories: [toUIResult(repository("repo-a", "Alpha"))],
    dirty: () => {},
  });
  await view.settle();
  assert.match(
    text({ type: "root", props: {}, children: view.tree() }),
    /No URL connections/,
  );
  const tap = async (label: string) => {
    const [button] = all(view.tree(), isButton).filter(
      (n) => text(n) === label,
    );
    assert.ok(button, `${label} is rendered`);
    button.props.bindtap();
    await view.settle();
  };
  await tap("Add connection");
  const [input] = all(view.tree(), (n) => n.type === "input");
  input.props.bindinput({ detail: { value: "https://saved.example" } });
  await view.settle();
  await tap("Alpha");
  await tap("Save");
  assert.equal(f.state.creates, 1);
  assert.equal(f.state.listCalls, 2);
  assert.match(
    text({ type: "root", props: {}, children: view.tree() }),
    /https:\/\/saved\.example/,
  );
  view.unmount();
  f.stopParent();
});

const draftOf = (id: string, repositoryId: string, pageUrl: string) => ({
  id,
  images: [
    {
      id: `${id}-image`,
      png: "",
      width: 10,
      height: 10,
      layers: [],
      metadata: { pageUrl },
    },
  ],
  title: "Title",
  description: "",
  repositoryId,
});

test("an external URL-rule change reloads the composer rules for warnings and new drafts only", async () => {
  recordInputs();
  const f = fixture();
  f.state.rules = [
    { entityId: "rule-a", prefix: "https://a.example", repositoryId: "repo-a" },
  ];
  await f.adapter.open();
  const snapshot: { draft?: any; setDraft?: (draft: any) => void } = {};
  const view = bundle.runtime.mount(bundle.ComposerHost, {
    adapter: f.adapter,
    repositories: [
      toUIResult(repository("repo-a", "Alpha")),
      toUIResult(repository("repo-b", "Beta")),
    ],
    initial: draftOf("d1", "repo-a", "https://b.example/page"),
    snapshot,
  });
  await view.settle();
  const shown = () => text({ type: "root", props: {}, children: view.tree() });
  const warning = /A capture matches a different repository/;
  assert.equal(f.state.listCalls, 1);
  assert.doesNotMatch(shown(), warning);
  assert.equal(snapshot.draft.repositoryId, "repo-a");

  // Rule B is committed externally; the composer reloads its cached rules.
  f.state.rules = [
    { entityId: "rule-b", prefix: "https://b.example", repositoryId: "repo-b" },
  ];
  f.emit("1");
  await view.settle();
  assert.equal(f.parentRefreshes(), 1);
  assert.equal(f.state.listCalls, 2);
  // Warning matching uses the refreshed rules, but the selected repository of
  // the already matched draft is retained.
  assert.match(shown(), warning);
  assert.equal(snapshot.draft.repositoryId, "repo-a");

  // A new capture draft is matched against rule B.
  snapshot.setDraft!(draftOf("d2", "", "https://b.example/other"));
  await view.settle();
  assert.equal(snapshot.draft.id, "d2");
  assert.equal(snapshot.draft.repositoryId, "repo-b");
  assert.doesNotMatch(shown(), warning);

  view.unmount();
  f.stopParent();
  assert.equal(f.callbacks.size, 0);
});

// Components receive adapter results after toUI conversion.
function toUIResult(result: ReturnType<typeof entity>) {
  return toUI(result);
}
