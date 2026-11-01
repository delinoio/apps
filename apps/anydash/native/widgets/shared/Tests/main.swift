// SPDX-License-Identifier: Apache-2.0
// Decoder checks against fixtures from the TypeScript bindings.
import Foundation

func check(_ condition: Bool, _ message: String) {
  if !condition {
    FileHandle.standardError.write("FAIL: \(message)\n".data(using: .utf8)!)
    exit(1)
  }
}

let fixtures = URL(fileURLWithPath: CommandLine.arguments[1])
let list = try decodeSnapshotList(Array(try Data(contentsOf: fixtures.appendingPathComponent("snapshots.pb"))))
check(list.count == 3, "three snapshots")
let codex = list[0]
check(codex.summary?.title == "Codex · 5-hour", "title")
check(codex.summary?.primaryValue.content == .percent(62), "percent")
check(WidgetText.value(codex.summary!.primaryValue) == "62%", "62%")
var utc = Calendar(identifier: .gregorian)
utc.timeZone = TimeZone(identifier: "UTC")!
check(codex.deepLink?.absoluteString == "anydash://spaces/0192f000-0000-7000-8000-0000000000a1/dashboards/0192f000-0000-7000-8000-0000000000c1?widget=0192f4aa-0000-7000-8000-000000000001", "deep link")
check(WidgetText.supporting(codex.summary!).hasPrefix("left · Resets "), "supporting")
let ended = list[1]
check(ended.sharingState == .ended && ended.summary == nil, "ended has no data")
check(WidgetText.accessibility(ended, title: "Supabase · Requests").contains("Sharing ended"), "ended label")
check(WidgetText.value(list[2].summary!.primaryValue) == "$41.80", "money")
let data = try decodeWidgetDataSummary(Array(try Data(contentsOf: fixtures.appendingPathComponent("widget-data.pb"))))
check(data.summary?.primaryValue.content == .percent(61), "widget data summary")
let request = encodeQueryWidgetData(dashboardId: "d", widgetId: "w", timeZone: "")
check(request == [10, 1, 100, 18, 1, 119], "request encoding")
print("anydash widget decoder: ok")
