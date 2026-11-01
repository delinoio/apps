// SPDX-License-Identifier: Apache-2.0
// iOS home screen widgets. Snapshots come from the app group, written by the
// app after each refresh. When the OS lets the widget refresh and the app's
// shared access token is still valid, it calls QueryWidgetData as
// CLIENT_KIND_NATIVE_WIDGET; otherwise it shows the latest snapshot and time.
import AppIntents
import Foundation
import Security
import SwiftUI
import WidgetKit

enum SnapshotStore {
  static let group = "group.io.delino.anydash"

  static func all() -> [NativeWidgetSnapshot] {
    guard let directory = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)?
      .appendingPathComponent("snapshots", isDirectory: true),
      let files = try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
    else { return [] }
    return files.compactMap { file in
      guard file.pathExtension == "pb", let data = try? Data(contentsOf: file) else { return nil }
      return try? decodeSnapshot(Array(data))
    }.sorted { ($0.refreshedAt ?? .distantPast) > ($1.refreshedAt ?? .distantPast) }
  }

  static func snapshot(widgetId: String?) -> NativeWidgetSnapshot? {
    let snapshots = all()
    guard let widgetId else { return snapshots.first }
    return snapshots.first { $0.widgetId == widgetId }
  }
}

enum WidgetRefresh {
  /// Reads the short-lived AnyDash access token the app shares; never a refresh token.
  static func sharedToken() -> (token: String, version: String)? {
    guard let group = Bundle.main.object(forInfoDictionaryKey: "AnyDashSharedKeychainGroup") as? String else { return nil }
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: "io.delino.anydash.session",
      kSecAttrAccount as String: "widget-access-token",
      kSecAttrAccessGroup as String: group,
      kSecReturnData as String: true,
    ]
    var result: CFTypeRef?
    guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
      let data = result as? Data,
      let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      let token = json["token"] as? String,
      let expiresAt = json["expiresAt"] as? Double,
      expiresAt - 60 > Date().timeIntervalSince1970
    else { return nil }
    return (token, json["version"] as? String ?? "0.0.0")
  }

  static func refresh(_ snapshot: NativeWidgetSnapshot) async -> NativeWidgetSnapshot {
    guard snapshot.sharingState != .ended, let (token, version) = sharedToken(),
      let base = Bundle.main.object(forInfoDictionaryKey: "AnyDashAPIURL") as? String,
      let url = URL(string: "\(base)/anydash.v1.WidgetDataService/QueryWidgetData")
    else { return snapshot }
    var request = URLRequest(url: url, timeoutInterval: 15)
    request.httpMethod = "POST"
    request.httpBody = Data(encodeQueryWidgetData(dashboardId: snapshot.dashboardId, widgetId: snapshot.widgetId, timeZone: ""))
    request.setValue("application/proto", forHTTPHeaderField: "Content-Type")
    request.setValue("1", forHTTPHeaderField: "Connect-Protocol-Version")
    request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    request.setValue("CLIENT_KIND_NATIVE_WIDGET", forHTTPHeaderField: "X-AnyDash-Client-Kind")
    request.setValue(version, forHTTPHeaderField: "X-AnyDash-Client-Version")
    guard let (data, response) = try? await URLSession.shared.data(for: request),
      (response as? HTTPURLResponse)?.statusCode == 200,
      let decoded = try? decodeWidgetDataSummary(Array(data))
    else { return snapshot }
    var next = snapshot
    next.sharingState = decoded.sharingState == .unspecified ? snapshot.sharingState : decoded.sharingState
    next.summary = decoded.summary
    next.refreshedAt = Date()
    return next
  }
}

func entry(for snapshot: NativeWidgetSnapshot?, at date: Date) -> AnyDashEntry {
  AnyDashEntry(date: date, snapshot: snapshot, title: snapshot?.summary?.title ?? TitleMemory.title(for: snapshot?.widgetId))
}

/// Remembers widget titles so an ended widget can still name what ended.
enum TitleMemory {
  static let defaults = UserDefaults(suiteName: SnapshotStore.group)
  static func title(for widgetId: String?) -> String {
    widgetId.flatMap { defaults?.string(forKey: "title.\($0)") } ?? "AnyDash"
  }
  static func remember(_ snapshot: NativeWidgetSnapshot?) {
    if let snapshot, let title = snapshot.summary?.title { defaults?.set(title, forKey: "title.\(snapshot.widgetId)") }
  }
}

