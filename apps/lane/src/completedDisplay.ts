// SPDX-License-Identifier: Apache-2.0
import { CompletedSelection } from "@repo/protos/lane/v1/lane_pb";

export enum TaskLayout {
  List = "list",
  Board = "board",
}

export const completedOptions = [
  [CompletedSelection.NONE, "None"],
  [CompletedSelection.LAST_24_HOURS, "Last 24 hours"],
  [CompletedSelection.LAST_7_DAYS, "Last 7 days"],
  [CompletedSelection.LAST_30_DAYS, "Last 30 days"],
  [CompletedSelection.ALL, "All"],
] as const;

const dayMicros = 24n * 60n * 60n * 1_000_000n;

export function completedFilter(
  selection: CompletedSelection,
  nowMicros: bigint,
) {
  const duration =
    selection === CompletedSelection.LAST_24_HOURS
      ? dayMicros
      : selection === CompletedSelection.LAST_7_DAYS
        ? 7n * dayMicros
        : selection === CompletedSelection.LAST_30_DAYS
          ? 30n * dayMicros
          : undefined;
  if (duration === undefined) return { selection };
  const cutoff = nowMicros - duration;
  const seconds = cutoff / 1_000_000n;
  const micros = cutoff % 1_000_000n;
  return {
    selection,
    cutoff: { seconds: seconds.toString(), nanos: Number(micros * 1_000n) },
  };
}

export function completedNowMicros() {
  return BigInt(Date.now()) * 1_000n;
}
