// SPDX-License-Identifier: Apache-2.0
// macOS desktop and Notification Center widgets. Lynxtron cannot embed a
// WidgetKit extension, so this separate host reads the snapshots that the
// resident desktop app writes to the local collector after each refresh
// (ListNativeWidgetSnapshots). Without a running collector it keeps showing
// the last snapshot it read, with that snapshot's time.
import AppIntents
import SwiftUI
import WidgetKit

enum SnapshotSource {
  static let defaults = UserDefaults.standard

  /// Reads the collector and remembers the result for offline timelines.
  static func load() -> [NativeWidgetSnapshot] {
    do {
      let snapshots = try CollectorClient.listSnapshots()
      // Persist only the encoded list returned by the collector; ENDED entries
      // carry no data, so removed data is not retained here either.
      defaults.set(snapshots.map(\.widgetId), forKey: "widgetIds")
      cache = snapshots
      for snapshot in snapshots { if let title = snapshot.summary?.title { defaults.set(title, forKey: "title.\(snapshot.widgetId)") } }
      return snapshots
    } catch {
      NSLog("{\"event\":\"anydash_widget_collector_unavailable\",\"platform\":\"macos\"}")
      return cache
    }
  }

  /// Last collector result in this extension process.
  nonisolated(unsafe) static var cache: [NativeWidgetSnapshot] = []

  static func snapshot(widgetId: String?) -> NativeWidgetSnapshot? {
    let snapshots = load().sorted { ($0.refreshedAt ?? .distantPast) > ($1.refreshedAt ?? .distantPast) }
    guard let widgetId else { return snapshots.first }
    return snapshots.first { $0.widgetId == widgetId }
  }

  static func title(for widgetId: String?) -> String {
    widgetId.flatMap { defaults.string(forKey: "title.\($0)") } ?? "AnyDash"
  }
}

func macEntry(_ snapshot: NativeWidgetSnapshot?) -> AnyDashEntry {
  AnyDashEntry(date: Date(), snapshot: snapshot, title: snapshot?.summary?.title ?? SnapshotSource.title(for: snapshot?.widgetId))
}

struct WidgetChoice: AppEntity {
  static var typeDisplayRepresentation: TypeDisplayRepresentation = "Widget"
  static var defaultQuery = WidgetChoiceQuery()
  var id: String
  var title: String
  var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(title)") }
}

struct WidgetChoiceQuery: EntityQuery {
  func entities(for identifiers: [String]) async throws -> [WidgetChoice] {
    try await suggestedEntities().filter { identifiers.contains($0.id) }
  }
  func suggestedEntities() async throws -> [WidgetChoice] {
    SnapshotSource.load().map { WidgetChoice(id: $0.widgetId, title: $0.summary?.title ?? SnapshotSource.title(for: $0.widgetId)) }
  }
}

struct SelectWidgetIntent: WidgetConfigurationIntent {
  static var title: LocalizedStringResource = "Choose a widget"
  @Parameter(title: "Widget") var widget: WidgetChoice?
}

struct CollectorProvider: AppIntentTimelineProvider {
  func placeholder(in context: Context) -> AnyDashEntry { macEntry(nil) }
  func snapshot(for configuration: SelectWidgetIntent, in context: Context) async -> AnyDashEntry {
    macEntry(SnapshotSource.snapshot(widgetId: configuration.widget?.id))
  }
  func timeline(for configuration: SelectWidgetIntent, in context: Context) async -> Timeline<AnyDashEntry> {
    // The desktop app refreshes data every minute while visible; widgets follow the OS cadence.
    Timeline(entries: [macEntry(SnapshotSource.snapshot(widgetId: configuration.widget?.id))],
             policy: .after(Date().addingTimeInterval(15 * 60)))
  }
}

struct SelectedWidget: Widget {
  var body: some WidgetConfiguration {
    AppIntentConfiguration(kind: "io.delino.anydash.selected", intent: SelectWidgetIntent.self, provider: CollectorProvider()) { entry in
      AnyDashWidgetView(entry: entry)
    }
    .configurationDisplayName("AnyDash widget")
    .description("Shows one widget from your dashboards.")
    .supportedFamilies([.systemSmall, .systemMedium])
  }
}

@main
struct AnyDashMacWidgets: WidgetBundle {
  var body: some Widget { SelectedWidget() }
}
