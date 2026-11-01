// SPDX-License-Identifier: Apache-2.0
import Foundation
import WidgetKit

/// WidgetKit and snapshot decoding for the Objective-C host.
@objc(AnyDashWidgetBridge)
public final class AnyDashWidgetBridge: NSObject {
  @objc public static func reloadAll() {
    WidgetCenter.shared.reloadAllTimelines()
  }

  @objc public static func widgetIdOf(_ data: Data) -> String? {
    (try? decodeSnapshot(Array(data)))?.widgetId
  }
}
