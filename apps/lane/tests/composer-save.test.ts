// SPDX-License-Identifier: Apache-2.0
// Real controller and composer with synthetic native/store failures. This does
// not establish platform capture or installation acceptance.
import assert from "node:assert/strict";
import test from "node:test";
import {
  loadStoreConsumers,
  type HostNode,
} from "./harness/store-consumers.ts";

const { RealQA } = await import(
  new URL("../native/desktop/realqa.mjs", import.meta.url).href
);
const content = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(content).join("");
function saveButton(nodes: (HostNode | string)[]): HostNode | undefined {
  for (const node of nodes) {
    if (typeof node === "string") continue;
    if (
      /^(Save to Lane|Retry)$/.test(content(node)) &&
      /(^| )button( |$)/.test(node.props.className ?? "")
    )
      return node;
    const child = saveButton(node.children);
    if (child) return child;
  }
}

test("a changed failed save and its lost response replay through a rebuilt composer", async () => {
  const bundle = await loadStoreConsumers();
  const host = new RealQA({
    native: {},
    emit() {},
    showComposer() {},
    closeComposer() {},
  });
  host.appendCapture({ png: "AQID", width: 1, height: 1 });
  await host.dispatch({
    op: "updateDraft",
    title: "First",
    repositoryId: "repo",
  });
  const requests: any[] = [];
  const receipts = new Map<string, string>();
  let committed = 0;
  const adapter = {
    spaceId: "scratch",
    subscribe: () => () => {},
    async call(method: { name: string }, request: any) {
      if (method.name === "ListRepositories")
        return {
          items: [
            {
              alternatives: [],
              version: {
                entityId: "repo",
                body: { case: "repository", value: { displayName: "Fixture" } },
              },
            },
          ],
        };
      if (method.name === "ListURLConnections") return { items: [] };
      assert.equal(method.name, "CreateCaptureTask");
      requests.push(structuredClone(request));
      if (requests.length === 1) throw new Error("Storage unavailable");
      const key = JSON.stringify(request);
      const previous = receipts.get(request.meta.requestId);
      if (previous)
        assert.equal(key, previous, "a replay must have the original payload");
      else {
        receipts.set(request.meta.requestId, key);
        committed++;
      }
      if (requests.length === 2) throw new Error("Reply lost after commit");
      return { result: { version: { entityId: request.id } } };
    },
  };
  const snapshot: { setDraft?: (draft: any) => void } = {};
  const mount = () =>
    bundle.runtime.mount(bundle.ComposerHost, {
      adapter,
      initial: structuredClone(host.draft),
      snapshot,
      captureHost: (input: Record<string, unknown>) => host.dispatch(input),
    });
  let view = mount();
  const save = async () => {
    await view.settle();
    const button = saveButton(view.tree());
    assert.ok(button, "Save task is available");
    assert.notEqual(button.props["accessibility-value"], "Disabled");
    button.props.bindtap();
    await view.settle();
  };
  await save();
  assert.match(view.tree().map(content).join(""), /Storage unavailable/);
  assert.equal(host.saving, false);
  await host.dispatch({ op: "updateDraft", title: "Revised" });
  snapshot.setDraft!(structuredClone(host.draft));
  await save();
  assert.match(view.tree().map(content).join(""), /Reply lost after commit/);
  assert.equal(host.draft.title, "Revised");
  assert.notEqual(requests[0].meta.requestId, requests[1].meta.requestId);
  view.unmount();
  view = mount();
  await save();
  assert.equal(requests.length, 3);
  assert.deepEqual(requests[2], requests[1]);
  assert.equal(committed, 1, "lost response retry creates exactly one task");
  assert.equal(host.draft, null, "confirmed replay clears draft memory");
  view.unmount();
});
