// SPDX-License-Identifier: Apache-2.0
// Read-only decoding of the anydash.v1 messages that native widgets display:
// NativeWidgetSnapshot (from the collector or the app group) and WidgetData
// (from QueryWidgetData). Field numbers follow protos/anydash/v1/anydash.proto;
// tests decode fixtures produced by the TypeScript bindings.
import Foundation

public enum SharingState: Int {
  case unspecified = 0, privateWidget = 1, shared = 2, ended = 3
}

public enum MetricState: Int {
  case unspecified = 0, value = 1, notProvided = 2, failed = 3, authExpired = 4, partial = 5, cannotBeCalculated = 6
}

public enum MetricContent: Equatable {
  case none
  case percent(Double)
  case decimal(String)
  case integer(Int64)
  case money(currency: String, amount: String)
  case time(Date)
  case duration(TimeInterval)
  case text(String)
  case flag(Bool)
}

public struct MetricValue: Equatable {
  public var state = MetricState.unspecified
  public var content = MetricContent.none
}

public struct WidgetSummary: Equatable {
  public var title = ""
  public var primaryLabel = ""
  public var primaryValue = MetricValue()
  public var secondaryLabel = ""
  public var secondaryValue = MetricValue()
  public var dataUpdatedAt: Date?
}

public struct NativeWidgetSnapshot: Equatable {
  public var dashboardId = ""
  public var widgetId = ""
  public var spaceId = ""
  public var sharingState = SharingState.unspecified
  public var summary: WidgetSummary?
  public var refreshedAt: Date?

  /// anydash://spaces/<space>/dashboards/<dashboard>?widget=<widget>
  public var deepLink: URL? {
    URL(string: "anydash://spaces/\(spaceId)/dashboards/\(dashboardId)?widget=\(widgetId)")
  }
}

public enum DecodeError: Error { case truncated, malformed }

/// Minimal Protobuf wire reader for the fields above. Unknown fields are skipped.
struct WireReader {
  private let data: [UInt8]
  private var offset = 0
  init(_ data: [UInt8]) { self.data = data }
  var atEnd: Bool { offset >= data.count }

  mutating func varint() throws -> UInt64 {
    var result: UInt64 = 0
    var shift: UInt64 = 0
    while true {
      guard offset < data.count, shift < 64 else { throw DecodeError.truncated }
      let byte = data[offset]
      offset += 1
      result |= UInt64(byte & 0x7f) << shift
      if byte & 0x80 == 0 { return result }
      shift += 7
    }
  }
  mutating func tag() throws -> (field: Int, wire: Int) {
    let value = try varint()
    return (Int(value >> 3), Int(value & 7))
  }
  mutating func bytes() throws -> [UInt8] {
    let length = Int(try varint())
    guard length >= 0, offset + length <= data.count else { throw DecodeError.truncated }
    defer { offset += length }
    return Array(data[offset..<offset + length])
  }
  mutating func string() throws -> String {
    guard let value = String(bytes: try bytes(), encoding: .utf8) else { throw DecodeError.malformed }
    return value
  }
  mutating func fixed64() throws -> UInt64 {
    guard offset + 8 <= data.count else { throw DecodeError.truncated }
    var value: UInt64 = 0
    for index in 0..<8 { value |= UInt64(data[offset + index]) << (8 * UInt64(index)) }
    offset += 8
    return value
  }
  mutating func skip(_ wire: Int) throws {
    switch wire {
    case 0: _ = try varint()
    case 1: _ = try fixed64()
    case 2: _ = try bytes()
    case 5:
      guard offset + 4 <= data.count else { throw DecodeError.truncated }
      offset += 4
    default: throw DecodeError.malformed
    }
  }
}

func decodeTimestamp(_ bytes: [UInt8]) throws -> Date {
  var reader = WireReader(bytes)
  var seconds: Int64 = 0
  var nanos: Int64 = 0
  while !reader.atEnd {
    let (field, wire) = try reader.tag()
    switch (field, wire) {
    case (1, 0): seconds = Int64(bitPattern: try reader.varint())
    case (2, 0): nanos = Int64(Int32(truncatingIfNeeded: try reader.varint()))
    default: try reader.skip(wire)
    }
  }
  return Date(timeIntervalSince1970: TimeInterval(seconds) + TimeInterval(nanos) / 1e9)
}

