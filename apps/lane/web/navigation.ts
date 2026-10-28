// SPDX-License-Identifier: Apache-2.0
import { routeTarget, targetPath } from "../src/routes.ts";
import type { TabTarget } from "../src/tabs.ts";

interface BrowserPort {
  location: { pathname: string };
  history: Pick<History, "state" | "pushState" | "replaceState" | "go">;
}

// The shared app acknowledges navigation after its unsaved-change guard settles.
// History changes never reload the document or replace the mounted Lynx view.
export class RouteHistory {
  private index = 0;
  private path = "/tasks";
  private sequence = 0;
  private pending?: { requestId: number; index: number; path: string };
  private restoring = false;
  private started = false;
  constructor(
    private browser: BrowserPort,
    private navigate: (target: TabTarget, requestId: number) => void,
  ) {}

  start(path = this.browser.location.pathname) {
    const target = routeTarget(path);
    const existingIndex = this.browser.history.state?.laneIndex;
    this.index = Number.isSafeInteger(existingIndex) ? existingIndex : 0;
    this.started = true;
    this.browser.history.replaceState(
      { laneIndex: this.index },
      "",
      this.browser.location.pathname,
    );
    this.request(target ?? routeTarget("/tasks")!, this.index);
    return !!target;
  }

  private request(target: TabTarget, index: number) {
    const requestId = ++this.sequence;
    this.pending = { requestId, index, path: targetPath(target) };
    this.navigate(target, requestId);
  }

  pop() {
    if (!this.started) return;
    if (this.restoring) {
      this.restoring = false;
      return;
    }
    const target =
      routeTarget(this.browser.location.pathname) ?? routeTarget("/tasks")!;
    this.request(target, this.browser.history.state?.laneIndex ?? 0);
  }

  report(path: string, requestId?: number) {
    if (!this.started || !routeTarget(path) || this.restoring) return;
    if (requestId !== undefined && requestId !== this.sequence) return;
    if (this.pending) {
      if (requestId !== this.pending.requestId) return;
      const pending = this.pending;
      this.pending = undefined;
      if (path !== pending.path && pending.index !== this.index) {
        this.restoring = true;
        this.browser.history.go(this.index - pending.index);
        return;
      }
      this.index = pending.index;
      this.path = path;
      this.browser.history.replaceState({ laneIndex: this.index }, "", path);
      return;
    }
    if (path === this.path) return;
    if (
      this.path.startsWith("/tasks/new/") &&
      path === `/tasks/${this.path.slice("/tasks/new/".length)}`
    ) {
      this.path = path;
      this.browser.history.replaceState({ laneIndex: this.index }, "", path);
      return;
    }
    this.path = path;
    this.browser.history.pushState({ laneIndex: ++this.index }, "", path);
  }
}
