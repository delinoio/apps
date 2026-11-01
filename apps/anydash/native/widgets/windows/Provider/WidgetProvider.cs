// SPDX-License-Identifier: Apache-2.0
// Windows Widgets Board provider. Lynxtron packaging cannot register a widget
// provider, so this separate MSIX-packaged COM server reads the snapshots the
// resident desktop app writes to the local collector. Without a running
// collector it keeps the last snapshot it read, with that snapshot's time.
using System.Diagnostics;
using System.Text.Json;
using Microsoft.Windows.Widgets.Providers;

namespace AnyDash.Widgets;

internal sealed class PinnedWidget
{
    public required string Id { get; init; }
    public string? SelectedWidgetId { get; set; }
    public string Title { get; set; } = "AnyDash";
    public bool Active { get; set; }
    public bool Customizing { get; set; }
}

internal sealed record SavedState(string? WidgetId, string? Title);

public sealed class WidgetProvider : IWidgetProvider, IWidgetProvider2
{
    public const string DefinitionId = "AnyDash_Widget";
    private static readonly object Gate = new();
    private static readonly Dictionary<string, PinnedWidget> Pinned = new();
    private static readonly ManualResetEvent NoWidgets = new(false);
    private static List<NativeWidgetSnapshot> lastSnapshots = new();
    private static Timer? timer;

    public static ManualResetEvent NoWidgetsEvent => NoWidgets;

    public WidgetProvider()
    {
        foreach (var info in WidgetManager.GetDefault().GetWidgetInfos())
        {
            var saved = Restore(info.CustomState);
            lock (Gate)
                Pinned.TryAdd(info.WidgetContext.Id, new PinnedWidget
                {
                    Id = info.WidgetContext.Id,
                    SelectedWidgetId = saved?.WidgetId,
                    Title = saved?.Title ?? "AnyDash",
                });
        }
    }

    public void CreateWidget(WidgetContext widgetContext)
    {
        var pinned = new PinnedWidget { Id = widgetContext.Id };
        lock (Gate) Pinned[pinned.Id] = pinned;
        Update(pinned, Load());
    }

    public void DeleteWidget(string widgetId, string customState)
    {
        lock (Gate)
        {
            Pinned.Remove(widgetId);
            if (Pinned.Count == 0) NoWidgets.Set();
        }
    }

    public void OnActionInvoked(WidgetActionInvokedArgs actionInvokedArgs)
    {
        if (actionInvokedArgs.Verb != "select") return;
        var id = actionInvokedArgs.WidgetContext.Id;
        PinnedWidget? pinned;
        lock (Gate) Pinned.TryGetValue(id, out pinned);
        if (pinned is null) return;
        pinned.SelectedWidgetId = WidgetCard.SelectedWidgetId(actionInvokedArgs.Data);
        pinned.Customizing = false;
        Update(pinned, Load());
    }

    public void OnWidgetContextChanged(WidgetContextChangedArgs contextChangedArgs)
    {
        PinnedWidget? pinned;
        lock (Gate) Pinned.TryGetValue(contextChangedArgs.WidgetContext.Id, out pinned);
        if (pinned is not null) Update(pinned, Cached());
    }

    public void Activate(WidgetContext widgetContext)
    {
        PinnedWidget? pinned;
        lock (Gate)
        {
            Pinned.TryGetValue(widgetContext.Id, out pinned);
            if (pinned is null) return;
            pinned.Active = true;
            // Refresh every minute while the board shows a widget, matching the app's data cadence.
            timer ??= new Timer(_ => RefreshActive(), null, TimeSpan.FromMinutes(1), TimeSpan.FromMinutes(1));
        }
        Update(pinned, Load());
    }

    public void Deactivate(string widgetId)
    {
        lock (Gate)
        {
            if (Pinned.TryGetValue(widgetId, out var pinned)) pinned.Active = false;
            if (Pinned.Values.All(widget => !widget.Active))
            {
                timer?.Dispose();
                timer = null;
            }
        }
    }

    public void OnCustomizationRequested(WidgetCustomizationRequestedArgs customizationRequestedArgs)
    {
        PinnedWidget? pinned;
        lock (Gate) Pinned.TryGetValue(customizationRequestedArgs.WidgetContext.Id, out pinned);
        if (pinned is null) return;
        pinned.Customizing = true;
        Update(pinned, Load());
    }

    private static void RefreshActive()
    {
        var snapshots = Load();
        List<PinnedWidget> active;
        lock (Gate) active = Pinned.Values.Where(widget => widget.Active).ToList();
        foreach (var pinned in active) Update(pinned, snapshots);
    }

    private static List<NativeWidgetSnapshot> Cached()
    {
        lock (Gate) return lastSnapshots;
    }

    private static List<NativeWidgetSnapshot> Load()
    {
        try
        {
            var snapshots = CollectorClient.ListSnapshots(TimeSpan.FromSeconds(3))
                .OrderByDescending(snapshot => snapshot.RefreshedAt ?? DateTimeOffset.MinValue)
                .ToList();
            lock (Gate) lastSnapshots = snapshots;
            return snapshots;
        }
        catch (Exception error) when (error is CollectorUnavailableException or DecodeException or IOException)
        {
            Trace.TraceWarning("{\"event\":\"anydash_widget_collector_unavailable\",\"platform\":\"windows\"}");
            return Cached();
        }
    }

    private static void Update(PinnedWidget pinned, List<NativeWidgetSnapshot> snapshots)
    {
        var options = new WidgetUpdateRequestOptions(pinned.Id);
        if (pinned.Customizing)
        {
            options.Template = WidgetCard.ChooserTemplate;
            options.Data = WidgetCard.ChooserData(snapshots, _ => pinned.Title);
        }
        else
        {
            var snapshot = pinned.SelectedWidgetId is { } selected
                ? snapshots.FirstOrDefault(entry => entry.WidgetId == selected)
                : snapshots.FirstOrDefault();
            if (snapshot?.Summary?.Title is { } title) pinned.Title = title;
            options.Template = WidgetCard.Template;
            options.Data = WidgetCard.Data(snapshot, pinned.Title, DateTimeOffset.Now, TimeZoneInfo.Local);
        }
        options.CustomState = JsonSerializer.Serialize(new SavedState(pinned.SelectedWidgetId, pinned.Title));
        WidgetManager.GetDefault().UpdateWidget(options);
    }

    private static SavedState? Restore(string? customState)
    {
        if (string.IsNullOrEmpty(customState)) return null;
        try
        {
            return JsonSerializer.Deserialize<SavedState>(customState);
        }
        catch (JsonException)
        {
            return null;
        }
    }
}
