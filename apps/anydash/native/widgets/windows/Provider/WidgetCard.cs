// SPDX-License-Identifier: Apache-2.0
// Adaptive Card template and data for the Widgets Board. The layout follows
// the design record's native widget: "{Provider} · {metric}", the primary
// value, one supporting line and "Updated {time}". Selecting the card opens
// the widget's anydash:// deep link in the desktop app.
using System.Text.Json;
using System.Text.Json.Nodes;

namespace AnyDash.Widgets;

public static class WidgetCard
{
    public const string Template = """
{
  "type": "AdaptiveCard",
  "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
  "version": "1.5",
  "speak": "${speak}",
  "selectAction": { "type": "Action.OpenUrl", "url": "${link}", "$when": "${link != ''}" },
  "body": [
    { "type": "TextBlock", "text": "${title}", "size": "small", "weight": "bolder", "isSubtle": true, "wrap": false },
    { "type": "TextBlock", "text": "${value}", "size": "${valueSize}", "weight": "bolder", "isSubtle": "${muted}", "spacing": "small", "wrap": false, "$when": "${value != ''}" },
    { "type": "TextBlock", "text": "${supporting}", "size": "small", "isSubtle": true, "spacing": "small", "wrap": true, "maxLines": 2 },
    { "type": "TextBlock", "text": "${updated}", "size": "small", "isSubtle": true, "spacing": "medium", "$when": "${updated != ''}" }
  ]
}
""";

    /// <summary>Lists the snapshots to choose from during Widgets Board customization.</summary>
    public const string ChooserTemplate = """
{
  "type": "AdaptiveCard",
  "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
  "version": "1.5",
  "body": [
    { "type": "TextBlock", "text": "${heading}", "weight": "bolder", "wrap": true },
    { "type": "TextBlock", "text": "${empty}", "isSubtle": true, "wrap": true, "$when": "${count(choices) == 0}" },
    { "type": "ActionSet", "$data": "${choices}", "actions": [ { "type": "Action.Execute", "title": "${title}", "verb": "select", "data": { "widgetId": "${id}" } } ] }
  ]
}
""";

    public const string ChooseWidget = "Choose a widget";

    public static string Data(NativeWidgetSnapshot? snapshot, string rememberedTitle, DateTimeOffset now, TimeZoneInfo zone)
    {
        var data = new JsonObject();
        if (snapshot is null)
        {
            data["title"] = "AnyDash";
            data["value"] = "";
            data["supporting"] = WidgetText.Empty;
            data["updated"] = "";
            data["link"] = "";
            data["speak"] = WidgetText.Empty;
        }
        else if (snapshot.Summary is not { } summary || snapshot.SharingState == SharingState.Ended)
        {
            // Sharing ended: no value or time, only the state and what to do next.
            data["title"] = rememberedTitle;
            data["value"] = WidgetText.SharingEnded;
            data["valueSize"] = "medium";
            data["muted"] = true;
            data["supporting"] = WidgetText.SharingEndedDetail;
            data["updated"] = "";
            data["link"] = snapshot.DeepLink;
            data["speak"] = WidgetText.Accessibility(snapshot, rememberedTitle, now, zone);
        }
        else
        {
            var notProvided = summary.Primary.State == MetricState.NotProvided;
            var updatedAt = summary.DataUpdatedAt ?? snapshot.RefreshedAt;
            data["title"] = summary.Title;
            data["value"] = WidgetText.Value(summary.Primary, now, zone);
            data["valueSize"] = notProvided ? "medium" : "extraLarge";
            // Adaptive Cards have no opacity; subtle text stands in for the
            // design's dimmed stale value next to the failure state.
            data["muted"] = notProvided || WidgetText.Stale(summary.Primary);
            data["supporting"] = WidgetText.Supporting(summary, now, zone);
            data["updated"] = updatedAt is { } at ? WidgetText.Updated(at, now, zone) : "";
            data["link"] = snapshot.DeepLink;
            data["speak"] = WidgetText.Accessibility(snapshot, summary.Title, now, zone);
        }
        data["valueSize"] ??= "extraLarge";
        data["muted"] ??= false;
        return data.ToJsonString();
    }

    public static string ChooserData(IEnumerable<NativeWidgetSnapshot> snapshots, Func<string, string> title)
    {
        var choices = new JsonArray();
        foreach (var snapshot in snapshots)
            choices.Add(new JsonObject { ["id"] = snapshot.WidgetId, ["title"] = snapshot.Summary?.Title ?? title(snapshot.WidgetId) });
        return new JsonObject { ["heading"] = ChooseWidget, ["empty"] = WidgetText.Empty, ["choices"] = choices }.ToJsonString();
    }

    public static string? SelectedWidgetId(string? actionData)
    {
        if (string.IsNullOrEmpty(actionData)) return null;
        try
        {
            return JsonNode.Parse(actionData)?["widgetId"]?.GetValue<string>();
        }
        catch (JsonException)
        {
            return null;
        }
    }
}
