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
import { draftFor } from "../src/model.ts";
import {
  loadTaskEditor,
  type HostNode,
  type TaskEditorBundle,
} from "./harness/task-editor.ts";

let bundle: TaskEditorBundle;
test.before(async () => {
  bundle = await loadTaskEditor();
});

const textOf = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(textOf).join("");
function find(
  nodes: HostNode[],
  accept: (node: HostNode) => boolean,
): HostNode[] {
  const found: HostNode[] = [];
  const walk = (node: HostNode | string) => {
    if (typeof node === "string") return;
    if (accept(node)) found.push(node);
    node.children.forEach(walk);
  };
  nodes.forEach(walk);
  return found;
}
const button = (nodes: HostNode[], label: string) => {
  const found = find(
    nodes,
    (node) =>
      node.type === "view" && node.props["accessibility-label"] === label,
  );
  assert.ok(found.length, `${label} is available`);
  return found[0];
};
const input = (nodes: HostNode[], label: string) => {
  const found = find(
    nodes,
    (node) =>
      node.type === "input" && node.props["accessibility-label"] === label,
  );
  assert.ok(found.length, `${label} is available`);
  return found[0];
};

test("single-select preserves None, keyboard selection, Escape and disabled state", async () => {
  const view = bundle.runtime.mount(bundle.SelectHost, {
    control: "priority",
    label: "Priority",
    value: "",
    options: [
      ["", "None"],
      ["high", "High"],
      ["low", "Low"],
    ],
  });
  const tree = () => view.tree();
  assert.equal(
    find(tree(), (node) => node.props.className === "task-select-options")
      .length,
    0,
  );
  const trigger = button(tree(), "Priority: None");
  trigger.props.bindkeydown({ key: "ArrowDown" });
  await view.settle();
  assert.equal(button(tree(), "Priority: None").props["data-expanded"], "true");
  assert.equal(
    find(
      tree(),
      (node) =>
        node.props["accessibility-label"] === "Priority: None" &&
        node.props["data-pressed"] === "true",
    )[0]?.props["accessibility-value"],
    "Selected",
  );
  button(tree(), "Priority: None").props.bindkeydown({ key: "ArrowDown" });
  await view.settle();
  assert.ok(bundle.focusCalls.some((call) => call.method === "setFocus"));
  button(tree(), "Priority: High").props.bindkeydown({ key: "Enter" });
  await view.settle();
  assert.equal(bundle.captured.value, "high");
  assert.equal(
    button(tree(), "Priority: High").props["data-expanded"],
    "false",
  );
  button(tree(), "Priority: High").props.bindtap();
  await view.settle();
  button(tree(), "Priority: None").props.bindkeydown({ key: "Escape" });
  await view.settle();
  assert.equal(bundle.captured.value, "high");
  assert.equal(
    button(tree(), "Priority: High").props["data-expanded"],
    "false",
  );
  assert.ok(
    bundle.focusCalls.some(
      (call) =>
        call.selector.startsWith("#task-select-") && call.method === "setFocus",
    ),
  );
  view.unmount();

  const disabled = bundle.runtime.mount(bundle.SelectHost, {
    control: "priority",
    label: "Priority",
    value: "high",
    options: [
      ["", "None"],
      ["high", "High"],
    ],
    disabled: true,
  });
  const locked = button(disabled.tree(), "Priority: High");
  assert.equal(locked.props.focusable, false);
  locked.props.bindtap();
  await disabled.settle();
  assert.equal(
    find(
      disabled.tree(),
      (node) => node.props.className === "task-select-options",
    ).length,
    0,
  );
  disabled.unmount();
});

test("label search retains selected chips and the 100-label limit", async () => {
  const options = Array.from({ length: 101 }, (_, index) => ({
    id: `label-${index}`,
    title: index === 100 ? "Last label" : `Label ${index}`,
  }));
  const selected = options.slice(0, 100).map((option) => option.id);
  const view = bundle.runtime.mount(bundle.LabelsHost, {
    selected,
    options,
    limit: 100,
    disabled: false,
  });
  const tree = () => view.tree();
  button(tree(), "Labels: 100 selected").props.bindtap();
  await view.settle();
  input(tree(), "Search labels").props.bindinput({ detail: { value: "last" } });
  await view.settle();
  assert.equal(
    find(
      tree(),
      (node) =>
        node.props["accessibility-label"] === "Remove Label 0 from Labels",
    ).length,
    1,
  );
  assert.equal(button(tree(), "Labels: Last label").props.focusable, false);
  button(tree(), "Remove Label 0 from Labels").props.bindtap();
  await view.settle();
  assert.equal(bundle.captured.selected?.length, 99);
  button(tree(), "Labels: Last label").props.bindtap();
  await view.settle();
  assert.equal(bundle.captured.selected?.length, 100);
  assert.equal(
    button(tree(), "Labels: 100 selected").props["data-expanded"],
    "true",
  );
  input(tree(), "Search labels").props.bindkeydown({ key: "Escape" });
  await view.settle();
  assert.equal(
    button(tree(), "Labels: 100 selected").props["data-expanded"],
    "false",
  );
  assert.ok(bundle.captured.selected?.includes("label-100"));
  view.unmount();
});

