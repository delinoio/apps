// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
import {
  EntityKind,
  LaneService,
  PlanningService,
  type EntityResult as ProtoEntityResult,
} from "@delino/protos/lane/v1/lane_pb";
import type { UI } from "./serializable";
import type { LaneAdapter } from "./adapter";
import {
  describeError,
  planningItems,
  type PlanningItem,
  type TaskRef,
} from "./planning";
import { taskOf } from "./model";

type EntityResult = UI<ProtoEntityResult>;
export type LoadStatus = "loading" | "ready" | "error";

export interface PlanningState {
  items: PlanningItem[];
  status: LoadStatus;
  error: string;
  retry: () => void;
}
/**
 * Loads every repository's labels, milestones and cycles; reloads on store changes.
 * `space` identifies the active data space: cached items are valid only for the space
 * that loaded them, so a switch never exposes another space's names, even after a
 * failed reload. A refresh within the same space keeps the previous items meanwhile.
 */
export function usePlanning(
  adapter: LaneAdapter | undefined,
  revision: number,
  space: string,
): PlanningState {
  const [state, setState] = useState<{
    space: string;
    items: PlanningItem[];
    status: LoadStatus;
    error: string;
  }>({ space, items: [], status: "loading", error: "" });
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!adapter) return;
    let active = true;
    setState((s) =>
      s.space === space
        ? { ...s, status: s.items.length ? s.status : "loading" }
        : { space, items: [], status: "loading", error: "" },
    );
    void (async () => {
      try {
        const results: EntityResult[] = [];
        let cursor = "";
        do {
          const r = await adapter.call(
            PlanningService.method.listPlanningItems,
            { page: { size: 100, cursor }, kind: EntityKind.UNSPECIFIED },
          );
          results.push(...r.items);
          cursor = r.nextCursor;
        } while (cursor);
        if (active)
          setState({
            space,
            items: planningItems(results),
            status: "ready",
            error: "",
          });
      } catch (e) {
        if (active)
          setState((s) => ({
            space,
            items: s.space === space ? s.items : [],
            status: "error",
            error: describeError(e),
          }));
      }
    })();
    return () => {
      active = false;
    };
  }, [adapter, revision, attempt, space]);
  // Never expose items cached for a previous data space while the new one loads.
  const current = state.space === space;
  return {
    items: current ? state.items : [],
    status: current ? state.status : "loading",
    error: current ? state.error : "",
    retry: () => setAttempt((v) => v + 1),
  };
}

export interface TaskOptions {
  tasks: TaskRef[];
  status: LoadStatus;
  error: string;
  retry: () => void;
  /** Reloads and returns the fresh tasks; used right after a save. */
  reload: () => Promise<TaskRef[]>;
}
async function loadRepositoryTasks(
  adapter: LaneAdapter,
  repositoryId: string,
): Promise<TaskRef[]> {
  const tasks: TaskRef[] = [];
  let cursor = "";
  do {
    const r = await adapter.call(LaneService.method.listTasks, {
      page: { size: 100, cursor },
      repositoryId,
    });
    for (const item of r.items) {
      const task = taskOf(item.version);
      if (task && item.version && !item.version.deleted)
        tasks.push({ id: item.version.entityId, title: task.title, task });
    }
    cursor = r.nextCursor;
  } while (cursor);
  return tasks;
}
/** Same-repository tasks: relationship search options and derived Subtasks/Blocks. */
export function useTaskOptions(
  adapter: LaneAdapter,
  repositoryId: string,
  revision: number,
): TaskOptions {
  const [state, setState] = useState<{
    repositoryId: string;
    tasks: TaskRef[];
    status: LoadStatus;
    error: string;
  }>({ repositoryId, tasks: [], status: "loading", error: "" });
  const [attempt, setAttempt] = useState(0);
  const load = async () => {
    const tasks = await loadRepositoryTasks(adapter, repositoryId);
    setState({ repositoryId, tasks, status: "ready", error: "" });
    return tasks;
  };
  useEffect(() => {
    if (!repositoryId) {
      setState({ repositoryId, tasks: [], status: "ready", error: "" });
      return;
    }
    let active = true;
    setState((s) =>
      s.repositoryId === repositoryId
        ? s
        : { repositoryId, tasks: [], status: "loading", error: "" },
    );
    void loadRepositoryTasks(adapter, repositoryId).then(
      (tasks) => {
        if (active)
          setState({ repositoryId, tasks, status: "ready", error: "" });
      },
      (e) => {
        if (active)
          setState((s) => ({
            ...s,
            repositoryId,
            status: "error",
            error: describeError(e),
          }));
      },
    );
    return () => {
      active = false;
    };
  }, [adapter, repositoryId, revision, attempt]);
  // Never expose tasks of a previously selected repository as valid options.
  const current = state.repositoryId === repositoryId;
  return {
    tasks: current ? state.tasks : [],
    status: !repositoryId ? "ready" : current ? state.status : "loading",
    error: current ? state.error : "",
    retry: () => setAttempt((v) => v + 1),
    reload: repositoryId ? load : async () => [],
  };
}