func decodeMetricValue(_ bytes: [UInt8]) throws -> MetricValue {
  var reader = WireReader(bytes)
  var value = MetricValue()
  while !reader.atEnd {
    let (field, wire) = try reader.tag()
    switch (field, wire) {
    case (1, 0): value.state = MetricState(rawValue: Int(try reader.varint())) ?? .unspecified
    case (2, 1): value.content = .percent(Double(bitPattern: try reader.fixed64()))
    case (3, 2): value.content = .decimal(try reader.string())
    case (4, 0): value.content = .integer(Int64(bitPattern: try reader.varint()))
    case (5, 2):
      var money = WireReader(try reader.bytes())
      var currency = ""
      var amount = ""
      while !money.atEnd {
        let (moneyField, moneyWire) = try money.tag()
        switch (moneyField, moneyWire) {
        case (1, 2): currency = try money.string()
        case (2, 2): amount = try money.string()
        default: try money.skip(moneyWire)
        }
      }
      value.content = .money(currency: currency, amount: amount)
    case (6, 2): value.content = .time(try decodeTimestamp(try reader.bytes()))
    case (7, 2): value.content = .duration(try decodeTimestamp(try reader.bytes()).timeIntervalSince1970)
    case (8, 2): value.content = .text(try reader.string())
    case (9, 0): value.content = .flag(try reader.varint() != 0)
    default: try reader.skip(wire)
    }
  }
  return value
}

func decodeSummary(_ bytes: [UInt8]) throws -> WidgetSummary {
  var reader = WireReader(bytes)
  var summary = WidgetSummary()
  while !reader.atEnd {
    let (field, wire) = try reader.tag()
    switch (field, wire) {
    case (1, 2): summary.title = try reader.string()
    case (2, 2): summary.primaryLabel = try reader.string()
    case (3, 2): summary.primaryValue = try decodeMetricValue(try reader.bytes())
    case (4, 2): summary.secondaryLabel = try reader.string()
    case (5, 2): summary.secondaryValue = try decodeMetricValue(try reader.bytes())
    case (6, 2): summary.dataUpdatedAt = try decodeTimestamp(try reader.bytes())
    default: try reader.skip(wire)
    }
  }
  return summary
}

public func decodeSnapshot(_ bytes: [UInt8]) throws -> NativeWidgetSnapshot {
  var reader = WireReader(bytes)
  var snapshot = NativeWidgetSnapshot()
  while !reader.atEnd {
    let (field, wire) = try reader.tag()
    switch (field, wire) {
    case (1, 2): snapshot.dashboardId = try reader.string()
    case (2, 2): snapshot.widgetId = try reader.string()
    case (3, 2): snapshot.spaceId = try reader.string()
    case (4, 0): snapshot.sharingState = SharingState(rawValue: Int(try reader.varint())) ?? .unspecified
    case (5, 2): snapshot.summary = try decodeSummary(try reader.bytes())
    case (6, 2): snapshot.refreshedAt = try decodeTimestamp(try reader.bytes())
    default: try reader.skip(wire)
    }
  }
  // ENDED snapshots never display data, even if a summary was present.
  if snapshot.sharingState == .ended { snapshot.summary = nil }
  return snapshot
}

/// ListNativeWidgetSnapshotsResponse: field 1 repeats NativeWidgetSnapshot.
public func decodeSnapshotList(_ bytes: [UInt8]) throws -> [NativeWidgetSnapshot] {
  var reader = WireReader(bytes)
  var snapshots: [NativeWidgetSnapshot] = []
  while !reader.atEnd {
    let (field, wire) = try reader.tag()
    if field == 1 && wire == 2 { snapshots.append(try decodeSnapshot(try reader.bytes())) } else { try reader.skip(wire) }
  }
  return snapshots
}

/// QueryWidgetDataResponse → the summary fields of WidgetData (field 1).
public func decodeWidgetDataSummary(_ bytes: [UInt8]) throws -> (sharingState: SharingState, summary: WidgetSummary?) {
  var outer = WireReader(bytes)
  var state = SharingState.unspecified
  var summary: WidgetSummary?
  while !outer.atEnd {
    let (field, wire) = try outer.tag()
    guard field == 1, wire == 2 else { try outer.skip(wire); continue }
    var reader = WireReader(try outer.bytes())
    while !reader.atEnd {
      let (inner, innerWire) = try reader.tag()
      switch (inner, innerWire) {
      case (3, 0): state = SharingState(rawValue: Int(try reader.varint())) ?? .unspecified
      case (9, 2): summary = try decodeSummary(try reader.bytes())
      default: try reader.skip(innerWire)
      }
    }
  }
  return (state, state == .ended ? nil : summary)
}

