# Live test: hyprwin:hide_taskbar. Hides the real taskbar for ~15 s and always
# shows it again at the end (also if a check fails).
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\taskbar.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public struct TR { public int L, T, R, B; }
public struct TP { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct MI { public int dx, dy; public uint data, flags, time; public IntPtr extra; }
[StructLayout(LayoutKind.Sequential)] public struct IN { public uint type; public MI mi; }
public static class T8 {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out TR r);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out TR r, int s);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll")] public static extern uint SendInput(uint n, IN[] i, int size);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out TP p);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
  public static string FgClass() { var sb = new System.Text.StringBuilder(256); GetClassName(GetForegroundWindow(), sb, 256); return sb.ToString(); }
  public static void Tap(byte vk) { keybd_event(vk, 0, 0, IntPtr.Zero); keybd_event(vk, 0, 2, IntPtr.Zero); }
  public static void MoveTo(int x, int y) {
    int vx = GetSystemMetrics(76), vy = GetSystemMetrics(77), vw = GetSystemMetrics(78), vh = GetSystemMetrics(79);
    var i = new IN[1]; i[0].type = 0;
    i[0].mi.dx = (int)(((long)(x - vx) * 65535) / (vw - 1)); i[0].mi.dy = (int)(((long)(y - vy) * 65535) / (vh - 1));
    i[0].mi.flags = 0x0001 | 0x8000 | 0x4000; // MOVE | ABSOLUTE | VIRTUALDESK
    SendInput(1, i, Marshal.SizeOf(typeof(IN)));
  }
}
'@
[void][T8]::SetProcessDpiAwarenessContext([IntPtr](-4))
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null } }
function Wait-Anim { Start-Sleep -Milliseconds 150; $t0 = Get-Date; while ([T8]::FindWindow('hyprwin.ghost', [NullString]::Value) -ne [IntPtr]::Zero -and ((Get-Date) - $t0).TotalMilliseconds -lt 3000) { Start-Sleep -Milliseconds 20 }; Start-Sleep -Milliseconds 300 }
function Bottom($h) { $r = New-Object TR; [void][T8]::DwmGetWindowAttribute($h, 9, [ref]$r, 16); $r.B }

