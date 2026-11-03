// SPDX-License-Identifier: Apache-2.0
// COM class factory for the widget provider, following the Windows App SDK
// widget provider walkthrough.
using System.Runtime.InteropServices;
using Microsoft.Windows.Widgets.Providers;
using WinRT;

namespace AnyDash.Widgets.Com;

[ComImport, ComVisible(false), InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("00000001-0000-0000-C000-000000000046")]
internal interface IClassFactory
{
    [PreserveSig]
    int CreateInstance(IntPtr pUnkOuter, ref Guid riid, out IntPtr ppvObject);

    [PreserveSig]
    int LockServer(bool fLock);
}

[ComVisible(true)]
internal sealed class WidgetProviderFactory<T> : IClassFactory where T : IWidgetProvider, new()
{
    private const int ClassENoAggregation = -2147221232;

    public int CreateInstance(IntPtr pUnkOuter, ref Guid riid, out IntPtr ppvObject)
    {
        ppvObject = IntPtr.Zero;
        if (pUnkOuter != IntPtr.Zero) return ClassENoAggregation;
        var provider = IntPtr.Zero;
        try
        {
            provider = MarshalInspectable<IWidgetProvider>.FromManaged(new T());
            return Marshal.QueryInterface(provider, in riid, out ppvObject);
        }
        catch (Exception exception)
        {
            return Marshal.GetHRForException(exception);
        }
        finally
        {
            if (provider != IntPtr.Zero) Marshal.Release(provider);
        }
    }

    int IClassFactory.LockServer(bool fLock) => 0;
}
