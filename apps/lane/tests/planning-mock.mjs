// SPDX-License-Identifier: Apache-2.0
// Test-only planning fixture for the preview server (LANE_PREVIEW_SCENARIO=planning).
// It records every request so browser checks can assert the exact ProtoJSON sent.
// Illustrative data only; never seeded product data.
const id = (n) => `019a196d-0000-7000-8000-${String(n).padStart(12, "0")}`;
export const ids = {
  repoApps: id(1),
  repoDocs: id(101),
  taskA: id(3),
  taskB: id(21),
  taskC: id(22),
  taskDocs: id(23),
  labelBackend: id(31),
  labelFrontend: id(32),
  labelDocsBackend: id(33),
  milestone: id(41),
  cycle: id(51),
};
const origin = { kind: "ORIGIN_KIND_APP", deviceName: "Test device" };
const version = (entityId, versionId, body, extra = {}) => ({
  formatVersion: 3,
  entityId,
  versionId,
  ...body,
  origin,
  createdAt: "2026-10-25T00:00:00Z",
  ...extra,
});
const applied = (v) => ({
  result: "RESULT_KIND_APPLIED",
  version: v,
  alternatives: [],
});
export function createPlanningState(baseTask, repoApps) {
  const state = {
    requests: [],
    failNext: undefined,
    delayMs: 0,
    repositories: [
      {
        ...repoApps,
        repository: { ...repoApps.repository, displayName: "apps" },
      },
      version(ids.repoDocs, id(102), {
        kind: "ENTITY_KIND_REPOSITORY",
        repository: {
          normalizedUrl: "github.com/delinoio/docs",
          displayName: "docs",
        },
      }),
    ],
    items: [
      version(ids.labelBackend, id(61), {
        label: {
          repositoryId: ids.repoApps,
          name: "Backend",
          color: "#075AFF",
          description: "Server work",
        },
      }),
      version(ids.labelFrontend, id(62), {
        label: {
          repositoryId: ids.repoApps,
          name: "Frontend",
          color: "#1B7F4B",
          description: "",
        },
      }),
      version(ids.labelDocsBackend, id(63), {
        label: {
          repositoryId: ids.repoDocs,
          name: "Backend",
          color: "#A32A25",
          description: "",
        },
      }),
      version(ids.milestone, id(64), {
        milestone: {
          repositoryId: ids.repoApps,
          name: "v1 launch",
          description: "",
          targetDate: "2026-12-01",
          completed: false,
        },
      }),
      version(ids.cycle, id(65), {
        cycle: {
          repositoryId: ids.repoApps,
          name: "Sprint 1",
          description: "",
          startDate: "2026-11-02",
          endDate: "2026-11-15",
        },
      }),
    ],
    tasks: [],
    history: new Map(),
  };
  const task = (entityId, repositoryId, title, fields = {}) =>
    version(entityId, id(Number(entityId.slice(-3)) + 700), {
      kind: "ENTITY_KIND_TASK",
      task: {
        repositoryId,
        title,
        description: "",
        progress: "PROGRESS_TODO",
        proposal: "PROPOSAL_REGULAR",
        ...fields,
      },
    });
  state.tasks = [
    {
      ...baseTask,
      task: {
        ...baseTask.task,
        priority: "TASK_PRIORITY_HIGH",
        labelIds: [ids.labelBackend],
        dueDate: "2026-11-20",
        storyPoints: 3,
        milestoneId: ids.milestone,
      },
    },
    task(ids.taskB, ids.repoApps, "Write onboarding guide", {
      type: "TASK_TYPE_DOCUMENTATION",
      parentTaskId: ids.taskC,
    }),
    task(ids.taskC, ids.repoApps, "Plan the launch"),
    task(ids.taskDocs, ids.repoDocs, "Fix docs typo", {
      labelIds: [ids.labelDocsBackend],
    }),
  ];
  state.itemHistory = (entityId) => state.history.get(entityId);
  state.version = version;
  return state;
}
// Hand-encoded lane.v1.ErrorDetail: reason (field 1) = CONSTRAINT_VIOLATION (19), violation (field 6).
const violations = { PARENT_CYCLE: 1, DEPENDENCY_CYCLE: 2, DUPLICATE_NAME: 7 };
const errorBody = (code, message, violation) => ({
  code,
  message,
  details: [
    {
      type: "lane.v1.ErrorDetail",
      value: Buffer.from(
        violation ? [0x08, 19, 0x30, violations[violation]] : [0x08, 1],
      )
        .toString("base64")
        .replace(/=+$/, ""),
    },
  ],
});
const pick = (items, repositoryId) =>
  items.filter(
    (v) =>
      !repositoryId ||
      (v.task ?? v.label ?? v.milestone ?? v.cycle ?? v.repository)
        ?.repositoryId === repositoryId,
  );
