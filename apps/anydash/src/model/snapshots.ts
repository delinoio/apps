// SPDX-License-Identifier: Apache-2.0
import { create, toBinary } from "@bufbuild/protobuf";
import { base64Encode } from "@bufbuild/protobuf/wire";
import { timestampNow } from "@bufbuild/protobuf/wkt";
import { createClient, type Transport } from "@connectrpc/connect";
import {
  ListNativeWidgetSnapshotsResponseSchema,
  LocalCollectorService,
  NativeWidgetSnapshotSchema,
  SharingState,
  type Dashboard,
  type NativeWidgetSnapshot,
  type WidgetData,
} from "@repo/protos/anydash/v1/anydash_pb";
import { hostCall, HostMethod, HostPlatform } from "../host";
import { logError } from "../log";

/** PutNativeWidgetSnapshots accepts at most 100 snapshots per request. */
export const MAX_SNAPSHOTS = 100;

export function buildSnapshots(
  dashboard: Dashboard,
  data: WidgetData[],
): NativeWidgetSnapshot[] {
  const refreshedAt = timestampNow();
  const byId = new Map(data.map((entry) => [entry.widgetId, entry]));
  return dashboard.widgets.slice(0, MAX_SNAPSHOTS).map((widget) => {
    const entry = byId.get(widget.id);
    const ended =
      widget.sharingState === SharingState.ENDED ||
      entry?.sharingState === SharingState.ENDED;
    return create(NativeWidgetSnapshotSchema, {
      dashboardId: dashboard.id,
      widgetId: widget.id,
      spaceId: dashboard.spaceId,
      sharingState: ended
        ? SharingState.ENDED
        : (entry?.sharingState ?? widget.sharingState),
      // ENDED snapshots carry no summary; hosts then delete displayed data.
      summary: ended ? undefined : entry?.summary,
      refreshedAt,
      notices: ended ? [] : (entry?.notices ?? []),
    });
  });
}

/**
 * Desktop writes snapshots to the collector, which serves the macOS and
 * Windows widget hosts. Mobile hosts store them for their widget extensions.
 * Snapshots merge by widget; ENDED replaces previously shown data.
 */
export async function publishSnapshots(
  platform: HostPlatform,
  snapshots: NativeWidgetSnapshot[],
  collector?: Transport,
) {
  if (snapshots.length === 0) return;
  try {
    if (platform === HostPlatform.Desktop && collector) {
      await createClient(
        LocalCollectorService,
        collector,
      ).putNativeWidgetSnapshots({
        snapshots,
        replaceAll: false,
      });
    } else if (
      platform === HostPlatform.Ios ||
      platform === HostPlatform.Android
    ) {
      await hostCall(HostMethod.WidgetSnapshots, {
        snapshots: snapshots.map((snapshot) =>
          base64Encode(toBinary(NativeWidgetSnapshotSchema, snapshot)),
        ),
        replaceAll: false,
      });
    }
  } catch {
    logError("anydash_widget_snapshots_failed", { platform });
  }
}

/** Removes every native widget snapshot, for example after sign-out. */
export async function clearSnapshots(
  platform: HostPlatform,
  collector?: Transport,
) {
  try {
    if (platform === HostPlatform.Desktop && collector)
      await createClient(
        LocalCollectorService,
        collector,
      ).putNativeWidgetSnapshots({
        snapshots: [],
        replaceAll: true,
      });
    else if (platform === HostPlatform.Ios || platform === HostPlatform.Android)
      await hostCall(HostMethod.WidgetSnapshots, {
        snapshots: [],
        replaceAll: true,
      });
  } catch {
    logError("anydash_widget_snapshots_failed", { platform });
  }
}

/** Fixture encoder shared with native widget decoder tests. */
export const encodeSnapshotList = (snapshots: NativeWidgetSnapshot[]) =>
  toBinary(
    ListNativeWidgetSnapshotsResponseSchema,
    create(ListNativeWidgetSnapshotsResponseSchema, { snapshots }),
  );
