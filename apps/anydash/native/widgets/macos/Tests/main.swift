// SPDX-License-Identifier: Apache-2.0
// Collector response parsing checks for the macOS widget host.
import Foundation

func check(_ condition: Bool, _ message: String) {
  if !condition {
    FileHandle.standardError.write("FAIL: \(message)\n".data(using: .utf8)!)
    exit(1)
  }
}

let fixtures = URL(fileURLWithPath: CommandLine.arguments[1])
let list = Array(try Data(contentsOf: fixtures.appendingPathComponent("snapshots.pb")))
let sized = Array("HTTP/1.1 200 OK\r\ncontent-type: application/proto\r\ncontent-length: \(list.count)\r\n\r\n".utf8) + list
check(try decodeSnapshotList(CollectorClient.parseResponse(sized)).count == 3, "content-length body")
let hex = String(list.count, radix: 16)
let chunked = Array("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n\(hex)\r\n".utf8) + list + Array("\r\n0\r\n\r\n".utf8)
check(try CollectorClient.parseResponse(chunked) == list, "chunked body")
do {
  _ = try CollectorClient.parseResponse(Array("HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n".utf8))
  check(false, "error status")
} catch let error as CollectorError {
  check(error == .status(503), "status error")
}
check(CollectorClient.socketPath(environment: ["ANYDASH_HOME": "/tmp/x"]) == "/tmp/x/data/collector.sock", "custom home")
check(CollectorClient.socketPath(environment: [:]).hasSuffix("Library/Application Support/io.delino.anydash/collector.sock"), "default path")
do {
  _ = try CollectorClient.listSnapshots(path: "/nonexistent/anydash/collector.sock", timeout: 1)
  check(false, "missing collector")
} catch let error as CollectorError {
  check(error == .unavailable, "missing collector is unavailable")
}
print("anydash macOS collector client: ok")
