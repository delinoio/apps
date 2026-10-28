// SPDX-License-Identifier: Apache-2.0
import type { UI } from "./serializable";
import type { EntityResult } from "@repo/protos/lane/v1/lane_pb";
import type { MiniApp } from "./navigation";
import { LanePage } from "./navigation.ts";
import { TabCommand } from "./tab-commands.ts";
export { TabCommand } from "./tab-commands.ts";

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
  savedTaskId?: string;
}
export interface TabCommandRequest {
  command: TabCommand;
  // SelectIndex uses zero-based displayed order. Row moves require tabId.
  index?: number;
  tabId?: string;
}
export function tabCommandRequest(
  value: unknown,
): TabCommandRequest | undefined {
  if (!value || typeof value !== "object") return undefined;
  const request = value as Record<string, unknown>;
  if (!Object.values(TabCommand).includes(request.command as TabCommand))
    return undefined;
  if (
    request.command === TabCommand.SelectIndex &&
    (!Number.isInteger(request.index) ||
      (request.index as number) < 0 ||
      (request.index as number) > 7)
  )
    return undefined;
  if (request.tabId !== undefined && typeof request.tabId !== "string")
    return undefined;
  return request as unknown as TabCommandRequest;
}
export interface ClosedTab {
  target: TabTarget;
  position: number;
  label: string;
}
export const CLOSED_TAB_LIMIT = 20;
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
export function tabAtOffset(
  tabs: WorkspaceTab[],
  selected: string,
  offset: number,
): string | undefined {
  if (!tabs.length) return undefined;
  const index = tabs.findIndex((tab) => tab.id === selected);
  if (index < 0) return undefined;
  return tabs[(index + offset + tabs.length) % tabs.length]?.id;
}
export function tabAtIndex(
  tabs: WorkspaceTab[],
  index: number,
): string | undefined {
  return Number.isInteger(index) && index >= 0 ? tabs[index]?.id : undefined;
}
export function moveTab(
  tabs: WorkspaceTab[],
  id: string,
  destination: number,
): WorkspaceTab[] {
  const from = tabs.findIndex((tab) => tab.id === id);
  if (from < 0 || !Number.isInteger(destination)) return tabs;
  const to = Math.max(0, Math.min(destination, tabs.length - 1));
  if (from === to) return tabs;
  const next = [...tabs];
  const [tab] = next.splice(from, 1);
  next.splice(to, 0, tab);
  return next;
}
export function closedTab(
  tab: WorkspaceTab,
  position: number,
  savedTaskId?: string,
): ClosedTab {
  const { kind, label, app, page, repositoryId, taskId } = tab.target;
  const target: TabTarget =
    kind === TabKind.NewTask && savedTaskId
      ? { kind: TabKind.Task, label: "Task", taskId: savedTaskId, repositoryId }
      : kind === TabKind.NewTask
        ? { kind, label: "New task", repositoryId }
        : { kind, label, app, page, repositoryId, taskId };
  return { target, position, label: target.label };
}
export function recordClosedTab(
  history: ClosedTab[],
  entry: ClosedTab,
): ClosedTab[] {
  return [...history, entry].slice(-CLOSED_TAB_LIMIT);
}
export function restoredTarget(
  entry: ClosedTab,
  freshTaskId: string,
): TabTarget {
  return entry.target.kind === TabKind.NewTask
    ? { ...entry.target, taskId: freshTaskId }
    : { ...entry.target };
}
export function insertRestoredTab(
  tabs: WorkspaceTab[],
  entry: ClosedTab,
  target: TabTarget,
): { tabs: WorkspaceTab[]; selected: string } {
  const id = tabIdentity(target);
  if (tabs.some((tab) => tab.id === id)) return { tabs, selected: id };
  const next = [...tabs];
  next.splice(Math.max(0, Math.min(entry.position, next.length)), 0, {
    id,
    target,
  });
  return { tabs: next, selected: id };
}
