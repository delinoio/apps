// SPDX-License-Identifier: Apache-2.0
// Real composer presentation with a synthetic draft and adapter. This is not
// native capture or platform accessibility acceptance.
import assert from "node:assert/strict";
import test from "node:test";
import {
  loadStoreConsumers,
  type HostNode,
} from "./harness/store-consumers.ts";

const text = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(text).join("");
function all(
  nodes: (HostNode | string)[],
  match: (node: HostNode) => boolean,
): HostNode[] {
  const found: HostNode[] = [];
  const walk = (node: HostNode | string) => {
    if (typeof node === "string") return;
    if (match(node)) found.push(node);
    node.children.forEach(walk);
  };
  nodes.forEach(walk);
  return found;
}
const isButton = (node: HostNode) =>
  /(^| )button( |$)/.test(node.props.className ?? "");
const repository = (entityId: string, displayName: string) => ({
  alternatives: [],
  version: {
    entityId,
    body: { case: "repository", value: { displayName } },
  },
});
const adapter = {
  spaceId: "scratch",
  subscribe: () => () => {},
  async call(method: { name: string }) {
    if (method.name === "ListRepositories")
      return { items: [repository("repo-a", "Alpha")], nextCursor: "" };
    return { items: [], nextCursor: "" };
  },
};
const draft = (clipped?: boolean) => ({
  id: "draft",
  requestId: "request",
  title: "",
  description: "",
  repositoryId: "",
  images: [
    {
      id: "image",
      png: "",
      width: 10,
      height: 10,
      metadata: {},
      ...(clipped === undefined ? {} : { clipped }),
      layers: [
        { type: "draw", points: [] },
        { type: "cover", x: 0, y: 0, width: 1, height: 1 },
        { type: "text", x: 0, y: 0, width: 1, height: 1, text: "Note" },
      ],
    },
  ],
});

test("composer exposes tool selection, layer names, validation and clipping in text", async () => {
  const bundle = await loadStoreConsumers();
  const view = bundle.runtime.mount(bundle.ComposerHost, {
    adapter,
    initial: draft(true),
    snapshot: {},
  });
  await view.settle();
  const shown = () => view.tree().map(text).join("");
  const button = (label: string) => {
    const found = all(view.tree(), isButton).find(
      (node) => text(node) === label,
    );
    assert.ok(found, `${label} is rendered`);
    return found;
  };

  // Layers use the toolbar names rather than host identifiers.
  const layers = all(view.tree(), (node) =>
    (node.props.className ?? "").split(" ").includes("composer-layer"),
  ).map((node) => text(node.children[0] as HostNode));
  assert.deepEqual(layers, ["1. Freehand", "2. Opaque cover", "3. Text: Note"]);

  // The selected tool is announced and styled through the pressed state.
  assert.equal(button("Arrow").props["data-pressed"], "true");
  assert.equal(button("Arrow").props["accessibility-value"], "Selected");
  assert.equal(button("Freehand").props["data-pressed"], "false");
  button("Opaque cover").props.bindtap();
  await view.settle();
  assert.equal(button("Opaque cover").props["accessibility-value"], "Selected");
  assert.equal(button("Arrow").props["accessibility-value"], undefined);

  // Validation messages are part of the affected control's accessible name.
  const title = all(view.tree(), (node) => node.type === "input").find((node) =>
    String(node.props["accessibility-label"]).startsWith("Title"),
  );
  assert.equal(title?.props["accessibility-label"], "Title. Enter a title.");
  const alpha = button("Alpha");
  assert.equal(
    alpha.props["accessibility-label"],
    "Repository: Alpha. Select a repository. A deleted or conflicted repository must be reselected.",
  );
  assert.equal(alpha.props["accessibility-value"], undefined);
  alpha.props.bindtap();
  await view.settle();
  assert.equal(
    button("Alpha").props["accessibility-label"],
    "Repository: Alpha",
  );
  assert.equal(button("Alpha").props["accessibility-value"], "Selected");
  assert.doesNotMatch(shown(), /Select a repository\./);

  assert.match(
    shown(),
    /This capture contains only the visible part of the element\. Scroll and capture again to include more\./,
  );
  view.unmount();

  const whole = bundle.runtime.mount(bundle.ComposerHost, {
    adapter,
    initial: draft(),
    snapshot: {},
  });
  await whole.settle();
  assert.doesNotMatch(
    whole.tree().map(text).join(""),
    /only the visible part of the element/,
  );
  whole.unmount();
});
