// SPDX-License-Identifier: Apache-2.0
// Read-only decoding of the anydash.v1 NativeWidgetSnapshot messages the
// Windows widget displays. Field numbers follow protos/anydash/v1/anydash.proto
// and match the shared Swift and Android decoders. This file has no Windows
// dependencies so the decoder tests run on any .NET host.
using System.Globalization;
using System.Text;

namespace AnyDash.Widgets;

public enum SharingState { Unspecified = 0, Private = 1, Shared = 2, Ended = 3 }

public enum MetricState { Unspecified = 0, Value = 1, NotProvided = 2, Failed = 3, AuthExpired = 4, Partial = 5, CannotBeCalculated = 6 }

public enum ContentKind { None, Percent, Decimal, Integer, Money, Time, Duration, Text, Flag }

public sealed record MetricValue(MetricState State = MetricState.Unspecified, ContentKind Kind = ContentKind.None,
    double Number = 0, string Text = "", string Currency = "", DateTimeOffset? Time = null, bool Flag = false);

public sealed record WidgetSummary(string Title = "", string PrimaryLabel = "", MetricValue? PrimaryValue = null,
    string SecondaryLabel = "", MetricValue? SecondaryValue = null, DateTimeOffset? DataUpdatedAt = null)
{
    public MetricValue Primary => PrimaryValue ?? new MetricValue();
    public MetricValue Secondary => SecondaryValue ?? new MetricValue();
}

public sealed record NativeWidgetSnapshot(string DashboardId = "", string WidgetId = "", string SpaceId = "",
    SharingState SharingState = SharingState.Unspecified, WidgetSummary? Summary = null, DateTimeOffset? RefreshedAt = null)
{
    /// <summary>anydash://spaces/&lt;space&gt;/dashboards/&lt;dashboard&gt;?widget=&lt;widget&gt;</summary>
    public string DeepLink => $"anydash://spaces/{SpaceId}/dashboards/{DashboardId}?widget={WidgetId}";
    public bool Ended => SharingState == SharingState.Ended || Summary is null;
}

public sealed class DecodeException(string message) : Exception(message);

/// <summary>Minimal Protobuf wire reader; unknown fields are skipped.</summary>
internal sealed class WireReader(byte[] data)
{
    private int offset;
    public bool AtEnd => offset >= data.Length;

    public ulong Varint()
    {
        ulong result = 0;
        for (var shift = 0; shift < 64; shift += 7)
        {
            if (offset >= data.Length) throw new DecodeException("truncated varint");
            var value = data[offset++];
            result |= (ulong)(value & 0x7f) << shift;
            if ((value & 0x80) == 0) return result;
        }
        throw new DecodeException("varint too long");
    }

    public (int Field, int Wire) Tag()
    {
        var value = Varint();
        return ((int)(value >> 3), (int)(value & 7));
    }

    public byte[] Bytes()
    {
        var length = Varint();
        if (length > (ulong)(data.Length - offset)) throw new DecodeException("truncated bytes");
        var result = data.AsSpan(offset, (int)length).ToArray();
        offset += (int)length;
        return result;
    }

    public string String() => Encoding.UTF8.GetString(Bytes());

    public ulong Fixed64()
    {
        if (offset + 8 > data.Length) throw new DecodeException("truncated fixed64");
        var value = BitConverter.ToUInt64(data, offset);
        offset += 8;
        return value;
    }

    public void Skip(int wire)
    {
        switch (wire)
        {
            case 0: Varint(); break;
            case 1: Fixed64(); break;
            case 2: Bytes(); break;
            case 5:
                if (offset + 4 > data.Length) throw new DecodeException("truncated fixed32");
                offset += 4;
                break;
            default: throw new DecodeException($"unsupported wire type {wire}");
        }
    }
}

public static class SnapshotCodec
{
    private static DateTimeOffset Timestamp(byte[] bytes)
    {
        var reader = new WireReader(bytes);
        long seconds = 0, nanos = 0;
        while (!reader.AtEnd)
        {
            var (field, wire) = reader.Tag();
            if (field == 1 && wire == 0) seconds = (long)reader.Varint();
            else if (field == 2 && wire == 0) nanos = (int)reader.Varint();
            else reader.Skip(wire);
        }
        return DateTimeOffset.FromUnixTimeMilliseconds(seconds * 1000 + nanos / 1_000_000);
    }