let counter = 900;
export async function handle(state, method, input) {
  state.requests.push({ method, input });
  if (state.delayMs && /^(Save|Create|Delete)/.test(method))
    await new Promise((resolve) => setTimeout(resolve, state.delayMs));
  if (state.failNext && state.failNext.method === method) {
    const { code, message, violation, status } = state.failNext;
    state.failNext = undefined;
    return { status: status ?? 400, body: errorBody(code, message, violation) };
  }
  switch (method) {
    case "ListRepositories":
      return { body: { items: state.repositories.map(applied) } };
    case "ListTasks":
      return {
        body: { items: pick(state.tasks, input.repositoryId).map(applied) },
      };
    case "GetTask":
      return {
        body: {
          result: applied(state.tasks.find((t) => t.entityId === input.id)),
        },
      };
    case "ListPlanningItems": {
      const items = pick(state.items, input.repositoryId).filter(
        (v) =>
          !input.kind ||
          input.kind === "ENTITY_KIND_UNSPECIFIED" ||
          v.kind === input.kind ||
          (v.label && input.kind === "ENTITY_KIND_LABEL") ||
          (v.milestone && input.kind === "ENTITY_KIND_MILESTONE") ||
          (v.cycle && input.kind === "ENTITY_KIND_CYCLE"),
      );
      return { body: { items: items.map(applied) } };
    }
    case "GetPlanningItem":
      return {
        body: {
          result: applied(state.items.find((t) => t.entityId === input.id)),
        },
      };
    case "CreatePlanningItem": {
      const created = version(input.id, id(++counter), input.item);
      state.items.push(created);
      state.history.set(input.id, [created]);
      return { body: { result: applied(created) } };
    }
    case "SavePlanningItem": {
      const next = version(input.id, id(++counter), input.item);
      state.items = state.items.map((v) =>
        v.entityId === input.id ? next : v,
      );
      const earlier =
        state.history.get(input.id) ??
        [state.items.find(() => false)].filter(Boolean);
      state.history.set(input.id, [...earlier, next]);
      return { body: { result: applied(next) } };
    }
    case "DeletePlanningItem": {
      state.items = state.items.filter((v) => v.entityId !== input.id);
      return { body: { result: { result: "RESULT_KIND_DELETED" } } };
    }
    case "SaveTask": {
      const next = {
        ...state.tasks.find((t) => t.entityId === input.id),
        versionId: id(++counter),
        task: input.task,
      };
      state.tasks = state.tasks.map((t) =>
        t.entityId === input.id ? next : t,
      );
      return { body: { result: applied(next), affected: [] } };
    }
    case "CreateTask": {
      const { meta, repository, id: entityId, ...fields } = input;
      const created = version(entityId, id(++counter), {
        kind: "ENTITY_KIND_TASK",
        task: { ...fields, repositoryId: repository?.id ?? ids.repoApps },
      });
      state.tasks.push(created);
      return { body: { result: applied(created), affected: [] } };
    }
    case "ListHistory": {
      const earlier = state.history.get(input.id);
      if (earlier) return { body: { versions: earlier } };
      const current = state.tasks.find((t) => t.entityId === input.id);
      return { body: { versions: current ? [current] : [] } };
    }
    default:
      return undefined;
  }
}