struct LatestProvider: TimelineProvider {
  func placeholder(in context: Context) -> AnyDashEntry { entry(for: nil, at: Date()) }
  func getSnapshot(in context: Context, completion: @escaping (AnyDashEntry) -> Void) {
    completion(entry(for: SnapshotStore.snapshot(widgetId: nil), at: Date()))
  }
  func getTimeline(in context: Context, completion: @escaping (Timeline<AnyDashEntry>) -> Void) {
    Task {
      var snapshot = SnapshotStore.snapshot(widgetId: nil)
      if let current = snapshot { snapshot = await WidgetRefresh.refresh(current) }
      TitleMemory.remember(snapshot)
      completion(Timeline(entries: [entry(for: snapshot, at: Date())], policy: .after(Date().addingTimeInterval(15 * 60))))
    }
  }
}

struct LatestWidget: Widget {
  var body: some WidgetConfiguration {
    StaticConfiguration(kind: "io.delino.anydash.latest", provider: LatestProvider()) { entry in
      AnyDashWidgetView(entry: entry)
    }
    .configurationDisplayName("AnyDash")
    .description("Your most recently viewed widget.")
    .supportedFamilies([.systemSmall, .systemMedium])
  }
}

@available(iOS 17.0, *)
struct WidgetChoice: AppEntity {
  static var typeDisplayRepresentation: TypeDisplayRepresentation = "Widget"
  static var defaultQuery = WidgetChoiceQuery()
  var id: String
  var title: String
  var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(title)") }
}

@available(iOS 17.0, *)
struct WidgetChoiceQuery: EntityQuery {
  func entities(for identifiers: [String]) async throws -> [WidgetChoice] {
    try await suggestedEntities().filter { identifiers.contains($0.id) }
  }
  func suggestedEntities() async throws -> [WidgetChoice] {
    SnapshotStore.all().map { WidgetChoice(id: $0.widgetId, title: $0.summary?.title ?? TitleMemory.title(for: $0.widgetId)) }
  }
}

@available(iOS 17.0, *)
struct SelectWidgetIntent: WidgetConfigurationIntent {
  static var title: LocalizedStringResource = "Choose a widget"
  @Parameter(title: "Widget") var widget: WidgetChoice?
}

@available(iOS 17.0, *)
struct SelectedProvider: AppIntentTimelineProvider {
  func placeholder(in context: Context) -> AnyDashEntry { entry(for: nil, at: Date()) }
  func snapshot(for configuration: SelectWidgetIntent, in context: Context) async -> AnyDashEntry {
    entry(for: SnapshotStore.snapshot(widgetId: configuration.widget?.id), at: Date())
  }
  func timeline(for configuration: SelectWidgetIntent, in context: Context) async -> Timeline<AnyDashEntry> {
    var snapshot = SnapshotStore.snapshot(widgetId: configuration.widget?.id)
    if let current = snapshot { snapshot = await WidgetRefresh.refresh(current) }
    TitleMemory.remember(snapshot)
    return Timeline(entries: [entry(for: snapshot, at: Date())], policy: .after(Date().addingTimeInterval(15 * 60)))
  }
}

@available(iOS 17.0, *)
struct SelectedWidget: Widget {
  var body: some WidgetConfiguration {
    AppIntentConfiguration(kind: "io.delino.anydash.selected", intent: SelectWidgetIntent.self, provider: SelectedProvider()) { entry in
      AnyDashWidgetView(entry: entry)
    }
    .configurationDisplayName("AnyDash widget")
    .description("Shows one widget from your dashboards.")
    .supportedFamilies([.systemSmall, .systemMedium])
  }
}

// WidgetBundleBuilder has no availability branches before iOS 17, so the entry
// point picks one bundle: iOS 17+ lets the user choose the widget; iOS 16
// shows the most recently refreshed snapshot.
@available(iOS 17.0, *)
struct SelectableWidgets: WidgetBundle {
  var body: some Widget { SelectedWidget() }
}

struct LatestWidgets: WidgetBundle {
  var body: some Widget { LatestWidget() }
}

@main
enum AnyDashWidgetsEntry {
  static func main() {
    if #available(iOS 17.0, *) {
      SelectableWidgets.main()
    } else {
      LatestWidgets.main()
    }
  }
}
