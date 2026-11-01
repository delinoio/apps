// SPDX-License-Identifier: Apache-2.0
import { fromBinary, toBinary } from "@bufbuild/protobuf";
import { base64Decode, base64Encode } from "@bufbuild/protobuf/wire";
import {
  SharingState,
  WidgetDataSchema,
  type Dashboard,
  type WidgetData,
} from "@repo/protos/anydash/v1/anydash_pb";
import { hostCall, HostMethod } from "../host";
import { logError } from "../log";

/**
 * Persisted cache of the latest widget data on native hosts, keyed by user and
 * widget. The web host keeps no persisted copy and answers reads with nothing.
 */
export const widgetKey = (userKey: string, widgetId: string) =>
  `anydash.v1/${userKey}/widget/${widgetId}`;
export const userPrefix = (userKey: string) => `anydash.v1/${userKey}/`;

export async function readCachedWidgets(
  userKey: string,
  widgetIds: string[],
): Promise<Map<string, WidgetData>> {
  const found = new Map<string, WidgetData>();
  if (!userKey) return found;
  await Promise.all(
    widgetIds.map(async (id) => {
      try {
        const reply = await hostCall<{ value?: string }>(HostMethod.CacheRead, {
          key: widgetKey(userKey, id),
        });
        if (!reply?.value) return;
        const data = fromBinary(WidgetDataSchema, base64Decode(reply.value));
        // Never resurrect data that was cached before sharing ended.
        if (data.sharingState !== SharingState.ENDED) found.set(id, data);
      } catch {
        /* A missing or unreadable entry behaves as an empty cache. */
      }
    }),
  );
  return found;
}

export async function writeCachedWidgets(userKey: string, data: WidgetData[]) {
  if (!userKey) return;
  await Promise.all(
    data
      .filter((entry) => entry.sharingState !== SharingState.ENDED)
      .map((entry) =>
        hostCall(HostMethod.CacheWrite, {
          key: widgetKey(userKey, entry.widgetId),
          value: base64Encode(toBinary(WidgetDataSchema, entry)),
        }).catch(() => logError("anydash_cache_write_failed")),
      ),
  );
}

export async function purgeWidgets(userKey: string, widgetIds: string[]) {
  if (!userKey || widgetIds.length === 0) return;
  await Promise.all(
    widgetIds.map((id) =>
      hostCall(HostMethod.CacheRemove, {
        prefix: widgetKey(userKey, id),
      }).catch(() => logError("anydash_cache_purge_failed")),
    ),
  );
}

export async function purgeUser(userKey: string) {
  if (!userKey) return;
  await hostCall(HostMethod.CacheRemove, { prefix: userPrefix(userKey) }).catch(
    () => logError("anydash_cache_purge_failed"),
  );
}

/** Widgets whose sharing ended in either the dashboard or its data. */
export function endedWidgetIds(
  dashboard: Dashboard | undefined,
  data: WidgetData[] | undefined,
) {
  const ended = new Set<string>();
  for (const widget of dashboard?.widgets ?? [])
    if (widget.sharingState === SharingState.ENDED) ended.add(widget.id);
  for (const entry of data ?? [])
    if (entry.sharingState === SharingState.ENDED) ended.add(entry.widgetId);
  return [...ended];
}