test("relationship header menu preserves action order, eligibility and focused search", async () => {
  const initial = {
    id: "task-self",
    dirty: false,
    task: {
      repositoryId: "repo-a",
      parentTaskId: undefined,
      duplicateOfTaskId: undefined,
      dependsOnTaskIds: [],
      relatedTaskIds: [],
    },
    relations: { subtasks: [], blocks: [] },
  };
  const tasks = [
    { id: "task-self", title: "Self" },
    ...Array.from({ length: 9 }, (_, index) => ({
      id: `candidate-${index}`,
      title: "Same title",
    })),
  ];
  const opened: string[] = [];
  const view = bundle.runtime.mount(bundle.RelationshipsHost, {
    initial,
    disabled: false,
    options: { tasks, status: "ready", error: "", retry: () => {} },
    ownId: "task-self",
    moved: false,
    openTask: (id: string) => opened.push(id),
  });
  const tree = () => view.tree();
  button(tree(), "Add relationship").props.bindtap();
  await view.settle();
  const menu = find(
    tree(),
    (node) => node.props.className === "task-select-options",
  )[0];
  assert.deepEqual(
    find(
      [menu],
      (node) =>
        node.type === "view" &&
        node.props["accessibility-label"]?.startsWith("Add "),
    ).map((node) => node.props["accessibility-label"]),
    [
      "Add parent",
      "Add subtask",
      "Add dependency",
      "Add blocked task",
      "Add related task",
      "Add original task",
    ],
  );
  button(tree(), "Add parent").props.bindtap();
  await view.settle();
  assert.equal(
    find(
      tree(),
      (node) => node.props["accessibility-label"] === "Add self to Parent",
    ).length,
    0,
  );
  assert.ok(
    find(
      tree(),
      (node) =>
        node.type === "input" &&
        node.props["accessibility-label"] === "Search tasks for Parent",
    ).length,
  );
  assert.equal(
    find(tree(), (node) => textOf(node) === "Show more matching tasks").length >
      0,
    true,
  );
  button(tree(), "Show more matching tasks").props.bindtap();
  await view.settle();
  assert.equal(
    find(
      tree(),
      (node) =>
        node.props["accessibility-label"] === "Add Same title to Parent",
    ).length,
    9,
  );
  button(tree(), "Add Same title to Parent").props.bindtap();
  await view.settle();
  assert.equal(bundle.captured.draft?.task.parentTaskId, "candidate-0");
  assert.equal(bundle.captured.draft?.dirty, true);
  button(tree(), "Add relationship").props.bindtap();
  await view.settle();
  assert.equal(button(tree(), "Add parent").props.focusable, false);
  button(tree(), "Add parent").props.bindkeydown({ key: "Escape" });
  await view.settle();
  assert.equal(
    button(tree(), "Add relationship").props["data-expanded"],
    "false",
  );
  button(tree(), "Open Same title").props.bindtap();
  assert.deepEqual(opened, ["candidate-0"]);
  button(tree(), "Remove Same title from Parent").props.bindtap();
  await view.settle();
  assert.equal(bundle.captured.draft?.task.parentTaskId, undefined);
  view.unmount();
});