    private static MetricValue Metric(byte[] bytes)
    {
        var reader = new WireReader(bytes);
        var value = new MetricValue();
        while (!reader.AtEnd)
        {
            var (field, wire) = reader.Tag();
            value = (field, wire) switch
            {
                (1, 0) => value with { State = (MetricState)(int)reader.Varint() },
                (2, 1) => value with { Kind = ContentKind.Percent, Number = BitConverter.Int64BitsToDouble((long)reader.Fixed64()) },
                (3, 2) => value with { Kind = ContentKind.Decimal, Text = reader.String() },
                (4, 0) => value with { Kind = ContentKind.Integer, Number = (long)reader.Varint() },
                (5, 2) => Money(value, reader.Bytes()),
                (6, 2) => value with { Kind = ContentKind.Time, Time = Timestamp(reader.Bytes()) },
                (7, 2) => value with { Kind = ContentKind.Duration, Number = Timestamp(reader.Bytes()).ToUnixTimeMilliseconds() / 1000.0 },
                (8, 2) => value with { Kind = ContentKind.Text, Text = reader.String() },
                (9, 0) => value with { Kind = ContentKind.Flag, Flag = reader.Varint() != 0 },
                _ => Skip(reader, wire, value),
            };
        }
        return value;
    }

    private static T Skip<T>(WireReader reader, int wire, T value)
    {
        reader.Skip(wire);
        return value;
    }

    private static MetricValue Money(MetricValue value, byte[] bytes)
    {
        var reader = new WireReader(bytes);
        string currency = "", amount = "";
        while (!reader.AtEnd)
        {
            var (field, wire) = reader.Tag();
            if (field == 1 && wire == 2) currency = reader.String();
            else if (field == 2 && wire == 2) amount = reader.String();
            else reader.Skip(wire);
        }
        return value with { Kind = ContentKind.Money, Currency = currency, Text = amount };
    }

    private static WidgetSummary Summary(byte[] bytes)
    {
        var reader = new WireReader(bytes);
        var summary = new WidgetSummary();
        while (!reader.AtEnd)
        {
            var (field, wire) = reader.Tag();
            summary = (field, wire) switch
            {
                (1, 2) => summary with { Title = reader.String() },
                (2, 2) => summary with { PrimaryLabel = reader.String() },
                (3, 2) => summary with { PrimaryValue = Metric(reader.Bytes()) },
                (4, 2) => summary with { SecondaryLabel = reader.String() },
                (5, 2) => summary with { SecondaryValue = Metric(reader.Bytes()) },
                (6, 2) => summary with { DataUpdatedAt = Timestamp(reader.Bytes()) },
                _ => Skip(reader, wire, summary),
            };
        }
        return summary;
    }

    public static NativeWidgetSnapshot Snapshot(byte[] bytes)
    {
        var reader = new WireReader(bytes);
        var snapshot = new NativeWidgetSnapshot();
        while (!reader.AtEnd)
        {
            var (field, wire) = reader.Tag();
            snapshot = (field, wire) switch
            {
                (1, 2) => snapshot with { DashboardId = reader.String() },
                (2, 2) => snapshot with { WidgetId = reader.String() },
                (3, 2) => snapshot with { SpaceId = reader.String() },
                (4, 0) => snapshot with { SharingState = (SharingState)(int)reader.Varint() },
                (5, 2) => snapshot with { Summary = Summary(reader.Bytes()) },
                (6, 2) => snapshot with { RefreshedAt = Timestamp(reader.Bytes()) },
                _ => Skip(reader, wire, snapshot),
            };
        }
        // ENDED snapshots never display data, even if a summary was present.
        return snapshot.SharingState == SharingState.Ended ? snapshot with { Summary = null } : snapshot;
    }