$tray = [T8]::FindWindow('Shell_TrayWnd', [NullString]::Value)
$tr = New-Object TR; [void][T8]::GetWindowRect($tray, [ref]$tr)
$mon = $null
Kill-Hyprwin; Start-Sleep -Milliseconds 300
$state = "$env:TEMP\hyprwin-taskbar-state"
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-taskbar-test.conf"
(Get-Content "$root\crates\hyprwin-config\src\default.conf") -replace 'hide_taskbar = false', 'hide_taskbar = true' | Set-Content $cfg
$env:HYPRWIN_STATE_DIR = $state
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-tb-'
$procs = @()
$procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '60,160,90', 200, 200, 600, 400, 'hyprwin-tb-a' -PassThru
Start-Sleep -Milliseconds 800
$ha = [T8]::FindWindow([NullString]::Value, 'hyprwin-tb-a')
try {
    Write-Host "== hidden at start (taskbar was at $($tr.L),$($tr.T)-$($tr.R),$($tr.B))"
    Check 'taskbar visible before hyprwin starts' ([T8]::IsWindowVisible($tray))
    $d = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
    Start-Sleep -Milliseconds 1500; Wait-Anim
    $mon = (Ctl -j monitors | ConvertFrom-Json) | Where-Object { $_.focused }
    $monBottom = $mon.y + $mon.height
    Check 'taskbar hidden' (-not [T8]::IsWindowVisible($tray))
    Check 'ledger records it (for crash restore)' ((Get-Content "$state\ledger.json" -Raw) -match "$([int64]$tray)")
    $b = Bottom $ha
    $gap = [int](20 * $mon.scale)
    Check 'tiles reach into the taskbar space (bottom = screen bottom - scaled gaps_out)' ($b -eq $monBottom - $gap) "bottom=$b screen=$monBottom gap=$gap"

    Write-Host "`n== reveal at the screen edge"
    $cx = [int](($tr.L + $tr.R) / 2)
    [T8]::MoveTo($cx, $monBottom - 1); Start-Sleep -Milliseconds 120; [T8]::MoveTo($cx + 5, $monBottom - 1)
    Start-Sleep -Milliseconds 500
    $cp = New-Object TP; [void][T8]::GetCursorPos([ref]$cp)
    Check 'test cursor reached the edge' ($cp.Y -ge $monBottom - 2) "cursor=$($cp.X),$($cp.Y)"
    Check 'touching the edge shows the taskbar' ([T8]::IsWindowVisible($tray))
    [T8]::MoveTo($cx, [int]($tr.T + ($tr.B - $tr.T) / 2)); Start-Sleep -Milliseconds 900
    Check 'stays while the mouse is on it' ([T8]::IsWindowVisible($tray))
    [T8]::MoveTo($cx, [int]($mon.y + $mon.height / 2)); Start-Sleep -Milliseconds 1200
    Check 'hides again after the mouse leaves' (-not [T8]::IsWindowVisible($tray))
    Check 'windows did not move during the reveal' ((Bottom $ha) -eq $monBottom - $gap)

    Write-Host "`n== Start menu (Win key) while hidden"
    [T8]::Tap(0x5B); Start-Sleep -Milliseconds 900
    $fg = [T8]::FgClass()
    Check 'Win key still opens Start' ($fg -match 'CoreWindow|XamlExplorerHost|Shell_TrayWnd') "foreground=$fg"
    # Start opens on its own; the taskbar stays hidden (touch the edge for the tray).
    [T8]::Tap(0x1B); Start-Sleep -Milliseconds 1000
    Check 'taskbar still hidden after Start closes' (-not [T8]::IsWindowVisible($tray))
    Check 'Start closed again' ([T8]::FgClass() -notmatch 'CoreWindow') "foreground=$([T8]::FgClass())"

    Write-Host "`n== toggletaskbar"
    Ctl dispatch toggletaskbar | Out-Null; Wait-Anim
    Check 'toggle shows it' ([T8]::IsWindowVisible($tray))
    $b = Bottom $ha
    Check 'tiles shrink back above it' ($b -eq $tr.T - $gap) "bottom=$b taskbar top=$($tr.T)"
    Check 'ledger no longer lists it' (-not ((Get-Content "$state\ledger.json" -Raw) -match "$([int64]$tray)"))
    Ctl dispatch toggletaskbar | Out-Null; Wait-Anim
    Check 'toggle hides it again' (-not [T8]::IsWindowVisible($tray))

    Write-Host "`n== pause gives it back, resume hides it"
    Ctl pause | Out-Null; Start-Sleep -Milliseconds 600
    Check 'visible while paused' ([T8]::IsWindowVisible($tray))
    Ctl resume | Out-Null; Start-Sleep -Milliseconds 1200
    Check 'hidden after resume' (-not [T8]::IsWindowVisible($tray))

    Write-Host "`n== crash: the guard restores the taskbar"
    Get-Process hyprwin -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null }
    Start-Sleep -Milliseconds 2500
    Check 'taskbar visible after the daemon was killed' ([T8]::IsWindowVisible($tray))
}
finally {
    Kill-Hyprwin
    Start-Sleep -Milliseconds 400
    # Whatever happened above, never leave the user without a taskbar.
    foreach ($c in 'Shell_TrayWnd', 'Shell_SecondaryTrayWnd') {
        $h = [T8]::FindWindow($c, [NullString]::Value)
        if ($h -ne [IntPtr]::Zero) { [void][T8]::ShowWindow($h, 8) }
    }
    $procs | ForEach-Object { if (-not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
