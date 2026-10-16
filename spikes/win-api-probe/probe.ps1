# Win32 assumption probe for hyprwin (Step 0).
#
# Verifies on the *current* Windows build, against a window owned by ANOTHER process:
#   1. DwmSetWindowAttribute(DWMWA_CLOAK) on a foreign window  -> expected E_ACCESSDENIED
#   2. IApplicationView::SetCloak via IApplicationViewCollection -> expected to work
#   3. Cloaked window is actually invisible on screen (pixel check)
#   4. DwmRegisterThumbnail of a CLOAKED source still renders  (basis of the animation plan)
#   5. Invisible-border size: GetWindowRect vs DWMWA_EXTENDED_FRAME_BOUNDS
#   6. DWMWA_BORDER_COLOR / DWMWA_WINDOW_CORNER_PREFERENCE on a foreign window
#   7. Registered IIDs for virtual-desktop internals on this build (read-only registry lookup)
#
# Side effects: spawns one throwaway red test window in a child process, cloaks/uncloaks it,
# closes it. Always uncloaks in `finally`. No registry writes, no system settings touched.
# Run with: powershell -NoProfile -ExecutionPolicy Bypass -File probe.ps1

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

[StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }

[StructLayout(LayoutKind.Sequential)]
public struct DWM_THUMBNAIL_PROPERTIES {
    public uint dwFlags; public RECT rcDestination; public RECT rcSource;
    public byte opacity; public int fVisible; public int fSourceClientAreaOnly;
}

