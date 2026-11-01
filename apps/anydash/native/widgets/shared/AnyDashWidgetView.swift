// SPDX-License-Identifier: Apache-2.0
// Native widget layout from the design record: "{Provider} · {metric}", the
// primary value, one supporting line and "Updated {time}"; ENDED snapshots show
// "Sharing ended" without data. Tapping opens the widget's deep link.
import SwiftUI
import WidgetKit

public struct AnyDashEntry: TimelineEntry {
  public let date: Date
  public let snapshot: NativeWidgetSnapshot?
  /// Last known title, kept for ended widgets whose data was removed.
  public let title: String

  public init(date: Date, snapshot: NativeWidgetSnapshot?, title: String) {
    self.date = date
    self.snapshot = snapshot
    self.title = title
  }
}

public struct AnyDashWidgetView: View {
  let entry: AnyDashEntry
  public init(entry: AnyDashEntry) { self.entry = entry }

  public var body: some View {
    content
      .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
      .widgetURL(entry.snapshot?.deepLink)
      .accessibilityElement(children: .ignore)
      .accessibilityLabel(label)
      .modifier(WidgetBackground())
  }

  private var label: String {
    guard let snapshot = entry.snapshot else { return WidgetText.empty }
    return WidgetText.accessibility(snapshot, title: entry.title, now: entry.date)
  }

  @ViewBuilder private var content: some View {
    if let snapshot = entry.snapshot, snapshot.sharingState == .ended || snapshot.summary == nil {
      VStack(alignment: .leading, spacing: 4) {
        Text(entry.title).font(.caption.weight(.semibold)).foregroundStyle(.secondary).lineLimit(1)
        Text(WidgetText.sharingEnded).font(.system(size: 15, weight: .semibold)).foregroundStyle(.secondary)
        Text(WidgetText.sharingEndedDetail).font(.caption).foregroundStyle(.secondary)
      }
    } else if let snapshot = entry.snapshot, let summary = snapshot.summary {
      VStack(alignment: .leading, spacing: 4) {
        Text(summary.title).font(.caption.weight(.semibold)).foregroundStyle(.secondary).lineLimit(1)
        if summary.primaryValue.state == .notProvided {
          Text(WidgetText.notProvided).font(.system(size: 18, weight: .semibold)).foregroundStyle(.secondary)
        } else {
          Text(WidgetText.value(summary.primaryValue, now: entry.date))
            .font(.system(size: 26, weight: .bold))
            .opacity(summary.primaryValue.state == .failed || summary.primaryValue.state == .authExpired ? 0.55 : 1)
            .minimumScaleFactor(0.6)
            .lineLimit(1)
        }
        Text(WidgetText.supporting(summary, now: entry.date)).font(.caption).foregroundStyle(.secondary).lineLimit(2)
        Spacer(minLength: 0)
        if let updated = summary.dataUpdatedAt ?? snapshot.refreshedAt {
          Text(WidgetText.updated(updated, now: entry.date)).font(.caption).foregroundStyle(.secondary)
        }
      }
    } else {
      VStack(alignment: .leading, spacing: 4) {
        Text("AnyDash").font(.caption.weight(.semibold))
        Text(WidgetText.empty).font(.caption).foregroundStyle(.secondary)
      }
    }
  }
}

struct WidgetBackground: ViewModifier {
  func body(content: Content) -> some View {
    if #available(iOS 17.0, macOS 14.0, *) {
      content.containerBackground(.background, for: .widget)
    } else {
      content.padding().background(Color(white: 1))
    }
  }
}
