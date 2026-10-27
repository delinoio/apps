// SPDX-License-Identifier: Apache-2.0
import type { UI } from "./serializable";
import type { EntityResult } from "@repo/protos/lane/v1/lane_pb";
import type { MiniApp } from "./AppNavigation";
import { LanePage } from "./navigation.ts";

export enum TabKind {
  App = "app",
  NewTab = "new-tab",
  Page = "page",
  Repository = "repository",
  Task = "task",
  NewTask = "new-task",
}
export interface TabTarget {
  kind: TabKind;
  label: string;
  app?: MiniApp;
  page?: LanePage;
  repositoryId?: string;
  taskId?: string;
}
export interface WorkspaceTab {
  id: string;
  target: TabTarget;
}
export interface WorkspaceReport {
  id: string;
  title: string;
  dirty: boolean;
  busy: boolean;
  exitBusy: boolean;
  repositories: UI<EntityResult>[];
  repositoryStatus: string;
}
export const allTasks = (): TabTarget => ({
  kind: TabKind.Page,
  page: LanePage.Tasks,
  label: "All tasks",
});
export function tabIdentity(target: TabTarget): string {
  if (target.kind === TabKind.App) return `app-${target.app}`;
  if (target.kind === TabKind.NewTab) return "new-tab";
  // A saved new task keeps its mounted editor and becomes the same destination
  // as any later attempt to open that task through a list, relationship or link.
  if (target.kind === TabKind.Task || target.kind === TabKind.NewTask)
    return `task-${target.taskId}`;
  if (target.kind === TabKind.Repository)
    return `repository-${target.repositoryId}`;
  return `page-${target.page}`;
}
export function adjacentTab(
  tabs: WorkspaceTab[],
  closed: string,
): string | undefined {
  const index = tabs.findIndex((tab) => tab.id === closed);
  return tabs[index + 1]?.id ?? tabs[index - 1]?.id;
}