[ComImport, Guid("6D5140C1-7436-11CE-8034-00AA006009FA"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IServiceProvider10 {
    [PreserveSig] int QueryService(ref Guid service, ref Guid riid, [MarshalAs(UnmanagedType.IUnknown)] out object obj);
}

// Undocumented. Only the vtable slot position matters; m* are placeholders.
[ComImport, Guid("1841C6D7-4F9D-42C0-AF41-8747538F10E5"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IApplicationViewCollection {
    [PreserveSig] int m1(); [PreserveSig] int m2(); [PreserveSig] int m3();
    [PreserveSig] int GetViewForHwnd(IntPtr hwnd, out IApplicationView view);
}

// IApplicationView : IInspectable (3) + SetFocus, SwitchTo, TryInvokeBack, GetThumbnailWindow,
// GetMonitor, GetVisibility (6) -> SetCloak is slot 10 after IUnknown.
[ComImport, Guid("372E1D3B-38D3-42E4-A15B-8AB2B178F513"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IApplicationView {
    [PreserveSig] int m1(); [PreserveSig] int m2(); [PreserveSig] int m3();
    [PreserveSig] int m4(); [PreserveSig] int m5(); [PreserveSig] int m6();
    [PreserveSig] int m7(); [PreserveSig] int m8(); [PreserveSig] int m9();
    [PreserveSig] int SetCloak(int cloakType, int flags);
}

public static class N {
    [DllImport("dwmapi.dll")] public static extern int DwmSetWindowAttribute(IntPtr h, int a, ref int v, int s);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT v, int s);
    [DllImport("dwmapi.dll")] public static extern int DwmRegisterThumbnail(IntPtr dst, IntPtr src, out IntPtr thumb);
    [DllImport("dwmapi.dll")] public static extern int DwmUnregisterThumbnail(IntPtr thumb);
    [DllImport("dwmapi.dll")] public static extern int DwmUpdateThumbnailProperties(IntPtr thumb, ref DWM_THUMBNAIL_PROPERTIES p);
    [DllImport("dwmapi.dll")] public static extern int DwmFlush();
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint f);
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);

    static readonly Guid CLSID_ImmersiveShell = new Guid("C2F03A33-21F5-47FA-B4BB-156362A2F239");

    public static int SetCloakViaView(IntPtr hwnd, bool cloak) {
        var t = Type.GetTypeFromCLSID(CLSID_ImmersiveShell);
        var sp = (IServiceProvider10)Activator.CreateInstance(t);
        Guid g = typeof(IApplicationViewCollection).GUID; object o;
        int hr = sp.QueryService(ref g, ref g, out o);
        if (hr != 0) return hr;
        var coll = (IApplicationViewCollection)o;
        IApplicationView view;
        hr = coll.GetViewForHwnd(hwnd, out view);
        if (hr != 0 || view == null) return hr != 0 ? hr : -1;
        return view.SetCloak(1, cloak ? 2 : 0);
    }
}
'@

# Per-monitor-v2 so coordinates are physical pixels.
[void][N]::SetProcessDpiAwarenessContext([IntPtr](-4))

$results = [ordered]@{}
$results.os = (Get-CimInstance Win32_OperatingSystem | ForEach-Object { "$($_.Caption) $($_.Version) build $($_.BuildNumber)" })
$results.ubr = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').UBR
$results.displayVersion = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').DisplayVersion

$title = "hyprwin-probe-target-$PID"
$targetScript = @"
Add-Type -AssemblyName System.Windows.Forms
`$f = New-Object Windows.Forms.Form
`$f.Text = '$title'; `$f.BackColor = [Drawing.Color]::FromArgb(255,0,0)
`$f.StartPosition = 'Manual'; `$f.Location = New-Object Drawing.Point(120,120); `$f.Size = New-Object Drawing.Size(400,300)
`$f.TopMost = `$true
[void]`$f.ShowDialog()
"@
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($targetScript))
$proc = Start-Process powershell -ArgumentList '-NoProfile', '-WindowStyle', 'Hidden', '-EncodedCommand', $encoded -PassThru

function Hex($hr) { '0x{0:X8}' -f ($hr -band 0xFFFFFFFF) }
function Pixel($x, $y) {
    $bmp = New-Object Drawing.Bitmap 1, 1
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($x, $y, 0, 0, (New-Object Drawing.Size 1, 1))
    $c = $bmp.GetPixel(0, 0); $g.Dispose(); $bmp.Dispose()
    return $c
}
function IsRed($c) { $c.R -gt 200 -and $c.G -lt 60 -and $c.B -lt 60 }

$host_form = $null; $thumb = [IntPtr]::Zero; $hwnd = [IntPtr]::Zero
try {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($hwnd -eq [IntPtr]::Zero -and $sw.ElapsedMilliseconds -lt 15000) {
        Start-Sleep -Milliseconds 100; $hwnd = [N]::FindWindow([NullString]::Value, $title)
    }
    if ($hwnd -eq [IntPtr]::Zero) { throw "target window never appeared" }
    Start-Sleep -Milliseconds 500

    # 5. invisible borders
    $wr = New-Object RECT; [void][N]::GetWindowRect($hwnd, [ref]$wr)
    $fr = New-Object RECT; [void][N]::DwmGetWindowAttribute($hwnd, 9, [ref]$fr, 16)
    $results.windowRect = "$($wr.L),$($wr.T),$($wr.R),$($wr.B)"
    $results.extendedFrameBounds = "$($fr.L),$($fr.T),$($fr.R),$($fr.B)"
    $results.invisibleBorder_LTRB = "$($fr.L-$wr.L),$($fr.T-$wr.T),$($wr.R-$fr.R),$($wr.B-$fr.B)"

    $cx = [int](($fr.L + $fr.R) / 2); $cy = [int](($fr.T + $fr.B) / 2)
    $results.visibleBeforeCloak = IsRed (Pixel $cx $cy)

    # 6. border colour + corner preference on foreign window
    $color = 0x0000FF00  # COLORREF 0x00BBGGRR -> green
    $results.borderColorHr = Hex ([N]::DwmSetWindowAttribute($hwnd, 34, [ref]$color, 4))
    $corner = 1          # DWMWCP_DONOTROUND
    $results.cornerPrefHr = Hex ([N]::DwmSetWindowAttribute($hwnd, 33, [ref]$corner, 4))

    # 1. documented cloak on foreign window
    $one = 1
    $results.dwmCloakForeignHr = Hex ([N]::DwmSetWindowAttribute($hwnd, 13, [ref]$one, 4))

    # 2. undocumented IApplicationView cloak
    $results.appViewCloakHr = Hex ([N]::SetCloakViaView($hwnd, $true))
    Start-Sleep -Milliseconds 300; [void][N]::DwmFlush()
    $cl = 0; [void][N]::DwmGetWindowAttribute($hwnd, 14, [ref]$cl, 4)
    $results.dwmwaCloakedAfter = $cl   # 2 = DWM_CLOAKED_SHELL
    # 3. pixel check
    $results.visibleWhileCloaked = IsRed (Pixel $cx $cy)

    # 4. thumbnail of cloaked source into our own topmost host window
    $host_form = New-Object Windows.Forms.Form
    $host_form.FormBorderStyle = 'None'; $host_form.TopMost = $true; $host_form.ShowInTaskbar = $false
    $host_form.StartPosition = 'Manual'; $host_form.BackColor = [Drawing.Color]::Black
    $host_form.Location = New-Object Drawing.Point(($fr.R + 40), $fr.T)
    $host_form.Size = New-Object Drawing.Size(300, 200)
    $host_form.Show(); [Windows.Forms.Application]::DoEvents()
    $results.thumbRegisterHr = Hex ([N]::DwmRegisterThumbnail($host_form.Handle, $hwnd, [ref]$thumb))
    $p = New-Object DWM_THUMBNAIL_PROPERTIES
    $p.dwFlags = 0x1 -bor 0x4 -bor 0x8
    $p.rcDestination = New-Object RECT -Property @{ L = 0; T = 0; R = 300; B = 200 }
    $p.opacity = 255; $p.fVisible = 1
    $results.thumbUpdateHr = Hex ([N]::DwmUpdateThumbnailProperties($thumb, [ref]$p))
    for ($i = 0; $i -lt 10; $i++) { [Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 50 }
    [void][N]::DwmFlush()
    $hl = $host_form.Location
    $results.thumbnailOfCloakedRendersRed = IsRed (Pixel ($hl.X + 150) ($hl.Y + 100))

    # timing: 120 thumbnail moves, as an animation would do
    $sw = [Diagnostics.Stopwatch]::StartNew()
    for ($i = 0; $i -lt 120; $i++) {
        $p.dwFlags = 0x1; $p.rcDestination = New-Object RECT -Property @{ L = $i; T = 0; R = 180 + $i; B = 120 }
        [void][N]::DwmUpdateThumbnailProperties($thumb, [ref]$p)
    }
    $results.thumbUpdate120x_ms = $sw.Elapsed.TotalMilliseconds

    # timing: SetWindowPos on a foreign window (cloaked; no repaint visible)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    for ($i = 0; $i -lt 60; $i++) { [void][N]::SetWindowPos($hwnd, [IntPtr]::Zero, 120 + $i, 120, 400, 300, 0x0014) }
    $results.setWindowPos60x_ms = $sw.Elapsed.TotalMilliseconds
}
catch { $results.error = $_.ToString() }
finally {
    if ($thumb -ne [IntPtr]::Zero) { [void][N]::DwmUnregisterThumbnail($thumb) }
    if ($host_form) { $host_form.Close() }
    if ($hwnd -ne [IntPtr]::Zero) {
        $results.uncloakHr = Hex ([N]::SetCloakViaView($hwnd, $false))
        $cl = 0; [void][N]::DwmGetWindowAttribute($hwnd, 14, [ref]$cl, 4); $results.cloakedAfterRestore = $cl
    }
    if ($proc -and -not $proc.HasExited) { $proc.Kill() }
}

# 7. virtual desktop interface IIDs registered on this build (read-only)
$vd = @{}
Get-ChildItem 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Classes\Interface' -ErrorAction SilentlyContinue | ForEach-Object {
    $n = $_.GetValue('')
    if ($n -match '^I(VirtualDesktop|ApplicationView)') { $vd[$n] = $_.PSChildName }
}
$results.registeredShellIIDs = $vd

$results | ConvertTo-Json -Depth 4
