// SPDX-License-Identifier: Apache-2.0
// Containing app for the macOS widget extension. It has no interface: the
// AnyDash desktop app owns sign-in, data and settings, and the widget reads
// snapshots from the local collector. Launching it registers the extension.
import Foundation

print("{\"event\":\"anydash_widget_host_started\",\"platform\":\"macos\"}")
