// SPDX-License-Identifier: Apache-2.0
// LocalCollectorService.ListNativeWidgetSnapshots over the collector's named
// pipe \\.\pipe\io.delino.anydash.collector.<user SID> (Connect protocol,
// HTTP/1.1, binary Protobuf), as defined in the CLI contract's local IPC
// section. The desktop app writes the snapshots after each data refresh; this
// provider only reads them.
using System.IO.Pipes;
using System.Security.Principal;
using System.Text;

namespace AnyDash.Widgets;

public sealed class CollectorUnavailableException(string message) : Exception(message);

public static class CollectorClient
{
    public static string PipeName(string sid) => $"io.delino.anydash.collector.{sid}";

    public static List<NativeWidgetSnapshot> ListSnapshots(TimeSpan timeout)
    {
        var sid = WindowsIdentity.GetCurrent().User?.Value ?? throw new CollectorUnavailableException("No user SID.");
        using var pipe = new NamedPipeClientStream(".", PipeName(sid), PipeDirection.InOut, PipeOptions.None,
            TokenImpersonationLevel.Identification);
        try
        {
            pipe.Connect((int)timeout.TotalMilliseconds);
        }
        catch (Exception error) when (error is TimeoutException or IOException)
        {
            throw new CollectorUnavailableException("The collector is not running.");
        }
        var request = Encoding.ASCII.GetBytes(
            "POST /anydash.v1.LocalCollectorService/ListNativeWidgetSnapshots HTTP/1.1\r\nHost: localhost\r\n" +
            "Content-Type: application/proto\r\nConnect-Protocol-Version: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        pipe.Write(request, 0, request.Length);
        pipe.Flush();
        using var response = new MemoryStream();
        var buffer = new byte[16384];
        int read;
        while ((read = pipe.Read(buffer, 0, buffer.Length)) > 0)
        {
            response.Write(buffer, 0, read);
            if (response.Length > 4 << 20) throw new CollectorUnavailableException("The collector reply is too large.");
            // Connect unary replies are complete once the declared body has arrived.
            if (HttpResponse.Complete(response.GetBuffer().AsSpan(0, (int)response.Length))) break;
        }
        return SnapshotCodec.SnapshotList(HttpResponse.Body(response.ToArray()));
    }
}

/// <summary>HTTP/1.1 response parsing for Content-Length, chunked and close-delimited bodies.</summary>
public static class HttpResponse
{
    private static int HeaderEnd(ReadOnlySpan<byte> response) => response.IndexOf("\r\n\r\n"u8);

    public static bool Complete(ReadOnlySpan<byte> response)
    {
        var end = HeaderEnd(response);
        if (end < 0) return false;
        var headers = Headers(Encoding.ASCII.GetString(response[..end]));
        var body = response[(end + 4)..];
        if (headers.TryGetValue("transfer-encoding", out var encoding) && encoding.Contains("chunked", StringComparison.OrdinalIgnoreCase))
            return body.IndexOf("\r\n0\r\n\r\n"u8) >= 0 || body.StartsWith("0\r\n\r\n"u8);
        return headers.TryGetValue("content-length", out var length) && int.TryParse(length, out var size) && body.Length >= size;
    }

    public static byte[] Body(byte[] response)
    {
        var end = HeaderEnd(response);
        if (end < 0) throw new CollectorUnavailableException("Malformed collector reply.");
        var head = Encoding.ASCII.GetString(response, 0, end).Split("\r\n");
        var status = head[0].Split(' ').Skip(1).FirstOrDefault();
        var headers = Headers(string.Join("\r\n", head));
        var body = response.AsSpan(end + 4);
        byte[] result;
        if (headers.TryGetValue("transfer-encoding", out var encoding) && encoding.Contains("chunked", StringComparison.OrdinalIgnoreCase))
        {
            using var decoded = new MemoryStream();
            var offset = 0;
            while (true)
            {
                var lineEnd = body[offset..].IndexOf("\r\n"u8);
                if (lineEnd < 0) throw new CollectorUnavailableException("Malformed chunk.");
                var sizeText = Encoding.ASCII.GetString(body.Slice(offset, lineEnd)).Split(';')[0].Trim();
                var size = Convert.ToInt32(sizeText, 16);
                offset += lineEnd + 2;
                if (size == 0) break;
                if (offset + size > body.Length) throw new CollectorUnavailableException("Truncated chunk.");
                decoded.Write(body.Slice(offset, size));
                offset += size + 2;
            }
            result = decoded.ToArray();
        }
        else if (headers.TryGetValue("content-length", out var length) && int.TryParse(length, out var size))
        {
            if (size > body.Length) throw new CollectorUnavailableException("Truncated collector reply.");
            result = body[..size].ToArray();
        }
        else result = body.ToArray();
        if (status != "200") throw new CollectorUnavailableException($"Collector status {status}.");
        return result;
    }

    private static Dictionary<string, string> Headers(string head)
    {
        var headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var line in head.Split("\r\n").Skip(1))
        {
            var colon = line.IndexOf(':');
            if (colon > 0) headers[line[..colon].Trim()] = line[(colon + 1)..].Trim();
        }
        return headers;
    }
}
