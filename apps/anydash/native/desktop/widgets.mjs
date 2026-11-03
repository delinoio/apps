// SPDX-License-Identifier: Apache-2.0
// Resident native widget refresh. The dashboard screen publishes snapshots
// after each data refresh, but it exists only while a window is open. While
// no window is open, the main process refreshes the dashboards of the stored
// snapshots itself, so the macOS and Windows widget hosts, which only reread
// ListNativeWidgetSnapshots, keep current values and learn about ended
// sharing, removed widgets and deleted dashboards.
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { timestampNow } from "@bufbuild/protobuf/wkt";
import {
  ListNativeWidgetSnapshotsRequestSchema,
  ListNativeWidgetSnapshotsResponseSchema,
  NativeWidgetSnapshotSchema,
  PutNativeWidgetSnapshotsRequestSchema,
  QueryDashboardDataRequestSchema,
  QueryDashboardDataResponseSchema,
  SharingState,
} from "@repo/protos/anydash/v1/anydash_pb";

/** Background cadence, close to the refresh budget WidgetKit allows. */
export const BACKGROUND_REFRESH_MS = 15 * 60_000;

/** PutNativeWidgetSnapshots accepts at most 100 snapshots per request. */
export const MAX_SNAPSHOTS_PER_WRITE = 100;

/**
 * Splits a full replacement into permitted-size writes. Merged writes keep
 * accumulating snapshots across dashboards, so the stored set can exceed one
 * request: the first write replaces the stored set and the rest merge after
 * it, which keeps the stored order because the collector appends new widgets.
 */
export function replacementBatches(snapshots) {
  const batches = [];
  let start = 0;
  do {
    batches.push({
      snapshots: snapshots.slice(start, start + MAX_SNAPSHOTS_PER_WRITE),
      replaceAll: start === 0,
    });
    start += MAX_SNAPSHOTS_PER_WRITE;
  } while (start < snapshots.length);
  return batches;
}

/** Outcome of one dashboard query, from its Connect HTTP status. */
export const DashboardOutcome = Object.freeze({
  Data: "data",
  /** The dashboard was deleted: drop its snapshots. */
  Deleted: "deleted",
  /** The caller lost access, for example by leaving the team: end sharing. */
  AccessEnded: "access-ended",
  /** Signed out or the session expired: write nothing. */
  SignedOut: "signed-out",
  /** Temporary failure: keep the last snapshots. */
  Failed: "failed",
});

export function dashboardOutcome(status) {
  if (status === 200) return DashboardOutcome.Data;
  if (status === 404) return DashboardOutcome.Deleted;
  if (status === 403) return DashboardOutcome.AccessEnded;
  if (status === 401) return DashboardOutcome.SignedOut;
  return DashboardOutcome.Failed;
}

const ended = (snapshot, refreshedAt) =>
  create(NativeWidgetSnapshotSchema, {
    dashboardId: snapshot.dashboardId,
    widgetId: snapshot.widgetId,
    spaceId: snapshot.spaceId,
    sharingState: SharingState.ENDED,
    refreshedAt,
  });

export class WidgetRefresher {
  #timer;
  #generation = 0;
  #running;

  constructor({
    host,
    collector,
    auth,
    clientVersion,
    interval = BACKGROUND_REFRESH_MS,
    log = () => {},
  }) {
    Object.assign(this, {
      host,
      collector,
      auth,
      clientVersion,
      interval,
      log,
    });
  }