test("relationship replacement closes mounted selectors and relationship menus", async () => {
  const cases = [
    {
      relationship: "parentTaskId",
      change: "Change parent",
      title: "Parent",
    },
    {
      relationship: "duplicateOfTaskId",
      change: "Change original",
      title: "Duplicate of",
    },
  ] as const;
  for (const { relationship, change, title } of cases) {
    for (const openControl of ["priority", "relationship"] as const) {
      const view = bundle.runtime.mount(bundle.RelationshipsTransitionHost, {
        initial: {
          id: "task-self",
          dirty: false,
          task: {
            repositoryId: "repo-a",
            [relationship]: "existing-task",
            parentTaskId:
              relationship === "parentTaskId" ? "existing-task" : undefined,
            duplicateOfTaskId:
              relationship === "duplicateOfTaskId"
                ? "existing-task"
                : undefined,
            dependsOnTaskIds: [],
            relatedTaskIds: [],
          },
          relations: { subtasks: [], blocks: [] },
        },
        openControl: openControl === "relationship" ? openControl : undefined,
        disabled: false,
        options: {
          tasks: [
            { id: "task-self", title: "Self" },
            { id: "existing-task", title: "Existing" },
            { id: "candidate", title: "Candidate" },
          ],
          status: "ready",
          error: "",
          retry: () => {},
        },
        ownId: "task-self",
        moved: false,
        openTask: () => {},
      });
      const tree = () => view.tree();
      if (openControl === "priority") {
        button(tree(), "Priority: None").props.bindtap();
        await view.settle();
        assert.equal(
          button(tree(), "Priority: None").props["data-expanded"],
          "true",
        );
      } else {
        assert.equal(
          button(tree(), "Add relationship").props["data-expanded"],
          "true",
        );
      }
      button(tree(), change).props.bindtap();
      await view.settle();
      assert.equal(
        button(tree(), "Priority: None").props["data-expanded"],
        "false",
      );
      assert.equal(
        button(tree(), "Add relationship").props["data-expanded"],
        "false",
      );
      assert.equal(
        find(
          tree(),
          (node) =>
            node.type === "input" &&
            node.props["accessibility-label"] === `Search tasks for ${title}`,
        ).length,
        1,
      );
      view.unmount();
    }
  }
});

test("relationship loading, failure and move keep Add guarded and expose retry", async () => {
  const initial = {
    id: "task-self",
    dirty: false,
    task: {
      repositoryId: "repo-a",
      parentTaskId: undefined,
      duplicateOfTaskId: undefined,
      dependsOnTaskIds: [],
      relatedTaskIds: [],
    },
    relations: undefined,
  };
  const props = {
    initial,
    disabled: false,
    ownId: "task-self",
    moved: false,
    openTask: () => {},
  };
  const loading = bundle.runtime.mount(bundle.RelationshipsHost, {
    ...props,
    options: { tasks: [], status: "loading", error: "", retry: () => {} },
  });
  assert.equal(
    button(loading.tree(), "Add relationship").props.focusable,
    false,
  );
  assert.ok(
    find(
      loading.tree(),
      (node) => textOf(node) === "Loading tasks of this repository…",
    ).length,
  );
  loading.unmount();

  let retried = 0;
  const failed = bundle.runtime.mount(bundle.RelationshipsHost, {
    ...props,
    options: {
      tasks: [],
      status: "error",
      error: "fixture failure",
      retry: () => retried++,
    },
  });
  assert.equal(
    button(failed.tree(), "Add relationship").props.focusable,
    false,
  );
  button(failed.tree(), "Retry loading tasks").props.bindtap();
  assert.equal(retried, 1);
  failed.unmount();

  const moved = bundle.runtime.mount(bundle.RelationshipsHost, {
    ...props,
    moved: true,
    options: { tasks: [], status: "ready", error: "", retry: () => {} },
  });
  assert.equal(button(moved.tree(), "Add relationship").props.focusable, false);
  assert.ok(
    find(moved.tree(), (node) =>
      textOf(node).includes("Relationships are cleared when a task moves."),
    ).length,
  );
  moved.unmount();
});

