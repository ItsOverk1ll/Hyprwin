# Live test: the system pulling the rug. Explorer crashes and restarts (the
# shell interfaces hyprwin uses to hide windows die with it, the tray icon and
# taskbar are recreated), and the screen resolution changes on the fly.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\system.ps1
#
# Restarts explorer.exe and changes the display mode (restored at the end).

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
public struct DEVMODE {
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
  public short dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra; public int dmFields;
  public int dmPositionX, dmPositionY, dmDisplayOrientation, dmDisplayFixedOutput;
  public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
  public short dmLogPixels; public int dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
  public int dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2, dmPanningWidth, dmPanningHeight;
}
public struct NII { public uint cbSize; public IntPtr hWnd; public uint uID; public Guid guidItem; }
public struct RC8 { public int L, T, R, B; }
public static class S8 {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool EnumDisplaySettings(string dev, int mode, ref DEVMODE dm);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int ChangeDisplaySettings(ref DEVMODE dm, int flags);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int ChangeDisplaySettings(IntPtr dm, int flags);
  [DllImport("shell32.dll")] public static extern int Shell_NotifyIconGetRect(ref NII id, out RC8 r);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
}
'@
[void][S8]::SetProcessDpiAwarenessContext([IntPtr](-4))
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Cloaked($h) { $v = 0; [void][S8]::DwmGetWindowAttribute($h, 14, [ref]$v, 4); $v }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null } }
function Alive($p) { $p.Refresh(); -not $p.HasExited }
function Hwnd($t) { [S8]::FindWindow([NullString]::Value, $t) }
function Consistent {
    # Every managed window is shown/hidden as the model says.
    $bad = @()
    foreach ($w in (Ctl -j clients | ConvertFrom-Json)) {
        $h = [IntPtr][Convert]::ToInt64($w.address, 16)
        $hidden = (Cloaked $h) -ne 0
        if ($w.hidden -and -not $hidden) { $bad += "$($w.title) visible but on hidden ws $($w.workspace.id)" }
        if (-not $w.hidden -and $hidden) { $bad += "$($w.title) cloaked but on shown ws $($w.workspace.id)" }
    }
    $bad
}
function TrayIcon {
    $main = [S8]::FindWindow('hyprwin.main', [NullString]::Value)
    $id = New-Object NII; $id.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($id); $id.hWnd = $main; $id.uID = 1
    $r = New-Object RC8
    [S8]::Shell_NotifyIconGetRect([ref]$id, [ref]$r) -eq 0
}

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$state = "$env:TEMP\hyprwin-system-state"
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-system.conf"
Get-Content "$root\crates\hyprwin-config\src\default.conf" | Set-Content $cfg
$env:HYPRWIN_STATE_DIR = $state
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-sys-'
$procs = @()
foreach ($i in 1..4) { $procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '90,90,180', (60 * $i), 80, 500, 350, "hyprwin-sys-$i" -PassThru }
Start-Sleep -Milliseconds 1000
$d = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1800
$dm0 = New-Object DEVMODE; $dm0.dmSize = [Runtime.InteropServices.Marshal]::SizeOf($dm0)
[void][S8]::EnumDisplaySettings([NullString]::Value, -1, [ref]$dm0)   # ENUM_CURRENT_SETTINGS
try {
    Ctl dispatch focuswindow title:hyprwin-sys-3 | Out-Null
    Ctl dispatch movetoworkspacesilent 2 | Out-Null
    Ctl dispatch focuswindow title:hyprwin-sys-4 | Out-Null
    Ctl dispatch movetoworkspacesilent 2 | Out-Null
    Start-Sleep -Milliseconds 800
    Check 'setup: two windows hidden on workspace 2' ((Cloaked (Hwnd 'hyprwin-sys-3')) -ne 0 -and (Cloaked (Hwnd 'hyprwin-sys-4')) -ne 0)

    Write-Host "== explorer.exe crashes and restarts"
    Get-Process explorer -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 1500
    Check 'daemon survives explorer dying' (Alive $d)
    $bad = Consistent
    Check 'hidden windows stay hidden while explorer is gone' ($bad.Count -eq 0) ($bad -join '; ')
    if (-not (Get-Process explorer -ErrorAction SilentlyContinue)) { Start-Process explorer.exe }
    $t0 = Get-Date; while (-not (TrayIcon) -and ((Get-Date) - $t0).TotalSeconds -lt 20) { Start-Sleep -Milliseconds 300 }
    Check 'tray icon comes back with the new explorer' (TrayIcon) "$([int]((Get-Date) - $t0).TotalSeconds) s"
    Start-Sleep -Milliseconds 1500
    $bad = Consistent
    Check 'model matches the screen after the restart' ($bad.Count -eq 0) ($bad -join '; ')
    Ctl dispatch workspace 2 | Out-Null; Start-Sleep -Milliseconds 900
    Check 'switching workspaces still hides and shows (new shell interfaces)' ((Cloaked (Hwnd 'hyprwin-sys-3')) -eq 0 -and (Cloaked (Hwnd 'hyprwin-sys-1')) -ne 0) "sys-3=$(Cloaked (Hwnd 'hyprwin-sys-3')) sys-1=$(Cloaked (Hwnd 'hyprwin-sys-1'))"
    Ctl dispatch workspace 1 | Out-Null; Start-Sleep -Milliseconds 900
    $bad = Consistent
    Check 'and back' ($bad.Count -eq 0) ($bad -join '; ')

    Write-Host "`n== the screen resolution changes"
    $modes = @(); $i = 0; $m = New-Object DEVMODE; $m.dmSize = [Runtime.InteropServices.Marshal]::SizeOf($m)
    while ([S8]::EnumDisplaySettings([NullString]::Value, $i, [ref]$m)) {
        if ($m.dmBitsPerPel -eq $dm0.dmBitsPerPel -and ($m.dmPelsWidth -ne $dm0.dmPelsWidth -or $m.dmPelsHeight -ne $dm0.dmPelsHeight)) { $modes += $m }
        $i++; $m = New-Object DEVMODE; $m.dmSize = [Runtime.InteropServices.Marshal]::SizeOf($m)
    }
    $target = $modes | Sort-Object { [math]::Abs($_.dmPelsWidth - 1280) + [math]::Abs($_.dmPelsHeight - 1024) } | Select-Object -First 1
    if (-not $target) {
        Write-Host "  (only one display mode available here; skipped)"
    } else {
        $target.dmFields = 0x00180000   # DM_PELSWIDTH | DM_PELSHEIGHT
        $r = [S8]::ChangeDisplaySettings([ref]$target, 0)
        Write-Host "  switched to $($target.dmPelsWidth)x$($target.dmPelsHeight): $r"
        Start-Sleep -Milliseconds 2500
        $mon = (Ctl -j monitors | ConvertFrom-Json)[0]
        Check 'daemon picked up the new resolution' ($mon.width -eq $target.dmPelsWidth -and $mon.height -eq $target.dmPelsHeight) "monitor=$($mon.width)x$($mon.height)"
        $c = Ctl -j clients | ConvertFrom-Json | Where-Object { -not $_.hidden }
        $right = ($c | ForEach-Object { $_.at[0] + $_.size[0] } | Measure-Object -Maximum).Maximum
        Check 'tiles fill the new width' ([math]::Abs($right - ($mon.width - 20)) -le 2) "right edge=$right width=$($mon.width)"
        $bad = Consistent
        Check 'nothing shown or hidden by mistake' ($bad.Count -eq 0) ($bad -join '; ')
        [void][S8]::ChangeDisplaySettings([IntPtr]::Zero, 0)
        Start-Sleep -Milliseconds 2500
        $mon = (Ctl -j monitors | ConvertFrom-Json)[0]
        Check 'back to the original resolution' ($mon.width -eq $dm0.dmPelsWidth) "monitor=$($mon.width)x$($mon.height)"
    }
    Check 'daemon alive at the end' (Alive $d)
}
finally {
    [void][S8]::ChangeDisplaySettings([IntPtr]::Zero, 0)
    if (-not (Get-Process explorer -ErrorAction SilentlyContinue)) { Start-Process explorer.exe }
    Kill-Hyprwin
    Start-Sleep -Milliseconds 600
    $stuck = @(1..4 | Where-Object { $h = Hwnd "hyprwin-sys-$_"; $h -ne [IntPtr]::Zero -and (Cloaked $h) -ne 0 })
    Check 'guard restored every window after the daemon was killed' ($stuck.Count -eq 0) "stuck: $($stuck -join ',')"
    $procs | ForEach-Object { if ($_ -and -not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