private func encodeString(_ field: Int, _ value: String, into bytes: inout [UInt8]) {
  let utf8 = Array(value.utf8)
  guard !utf8.isEmpty else { return }
  bytes.append(UInt8(field << 3 | 2))
  var length = UInt64(utf8.count)
  repeat {
    var byte = UInt8(length & 0x7f)
    length >>= 7
    if length != 0 { byte |= 0x80 }
    bytes.append(byte)
  } while length != 0
  bytes.append(contentsOf: utf8)
}

/// QueryWidgetDataRequest(dashboard_id, widget_id, time_zone).
public func encodeQueryWidgetData(dashboardId: String, widgetId: String, timeZone: String) -> [UInt8] {
  var bytes: [UInt8] = []
  encodeString(1, dashboardId, into: &bytes)
  encodeString(2, widgetId, into: &bytes)
  encodeString(3, timeZone, into: &bytes)
  return bytes
}

/// English display strings matching the design record's native widget.
public enum WidgetText {
  public static let sharingEnded = "Sharing ended"
  public static let sharingEndedDetail = "Open AnyDash to choose another widget."
  public static let notProvided = "Not provided"
  public static let empty = "Choose a widget in AnyDash."

  public static func updated(_ date: Date, now: Date = Date(), calendar: Calendar = .current) -> String {
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.dateFormat = calendar.isDate(date, inSameDayAs: now) ? "HH:mm" : "MMM d, HH:mm"
    return "Updated \(formatter.string(from: date))"
  }

  public static func time(_ date: Date, now: Date = Date(), calendar: Calendar = .current) -> String {
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.dateFormat = calendar.isDate(date, inSameDayAs: now) ? "HH:mm" : "MMM d, HH:mm"
    return formatter.string(from: date)
  }

  public static func value(_ value: MetricValue, now: Date = Date()) -> String {
    if value.state == .notProvided { return notProvided }
    switch value.content {
    case .percent(let percent): return "\(Int(percent.rounded()))%"
    case .decimal(let text): return text
    case .integer(let integer):
      let formatter = NumberFormatter()
      formatter.numberStyle = .decimal
      formatter.locale = Locale(identifier: "en_US")
      return formatter.string(from: NSNumber(value: integer)) ?? String(integer)
    case .money(let currency, let amount):
      let rounded = Decimal(string: amount).map { NSDecimalNumber(decimal: $0).rounding(accordingToBehavior: NSDecimalNumberHandler(roundingMode: .plain, scale: 2, raiseOnExactness: false, raiseOnOverflow: false, raiseOnUnderflow: false, raiseOnDivideByZero: false)) }
      let text = rounded.map { String(format: "%.2f", $0.doubleValue) } ?? amount
      return currency == "USD" ? "$\(text)" : currency.isEmpty ? text : "\(text) \(currency)"
    case .time(let date): return time(date, now: now)
    case .duration(let seconds): return seconds >= 86400 ? "\(Int(seconds / 86400)) days" : "\(Int(seconds / 3600)) hours"
    case .text(let text): return text
    case .flag(let flag): return flag ? "On" : "Off"
    case .none: return ""
    }
  }

  /// "left · Resets 14:20" from the summary labels.
  public static func supporting(_ summary: WidgetSummary, now: Date = Date()) -> String {
    let secondary = [summary.secondaryLabel, value(summary.secondaryValue, now: now)].filter { !$0.isEmpty }.joined(separator: " ")
    return [summary.primaryLabel, secondary].filter { !$0.isEmpty }.joined(separator: " · ")
  }

  /// VoiceOver reads the state label, then the value, then the update time.
  public static func accessibility(_ snapshot: NativeWidgetSnapshot, title: String, now: Date = Date()) -> String {
    guard let summary = snapshot.summary, snapshot.sharingState != .ended else {
      return "\(title). \(sharingEnded). \(sharingEndedDetail)"
    }
    let state: String
    switch summary.primaryValue.state {
    case .failed: state = "Couldn’t refresh. "
    case .authExpired: state = "Authentication expired. "
    case .notProvided: state = ""
    default: state = ""
    }
    let updated = (summary.dataUpdatedAt ?? snapshot.refreshedAt).map { ". \(Self.updated($0, now: now))" } ?? ""
    return "\(summary.title). \(state)\(value(summary.primaryValue, now: now)) \(supporting(summary, now: now))\(updated)"
  }
}