  /** Starts periodic refreshes, for example when the last window closes. */
  start() {
    if (this.#timer) return;
    this.#timer = setInterval(() => void this.refresh(), this.interval);
    this.#timer.unref?.();
    this.log({ event: "anydash_widget_refresh_started" });
  }

  /**
   * Stops refreshes when a window opens. The dashboard screen publishes
   * again, and a refresh still in flight discards its result instead of
   * overwriting the window's newer snapshots or a sign-out's removal.
   */
  stop() {
    this.#generation += 1;
    if (!this.#timer) return;
    clearInterval(this.#timer);
    this.#timer = undefined;
    this.log({ event: "anydash_widget_refresh_stopped" });
  }

  refresh() {
    this.#running ??= this.#refresh().finally(() => {
      this.#running = undefined;
    });
    return this.#running;
  }

  async #refresh() {
    const generation = this.#generation;
    const current = () => generation === this.#generation;
    try {
      if (!(await this.auth.signedIn())) return;
      const listed = await this.collector.forward(
        "ListNativeWidgetSnapshots",
        Buffer.from(
          toBinary(
            ListNativeWidgetSnapshotsRequestSchema,
            create(ListNativeWidgetSnapshotsRequestSchema),
          ),
        ),
      );
      if (listed.status !== 200) {
        this.log({
          event: "anydash_widget_refresh_failed",
          stage: "list",
          status: listed.status,
        });
        return;
      }
      const stored = fromBinary(
        ListNativeWidgetSnapshotsResponseSchema,
        listed.body,
      ).snapshots;
      if (!stored.length) return;
      const dashboards = [
        ...new Set(stored.map((snapshot) => snapshot.dashboardId)),
      ];
      const refreshedAt = timestampNow();
      const replacements = new Map();
      for (const dashboardId of dashboards) {
        const reply = await this.host.apiCall(
          "anydash.v1.WidgetDataService",
          "QueryDashboardData",
          Buffer.from(
            toBinary(
              QueryDashboardDataRequestSchema,
              create(QueryDashboardDataRequestSchema, { dashboardId }),
            ),
          ),
          {
            "x-anydash-client-kind": "CLIENT_KIND_DESKTOP",
            "x-anydash-client-version": this.clientVersion,
          },
        );
        const outcome = dashboardOutcome(reply.status);
        const own = stored.filter(
          (snapshot) => snapshot.dashboardId === dashboardId,
        );
        if (outcome === DashboardOutcome.SignedOut) return;
        if (outcome === DashboardOutcome.Failed) {
          this.log({
            event: "anydash_widget_refresh_failed",
            stage: "query",
            status: reply.status,
            correlationId: reply.correlationId,
          });
          continue;
        }
        if (outcome === DashboardOutcome.Deleted) {
          for (const snapshot of own) replacements.set(snapshot.widgetId, null);
          continue;
        }
        if (outcome === DashboardOutcome.AccessEnded) {
          for (const snapshot of own)
            replacements.set(snapshot.widgetId, ended(snapshot, refreshedAt));
          continue;
        }
        const data = new Map(
          fromBinary(QueryDashboardDataResponseSchema, reply.body).widgets.map(
            (entry) => [entry.widgetId, entry],
          ),
        );
        for (const snapshot of own) {
          const entry = data.get(snapshot.widgetId);
          // The response covers every widget on the dashboard, so a missing
          // widget was removed.
          if (!entry) replacements.set(snapshot.widgetId, null);
          else if (entry.sharingState === SharingState.ENDED)
            replacements.set(snapshot.widgetId, ended(snapshot, refreshedAt));
          else
            replacements.set(
              snapshot.widgetId,
              create(NativeWidgetSnapshotSchema, {
                dashboardId: snapshot.dashboardId,
                widgetId: snapshot.widgetId,
                spaceId: snapshot.spaceId,
                sharingState: entry.sharingState,
                summary: entry.summary,
                refreshedAt,
                notices: entry.notices,
              }),
            );
        }
      }
      if (!replacements.size) return;
      const next = stored
        .map((snapshot) =>
          replacements.has(snapshot.widgetId)
            ? replacements.get(snapshot.widgetId)
            : snapshot,
        )
        .filter(Boolean);
      // A window opened or the user signed out while the requests ran.
      if (!current() || !(await this.auth.signedIn()) || !current()) return;
      // Once started, the batches all run: stopping between them would drop
      // the snapshots of the remaining batches.
      let written;
      for (const batch of replacementBatches(next)) {
        written = await this.collector.forward(
          "PutNativeWidgetSnapshots",
          Buffer.from(
            toBinary(
              PutNativeWidgetSnapshotsRequestSchema,
              create(PutNativeWidgetSnapshotsRequestSchema, batch),
            ),
          ),
        );
        if (written.status !== 200) break;
      }
      this.log({
        event:
          written.status === 200
            ? "anydash_widget_refresh_written"
            : "anydash_widget_refresh_failed",
        stage: "write",
        status: written.status,
        snapshots: next.length,
        removed: stored.length - next.length,
      });
    } catch (error) {
      this.log({
        event: "anydash_widget_refresh_failed",
        stage: "refresh",
        code: error?.code ?? "UNKNOWN",
      });
    }
  }
}
