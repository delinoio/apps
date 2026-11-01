// SPDX-License-Identifier: Apache-2.0
// Decoder checks against fixtures from the TypeScript bindings (scripts/widget-fixtures.ts).
using System.Text;
using System.Text.Json.Nodes;
using AnyDash.Widgets;
using Xunit;

public class SnapshotTests
{
    private static readonly string Fixtures = Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", "..", "fixtures");
    private static readonly DateTimeOffset Now = DateTimeOffset.Parse("2026-11-01T03:41:00Z");
    private static List<NativeWidgetSnapshot> List() => SnapshotCodec.SnapshotList(File.ReadAllBytes(Path.Combine(Fixtures, "snapshots.pb")));

    [Fact]
    public void DecodesSnapshots()
    {
        var list = List();
        Assert.Equal(3, list.Count);
        var codex = list[0].Summary!;
        Assert.Equal("Codex · 5-hour", codex.Title);
        Assert.Equal("62%", WidgetText.Value(codex.Primary, Now, TimeZoneInfo.Utc));
        Assert.Equal("left · Resets 05:20", WidgetText.Supporting(codex, Now, TimeZoneInfo.Utc));
        Assert.Equal("anydash://spaces/0192f000-0000-7000-8000-0000000000a1/dashboards/0192f000-0000-7000-8000-0000000000c1?widget=0192f4aa-0000-7000-8000-000000000001", list[0].DeepLink);
        Assert.Equal(SharingState.Ended, list[1].SharingState);
        Assert.Null(list[1].Summary);
        Assert.Equal("$41.80", WidgetText.Value(list[2].Summary!.Primary, Now, TimeZoneInfo.Utc));
    }

    [Fact]
    public void CardsShowValueOrSharingEnded()
    {
        var list = List();
        var value = JsonNode.Parse(WidgetCard.Data(list[0], "AnyDash", Now, TimeZoneInfo.Utc))!;
        Assert.Equal("62%", value["value"]!.GetValue<string>());
        Assert.Equal("Updated 03:41", value["updated"]!.GetValue<string>());
        var ended = JsonNode.Parse(WidgetCard.Data(list[1], "Supabase · Requests", Now, TimeZoneInfo.Utc))!;
        Assert.Equal(WidgetText.SharingEnded, ended["value"]!.GetValue<string>());
        Assert.Equal("", ended["updated"]!.GetValue<string>());
        Assert.Equal("w1", WidgetCard.SelectedWidgetId("{\"widgetId\":\"w1\"}"));
    }

    [Fact]
    public void ParsesCollectorReplies()
    {
        var body = File.ReadAllBytes(Path.Combine(Fixtures, "snapshots.pb"));
        var sized = Encoding.ASCII.GetBytes($"HTTP/1.1 200 OK\r\ncontent-length: {body.Length}\r\n\r\n").Concat(body).ToArray();
        Assert.True(HttpResponse.Complete(sized));
        Assert.Equal(body, HttpResponse.Body(sized));
        var chunked = Encoding.ASCII.GetBytes($"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n{body.Length:x}\r\n")
            .Concat(body).Concat(Encoding.ASCII.GetBytes("\r\n0\r\n\r\n")).ToArray();
        Assert.Equal(body, HttpResponse.Body(chunked));
        Assert.Throws<CollectorUnavailableException>(() => HttpResponse.Body(Encoding.ASCII.GetBytes("HTTP/1.1 503 x\r\ncontent-length: 0\r\n\r\n")));
    }
}