test("compact relationship chips keep linked actions and draft-only edits", async () => {
  const opened: string[] = [];
  const view = bundle.runtime.mount(bundle.RelationshipsHost, {
    compact: true,
    initial: {
      id: "task-self",
      dirty: false,
      task: {
        repositoryId: "repo-a",
        parentTaskId: "task-parent",
        duplicateOfTaskId: undefined,
        dependsOnTaskIds: [],
        relatedTaskIds: [],
      },
      relations: { subtasks: [], blocks: [] },
    },
    disabled: false,
    ownId: "task-self",
    moved: false,
    options: {
      tasks: [
        { id: "task-parent", title: "Parent task" },
        { id: "task-new", title: "New parent" },
      ],
      status: "ready",
      error: "",
      retry: () => {},
    },
    openTask: (id: string) => opened.push(id),
  });
  const tree = () => view.tree();
  assert.equal(
    find(tree(), (node) => node.props.className === "relation").length,
    0,
  );
  button(
    tree(),
    "Parent: Parent task; open relationship actions",
  ).props.bindtap();
  await view.settle();
  button(tree(), "Open").props.bindkeydown({ key: "Escape" });
  await view.settle();
  assert.equal(
    find(tree(), (node) => textOf(node) === "Close relationship actions")
      .length,
    0,
  );
  button(
    tree(),
    "Parent: Parent task; open relationship actions",
  ).props.bindtap();
  await view.settle();
  button(tree(), "Open").props.bindtap();
  await view.settle();
  assert.deepEqual(opened, ["task-parent"]);
  button(
    tree(),
    "Parent: Parent task; open relationship actions",
  ).props.bindtap();
  await view.settle();
  button(tree(), "Change").props.bindtap();
  await view.settle();
  button(tree(), "Add New parent to Parent").props.bindtap();
  await view.settle();
  assert.equal(bundle.captured.draft?.task.parentTaskId, "task-new");
  assert.equal(bundle.captured.draft?.dirty, true);
  button(
    tree(),
    "Parent: New parent; open relationship actions",
  ).props.bindtap();
  await view.settle();
  button(tree(), "Remove").props.bindtap();
  await view.settle();
  assert.equal(bundle.captured.draft?.task.parentTaskId, undefined);
  assert.ok(
    find(tree(), (node) => textOf(node) === "No relationships.").length,
  );
  view.unmount();
});

test("retained task editors restore relationship focus to their own chip", async () => {
  const props = {
    compact: true,
    initial: {
      id: "task-self",
      dirty: false,
      task: {
        repositoryId: "repo-a",
        parentTaskId: "task-parent",
        dependsOnTaskIds: [],
        relatedTaskIds: [],
      },
      relations: { subtasks: [], blocks: [] },
    },
    disabled: false,
    ownId: "task-self",
    moved: false,
    options: {
      tasks: [{ id: "task-parent", title: "Parent task" }],
      status: "ready",
      error: "",
      retry: () => {},
    },
    openTask: () => {},
  };
  // Both instances remain mounted, as when task tabs retain their editors.
  const { jsx } = bundle.runtime;
  const view = bundle.runtime.mount(
    () =>
      jsx("view", {
        children: [
          jsx("view", {
            hidden: true,
            children: jsx(bundle.RelationshipsHost, props),
          }),
          jsx("view", { children: jsx(bundle.RelationshipsHost, props) }),
        ],
      }),
    {},
  );
  const panels = () => view.tree()[0].children as HostNode[];
  const label = "Parent: Parent task; open relationship actions";
  const retainedId = button([panels()[0]], label).props.id;
  const activeId = button([panels()[1]], label).props.id;
  assert.notEqual(activeId, retainedId);
  button([panels()[1]], label).props.bindtap();
  await view.settle();
  button([panels()[1]], "Open").props.bindkeydown({ key: "Escape" });
  await view.settle();
  assert.deepEqual(bundle.focusCalls.at(-1), {
    selector: `#${activeId}`,
    method: "setFocus",
  });
  assert.equal(button([panels()[1]], label).props.id, activeId);
  view.unmount();
});

