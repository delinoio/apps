// SPDX-License-Identifier: Apache-2.0
// LocalCollectorService.ListNativeWidgetSnapshots over the collector's Unix
// domain socket (Connect protocol, HTTP/1.1, binary Protobuf), as defined in
// docs/cmds-anydash-cli-contract.md#local-ipc. The desktop app writes the
// snapshots after each data refresh; this widget host only reads them.
import Foundation

public enum CollectorError: Error, Equatable {
  case unavailable
  case malformed
  case status(Int)
}

public enum CollectorClient {
  /// `~/Library/Application Support/io.delino.anydash/collector.sock`, or
  /// `$ANYDASH_HOME/collector.sock` when the collector runs with a custom home.
  public static func socketPath(environment: [String: String] = ProcessInfo.processInfo.environment) -> String {
    if let home = environment["ANYDASH_HOME"], !home.isEmpty { return (home as NSString).appendingPathComponent("collector.sock") }
    // Sandboxed extensions see a container home; the collector lives in the real one.
    let user = getpwuid(getuid()).flatMap { String(validatingUTF8: $0.pointee.pw_dir) } ?? NSHomeDirectory()
    return (user as NSString).appendingPathComponent("Library/Application Support/io.delino.anydash/collector.sock")
  }

  public static func listSnapshots(path: String = socketPath(), timeout: TimeInterval = 5) throws -> [NativeWidgetSnapshot] {
    let body = try unary(path: path, method: "ListNativeWidgetSnapshots", request: [], timeout: timeout)
    return try decodeSnapshotList(body)
  }

  static func unary(path: String, method: String, request: [UInt8], timeout: TimeInterval) throws -> [UInt8] {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { throw CollectorError.unavailable }
    defer { close(fd) }
    var seconds = timeval(tv_sec: Int(timeout), tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &seconds, socklen_t(MemoryLayout<timeval>.size))
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &seconds, socklen_t(MemoryLayout<timeval>.size))
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8)
    guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else { throw CollectorError.unavailable }
    withUnsafeMutableBytes(of: &address.sun_path) { buffer in
      buffer.copyBytes(from: bytes)
      buffer[bytes.count] = 0
    }
    let connected = withUnsafePointer(to: &address) {
      $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
    }
    guard connected == 0 else { throw CollectorError.unavailable }
    let head = "POST /anydash.v1.LocalCollectorService/\(method) HTTP/1.1\r\nHost: localhost\r\n"
      + "Content-Type: application/proto\r\nConnect-Protocol-Version: 1\r\n"
      + "Content-Length: \(request.count)\r\nConnection: close\r\n\r\n"
    let message = Array(head.utf8) + request
    var sent = 0
    while sent < message.count {
      let written = message[sent...].withUnsafeBytes { write(fd, $0.baseAddress, $0.count) }
      guard written > 0 else { throw CollectorError.unavailable }
      sent += written
    }
    var response: [UInt8] = []
    var chunk = [UInt8](repeating: 0, count: 16384)
    while true {
      let count = read(fd, &chunk, chunk.count)
      if count < 0 { throw CollectorError.unavailable }
      if count == 0 { break }
      response.append(contentsOf: chunk[0..<count])
      if response.count > 4 << 20 { throw CollectorError.malformed }
    }
    return try parseResponse(response)
  }

  /// HTTP/1.1 response → body; supports Content-Length, chunked and close-delimited bodies.
  public static func parseResponse(_ response: [UInt8]) throws -> [UInt8] {
    let separator: [UInt8] = [13, 10, 13, 10]
    guard let end = (0..<max(0, response.count - 3)).first(where: { Array(response[$0..<$0 + 4]) == separator }),
      let head = String(bytes: response[0..<end], encoding: .utf8)
    else { throw CollectorError.malformed }
    let lines = head.components(separatedBy: "\r\n")
    let status = lines.first?.split(separator: " ").dropFirst().first.flatMap { Int($0) } ?? 0
    var headers: [String: String] = [:]
    for line in lines.dropFirst() {
      guard let colon = line.firstIndex(of: ":") else { continue }
      headers[line[..<colon].lowercased()] = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
    }
    var body = Array(response[(end + 4)...])
    if headers["transfer-encoding"]?.lowercased().contains("chunked") == true {
      var decoded: [UInt8] = []
      var offset = 0
      while true {
        guard let lineEnd = (offset..<max(offset, body.count - 1)).first(where: { body[$0] == 13 && body[$0 + 1] == 10 }),
          let sizeText = String(bytes: body[offset..<lineEnd], encoding: .utf8)?.split(separator: ";").first,
          let size = Int(sizeText.trimmingCharacters(in: .whitespaces), radix: 16)
        else { throw CollectorError.malformed }
        offset = lineEnd + 2
        if size == 0 { break }
        guard offset + size <= body.count else { throw CollectorError.malformed }
        decoded.append(contentsOf: body[offset..<offset + size])
        offset += size + 2
      }
      body = decoded
    } else if let length = headers["content-length"].flatMap({ Int($0) }) {
      guard length <= body.count else { throw CollectorError.malformed }
      body = Array(body[0..<length])
    }
    guard status == 200 else { throw CollectorError.status(status) }
    return body
  }
}