    /// <summary>ListNativeWidgetSnapshotsResponse: field 1 repeats NativeWidgetSnapshot.</summary>
    public static List<NativeWidgetSnapshot> SnapshotList(byte[] bytes)
    {
        var reader = new WireReader(bytes);
        var snapshots = new List<NativeWidgetSnapshot>();
        while (!reader.AtEnd)
        {
            var (field, wire) = reader.Tag();
            if (field == 1 && wire == 2) snapshots.Add(Snapshot(reader.Bytes()));
            else reader.Skip(wire);
        }
        return snapshots;
    }
}

/// <summary>English display strings matching the design record's native widget and the other hosts.</summary>
public static class WidgetText
{
    public const string SharingEnded = "Sharing ended";
    public const string SharingEndedDetail = "Open AnyDash to choose another widget.";
    public const string NotProvided = "Not provided";
    public const string Empty = "Choose a widget in AnyDash.";

    public static string Time(DateTimeOffset at, DateTimeOffset now, TimeZoneInfo zone)
    {
        var local = TimeZoneInfo.ConvertTime(at, zone);
        var today = TimeZoneInfo.ConvertTime(now, zone);
        return local.ToString(local.Date == today.Date ? "HH:mm" : "MMM d, HH:mm", CultureInfo.InvariantCulture);
    }

    public static string Updated(DateTimeOffset at, DateTimeOffset now, TimeZoneInfo zone) => $"Updated {Time(at, now, zone)}";

    public static string Value(MetricValue value, DateTimeOffset now, TimeZoneInfo zone)
    {
        if (value.State == MetricState.NotProvided) return NotProvided;
        return value.Kind switch
        {
            ContentKind.Percent => $"{Math.Round(value.Number, MidpointRounding.AwayFromZero).ToString(CultureInfo.InvariantCulture)}%",
            ContentKind.Decimal => value.Text,
            ContentKind.Integer => ((long)value.Number).ToString("N0", CultureInfo.InvariantCulture),
            ContentKind.Money => Money(value),
            ContentKind.Time => value.Time is { } time ? Time(time, now, zone) : "",
            ContentKind.Duration => value.Number >= 86400 ? $"{(int)(value.Number / 86400)} days" : $"{(int)(value.Number / 3600)} hours",
            ContentKind.Text => value.Text,
            ContentKind.Flag => value.Flag ? "On" : "Off",
            _ => "",
        };
    }

    private static string Money(MetricValue value)
    {
        var text = decimal.TryParse(value.Text, NumberStyles.Number, CultureInfo.InvariantCulture, out var amount)
            ? Math.Round(amount, 2, MidpointRounding.AwayFromZero).ToString("0.00", CultureInfo.InvariantCulture)
            : value.Text;
        return value.Currency switch
        {
            "USD" => $"${text}",
            "" => text,
            _ => $"{text} {value.Currency}",
        };
    }

    /// <summary>"left · Resets 14:20" from the summary labels.</summary>
    public static string Supporting(WidgetSummary summary, DateTimeOffset now, TimeZoneInfo zone)
    {
        var secondary = string.Join(" ", new[] { summary.SecondaryLabel, Value(summary.Secondary, now, zone) }.Where(part => part.Length > 0));
        return string.Join(" · ", new[] { summary.PrimaryLabel, secondary }.Where(part => part.Length > 0));
    }

    /// <summary>Narrator reads the state label, then the value, then the update time.</summary>
    public static string Accessibility(NativeWidgetSnapshot snapshot, string title, DateTimeOffset now, TimeZoneInfo zone)
    {
        if (snapshot.Summary is not { } summary || snapshot.SharingState == SharingState.Ended)
            return $"{title}. {SharingEnded}. {SharingEndedDetail}";
        var state = summary.Primary.State switch
        {
            MetricState.Failed => "Couldn’t refresh. ",
            MetricState.AuthExpired => "Authentication expired. ",
            _ => "",
        };
        var updated = (summary.DataUpdatedAt ?? snapshot.RefreshedAt) is { } at ? $". {Updated(at, now, zone)}" : "";
        return $"{summary.Title}. {state}{Value(summary.Primary, now, zone)} {Supporting(summary, now, zone)}{updated}";
    }

    public static bool Stale(MetricValue value) => value.State is MetricState.Failed or MetricState.AuthExpired;
}