test("task editor selectors change the draft before explicit Save", async () => {
  const initial = draftFor(
    create(EntityResultSchema, {
      version: create(EntityVersionSchema, {
        entityId: "task-1",
        versionId: "v1",
        body: {
          case: "task",
          value: create(TaskSchema, {
            title: "Fixture",
            repositoryId: "repo-a",
            labelIds: ["missing-label"],
            milestoneId: "missing-milestone",
            cycleId: "missing-cycle",
            progress: Progress.TODO,
            storyPoints: 0,
            startDate: "2028-02-29",
            parentTaskId: "missing-parent",
            proposal: Proposal.REGULAR,
          }),
        },
      }),
    }),
  );
  const calls: { method: string; request: any }[] = [];
  const adapter = {
    mode: "web",
    call: async (method: { name: string }, request: any) => {
      calls.push({ method: method.name, request });
      if (method.name === "ListTasks") return { items: [], nextCursor: "" };
      return { result: undefined };
    },
    local: async () => ({}),
    subscribe: () => () => {},
    close: () => {},
    open: async () => {},
  };
  const view = bundle.runtime.mount(bundle.Host, {
    adapter,
    initial,
    repositories: [
      {
        version: {
          entityId: "repo-a",
          body: { case: "repository", value: { url: "example/a", name: "A" } },
        },
      },
    ],
    planning: { items: [], status: "ready", error: "", retry: () => {} },
    revision: 0,
    openTask: () => {},
    beginCompletion: () => ({ saved: () => {}, close: () => {} }),
    onClose: () => {},
  });
  await view.settle();
  const tree = () => view.tree();
  const layout = () =>
    find(tree(), (node) =>
      node.props.className?.startsWith("editor-content"),
    )[0];
  assert.equal(layout().props.className, "editor-content");
  assert.ok(button(tree(), "Edit Points: 0"));
  assert.ok(button(tree(), "Edit Start: 2028-02-29"));
  assert.ok(
    button(tree(), "Parent: (unavailable task); open relationship actions"),
  );
  assert.equal(
    find(
      tree(),
      (node) =>
        node.type === "input" &&
        node.props["accessibility-label"] === "Start date (YYYY-MM-DD)",
    ).length,
    0,
  );
  layout().props.bindlayoutchange({ detail: { width: 1100 } });
  await view.settle();
  assert.equal(layout().props.className, "editor-content editor-content-wide");
  layout().props.bindlayoutchange({ detail: { width: 1440 } });
  await view.settle();
  assert.equal(layout().props.className, "editor-content editor-content-wide");
  layout().props.bindlayoutchange({ detail: { width: 1100 } });
  await view.settle();
  assert.equal(
    button(tree(), "Milestone: (unavailable milestone)").props.focusable,
    true,
  );
  assert.equal(
    button(tree(), "Cycle: (unavailable cycle)").props.focusable,
    true,
  );
  assert.equal(
    button(tree(), "Remove (unavailable label) from Labels").props.focusable,
    true,
  );
  button(tree(), "Milestone: (unavailable milestone)").props.bindtap();
  await view.settle();
  button(tree(), "Milestone: None").props.bindtap();
  await view.settle();
  assert.equal(bundle.captured.draft?.task.milestoneId, undefined);
  button(tree(), "Progress: To do").props.bindtap();
  await view.settle();
  button(tree(), "Progress: In progress").props.bindtap();
  await view.settle();
  button(tree(), "Priority: None").props.bindtap();
  await view.settle();
  button(tree(), "Priority: High").props.bindtap();
  await view.settle();
  assert.equal(bundle.captured.draft?.dirty, true);
  assert.equal(calls.filter((call) => call.method === "SaveTask").length, 0);
  layout().props.bindlayoutchange({ detail: { width: 1099 } });
  await view.settle();
  assert.equal(layout().props.className, "editor-content");
  for (const width of [720, 390, 360]) {
    layout().props.bindlayoutchange({ detail: { width } });
    await view.settle();
    assert.equal(layout().props.className, "editor-content");
    assert.ok(button(tree(), "Edit Points: 0"));
  }
  assert.ok(button(tree(), "Edit Priority: High"));
  assert.ok(button(tree(), "Edit Points: 0"));
  assert.equal(
    find(tree(), (node) => node.props.className === "relation").length,
    0,
  );
  button(tree(), "Add or edit property").props.bindtap();
  await view.settle();
  button(tree(), "Story points").props.bindkeydown({ key: "Escape" });
  await view.settle();
  assert.equal(
    find(tree(), (node) => textOf(node) === "Story points").length,
    0,
  );
  button(tree(), "Add or edit property").props.bindtap();
  await view.settle();
  button(tree(), "Story points").props.bindtap();
  await view.settle();
  input(tree(), "Story points (whole number)").props.bindinput({
    detail: { value: "1.5" },
  });
  await view.settle();
  assert.ok(button(tree(), "Edit Points: 1.5"));
  assert.equal(calls.filter((call) => call.method === "SaveTask").length, 0);
  input(tree(), "Story points (whole number)").props.bindinput({
    detail: { value: "0" },
  });
  await view.settle();
  assert.equal(
    button(tree(), "Progress: In progress").props["data-expanded"],
    "false",
  );
  find(
    tree(),
    (node) => node.type === "view" && textOf(node) === "Save",
  )[0]?.props.bindtap();
  await view.settle();
  const saved = calls.filter((call) => call.method === "SaveTask");
  assert.equal(saved.length, 1);
  assert.equal(saved[0].request.task.priority, 2);
  assert.equal(saved[0].request.task.progress, Progress.IN_PROGRESS);
  view.unmount();
});
