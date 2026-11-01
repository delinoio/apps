// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "@rstest/core";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { SharingState } from "@repo/protos/anydash/v1/anydash_pb";
import { Copy } from "../src/copy";
import {
  BadgeTone,
  duplicateDeviceCount,
  LineStyle,
  widgetView,
  WidgetState,
  type ViewContext,
} from "../src/model/widget-view";
import * as fixtures from "./fixtures/scenarios";

const widgets = fixtures.makeWidgets();
const data = fixtures.makeWidgetData(widgets);
const ctx = (extra: Partial<ViewContext> = {}): ViewContext => ({
  timeZone: "Asia/Seoul",
  now: fixtures.NOW,
  devices: fixtures.devices(),
  connections: fixtures.connections(),
  ...extra,
});
const view = (index: number, extra: Partial<ViewContext> = {}) =>
  widgetView(
    widgets[index],
    data.find((entry) => entry.widgetId === widgets[index].id),
    ctx(extra),
  );

describe("metric states", () => {
  it("shows subscription remaining as 100 − used with the reset time", () => {
    const result = view(0);
    expect(result.state).toBe(WidgetState.Value);
    expect(result.primary?.text).toBe("62% left");
    expect(result.usedPercent).toBe(38);
    expect(result.lines[0].text).toBe("38% used · Resets 14:20");
    expect(result.badges).toEqual([]);
  });

  it("renders Not provided as muted text without a badge, distinct from failure", () => {
    const notProvided = view(1);
    expect(notProvided.state).toBe(WidgetState.NotProvided);
    expect(notProvided.primary).toEqual({
      text: Copy.notProvided,
      muted: true,
    });
    expect(notProvided.badges).toEqual([]);
    expect(notProvided.lines[0].text).toBe("Codex doesn’t report this value.");
    const failed = view(2);
    expect(failed.primary?.muted).toBeFalsy();
    expect(failed.badges[0].tone).toBe(BadgeTone.Danger);
  });

  it("keeps the stale last value with a red Couldn’t refresh badge on failure", () => {
    const result = view(2);
    expect(result.state).toBe(WidgetState.Failed);
    expect(result.primary).toEqual({ text: "71% left", stale: true });
    expect(result.badges).toEqual([
      { tone: BadgeTone.Danger, label: "Couldn’t refresh" },
    ]);
    expect(result.lines[0].text).toBe("Last updated 11:52");
  });

  it("adds the retry time when the collector reports the next attempt", () => {
    const result = view(2, {
      nextAttempt: new Map([
        [
          fixtures.ids.claude,
          timestampFromDate(new Date(fixtures.NOW.getTime() + 4 * 60_000)),
        ],
      ]),
    });
    expect(result.lines[0].text).toBe("Last updated 11:52 · Retrying in 4 min");
  });

  it("explains CLI re-login for expired local authentication", () => {
    const result = view(3);
    expect(result.state).toBe(WidgetState.AuthExpired);
    expect(result.badges).toEqual([
      { tone: BadgeTone.Warning, label: "Authentication expired" },
    ]);
    expect(result.lines[0].text).toBe(
      "Sign in to the grok CLI again on studio-mac. AnyDash resumes automatically.",
    );
    expect(result.lines[1]).toEqual({
      text: "Last value 54% left · Oct 31, 18:05",
      style: LineStyle.Stale,
    });
  });

  it("shows partial aggregation with skipped records and the estimated cost", () => {
    const result = view(4);
    expect(result.state).toBe(WidgetState.Partial);
    expect(result.primary?.text).toBe("48.2M tokens");
    expect(result.lines.map((line) => line.text)).toEqual([
      "Est. cost $214.30 (USD, estimated)",
      "Update the collector on build-box to read them.",
    ]);
    expect(result.badges).toEqual([
      {
        tone: BadgeTone.Warning,
        label: "Partial aggregation · 12 records skipped",
      },
    ]);
  });

  it("shows Can’t be calculated with the model that has no price", () => {
    const result = view(5);
    expect(result.state).toBe(WidgetState.CannotBeCalculated);
    expect(result.primary).toEqual({
      text: "Can’t be calculated",
      muted: true,
    });
    expect(result.lines[0].text).toBe(
      "No public price for gpt-6-luna at that time. Token counts are still shown.",
    );
  });

  it("labels provider costs in their original currency", () => {
    const result = view(6);
    expect(result.primary?.text).toBe("$41.80");
    expect(result.lines[0].text).toBe("USD, billed cost from Vercel · Hobby");
  });

  it("shows Sharing ended with no cached value", () => {
    const result = view(7);
    expect(result.state).toBe(WidgetState.SharingEnded);
    expect(result.primary).toBeUndefined();
    expect(result.rows).toEqual([]);
    expect(result.badges).toEqual([
      { tone: BadgeTone.Neutral, label: "Sharing ended" },
    ]);
    expect(result.lines[0].text).toBe(Copy.sharingEndedDetail);
    expect(result.title).toBe(
      "Shared by a team member · Supabase API requests",
    );
  });

  it("treats data reported as ENDED like an ended widget", () => {
    const widget = { ...widgets[0], sharingState: SharingState.SHARED };
    const entry = { ...data[0], sharingState: SharingState.ENDED };
    expect(widgetView(widget, entry, ctx()).state).toBe(
      WidgetState.SharingEnded,
    );
  });

  it("renders a loading widget until its data arrives", () => {
    expect(view(8).state).toBe(WidgetState.Loading);
  });

  it("reads the state label, then the value, then the update time", () => {
    expect(view(2).accessibilityLabel).toBe(
      "Claude Code · Weekly limit. Couldn’t refresh. 71% left. Last updated 11:52. Updated 11:52",
    );
    expect(
      view(0).accessibilityLabel.startsWith("Codex · 5-hour limit. 62% left."),
    ).toBe(true);
  });

  it("detects the duplicate-device notice", () => {
    expect(duplicateDeviceCount(data, fixtures.devices())).toBe(3);
    expect(
      duplicateDeviceCount(data.slice(0, 2), fixtures.devices()),
    ).toBeUndefined();
  });

  it("shows ready deployment resources", () => {
    const resource = fixtures.resourceData();
    const result = widgetView(
      {
        ...widgets[0],
        id: resource.widgetId,
        type: resource.type,
        layout: { ...widgets[0].layout!, title: "Vercel" },
      } as never,
      resource,
      ctx(),
    );
    expect(result.badges).toEqual([
      { tone: BadgeTone.Success, label: "2 projects ready" },
    ]);
  });
});
