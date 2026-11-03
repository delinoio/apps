// SPDX-License-Identifier: Apache-2.0
// COM server entry point. The Widgets Board activates this executable through
// the package manifest's CLSID; it exits once no AnyDash widget stays pinned.
using System.Diagnostics;
using System.Runtime.InteropServices;
using AnyDash.Widgets;
using AnyDash.Widgets.Com;

WinRT.ComWrappersSupport.InitializeComWrappers();
// Keep in sync with Package.appxmanifest (com:Class Id and CreateInstance ClassId).
var clsid = Guid.Parse("4F0A2B6E-8C1D-4E5A-9B7F-3D2C1A0E9F61");
var factory = new WidgetProviderFactory<WidgetProvider>();
Marshal.ThrowExceptionForHR(Native.CoRegisterClassObject(clsid, factory, 0x4, 0x1, out var cookie));
Trace.TraceInformation("{\"event\":\"anydash_widget_provider_started\",\"platform\":\"windows\"}");
try
{
    WidgetProvider.NoWidgetsEvent.WaitOne();
}
finally
{
    _ = Native.CoRevokeClassObject(cookie);
    GC.KeepAlive(factory);
}

internal static class Native
{
    [DllImport("ole32.dll")]
    public static extern int CoRegisterClassObject([MarshalAs(UnmanagedType.LPStruct)] Guid rclsid,
        [MarshalAs(UnmanagedType.IUnknown)] object pUnk, uint dwClsContext, uint flags, out uint lpdwRegister);

    [DllImport("ole32.dll")]
    public static extern int CoRevokeClassObject(uint dwRegister);
}
