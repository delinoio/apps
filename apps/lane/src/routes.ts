// SPDX-License-Identifier: Apache-2.0
import { LanePage, MiniApp } from "./navigation.ts";
import { allTasks, TabKind, type TabTarget } from "./tabs.ts";

const pages: Record<string, LanePage> = {
  tasks: LanePage.Tasks,
  proposals: LanePage.Proposals,
  repositories: LanePage.Repositories,
  events: LanePage.Events,
  settings: LanePage.Settings,
};
const idPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

export function routeTarget(path: string): TabTarget | undefined {
  if (path === "/") return allTasks();
  const parts = path.replace(/\/$/, "").split("/").slice(1);
  if (parts.length === 1) {
    const page = Object.hasOwn(pages, parts[0]) ? pages[parts[0]] : undefined;
    if (page)
      return {
        kind: TabKind.Page,
        page,
        label: page === LanePage.Tasks ? "All tasks" : page,
      };
    if (parts[0] === "new-tab")
      return { kind: TabKind.NewTab, label: "New tab" };
    const app =
      parts[0] === "pace"
        ? MiniApp.Pace
        : parts[0] === "realqa"
          ? MiniApp.RealQA
          : undefined;
    if (app) return { kind: TabKind.App, app, label: app };
  }
  if (parts.length === 2 && idPattern.test(parts[1])) {
    if (parts[0] === "tasks")
      return { kind: TabKind.Task, taskId: parts[1], label: "Task" };
    if (parts[0] === "repositories")
      return {
        kind: TabKind.Repository,
        repositoryId: parts[1],
        label: "Repository",
      };
  }
  // Draft IDs identify a tab in this window; refreshing never restores its input.
  if (
    parts[0] === "tasks" &&
    parts[1] === "new" &&
    parts.length === 3 &&
    idPattern.test(parts[2])
  )
    return { kind: TabKind.NewTask, taskId: parts[2], label: "New task" };
  return undefined;
}

export function targetPath(target: TabTarget): string {
  switch (target.kind) {
    case TabKind.Task:
      return `/tasks/${target.taskId}`;
    case TabKind.NewTask:
      return `/tasks/new/${target.taskId}`;
    case TabKind.Repository:
      return `/repositories/${target.repositoryId}`;
    case TabKind.NewTab:
      return "/new-tab";
    case TabKind.App:
      return target.app === MiniApp.Pace ? "/pace" : "/realqa";
    case TabKind.Page:
      return `/${Object.keys(pages).find((key) => pages[key] === target.page) ?? "tasks"}`;
  }
}
