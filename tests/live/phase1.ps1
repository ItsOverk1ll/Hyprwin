# Live end-to-end test for hyprwin Phase 1.
#
# Spawns coloured test windows, runs the real daemon restricted to them
# (HYPRWIN_MANAGE_ONLY), drives it through hyprwinctl, and checks real window
# geometry and cloak state. Your own windows are never touched.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\phase1.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"

Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public struct R { public int L, T, Ri, B; }
public static class W {
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out R r, int s);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
}
'@
[void][W]::SetProcessDpiAwarenessContext([IntPtr](-4))
Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices; using System.Threading;
public static class K {
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  public static void Chord(byte mod, byte key) {
    keybd_event(mod, 0, 0, UIntPtr.Zero); Thread.Sleep(30);
    keybd_event(key, 0, 0, UIntPtr.Zero); Thread.Sleep(30);
    keybd_event(key, 0, 2, UIntPtr.Zero); Thread.Sleep(30);
    keybd_event(mod, 0, 2, UIntPtr.Zero);
  }
  public static string ForegroundClass() { var sb = new StringBuilder(256); GetClassName(GetForegroundWindow(), sb, 256); return sb.ToString(); }
  public static bool StartMenuOpen() { var c = ForegroundClass(); return c == "Windows.UI.Core.CoreWindow" || c == "XamlExplorerHostIslandWindow"; }
}
'@

$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Frame($h) { $r = New-Object R; [void][W]::DwmGetWindowAttribute($h, 9, [ref]$r, 16); "$($r.L),$($r.T),$($r.Ri - $r.L),$($r.B - $r.T)" }
function Cloaked($h) { $v = 0; [void][W]::DwmGetWindowAttribute($h, 14, [ref]$v, 4); $v }
function Ctl { $out = & $ctl @args 2>&1; return ($out -join "`n") }
function Clients { (Ctl -j clients) | ConvertFrom-Json }
function Settle { Start-Sleep -Milliseconds 400 }

# Clean slate.
Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" | Invoke-CimMethod -MethodName Terminate | Out-Null }
Start-Sleep -Milliseconds 300

$targets = @()
$colors = @('230,20,20', '20,40,230', '20,200,40')
for ($i = 0; $i -lt 3; $i++) {
    $title = "hyprwin-test-$i"
    $targets += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', $colors[$i], (100 + 50 * $i), (100 + 50 * $i), 500, 400, $title -PassThru
}
$h = @()
foreach ($i in 0..2) {
    $hw = [IntPtr]::Zero; $t0 = Get-Date
    while ($hw -eq [IntPtr]::Zero -and ((Get-Date) - $t0).TotalSeconds -lt 10) { Start-Sleep -Milliseconds 50; $hw = [W]::FindWindow([NullString]::Value, "hyprwin-test-$i") }
    $h += $hw
}
Write-Host "test windows: $($h -join ', ')"

$cfg = "$env:TEMP\hyprwin-test.conf"
Get-Content "$root\crates\hyprwin-config\src\default.conf" | Set-Content $cfg
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-test-'
$daemon = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1500

try {
    Write-Host "`n== startup"
    $v = Ctl version
    Check 'daemon answers IPC' ($v -match 'hyprwin') $v
    $c = Clients
    Check 'all 3 test windows managed' ($c.Count -eq 3) "count=$($c.Count)"
    Check 'nothing else managed' (($c | Where-Object { $_.title -notmatch '^hyprwin-test-' }).Count -eq 0)

    Write-Host "`n== tiling geometry (dwindle, gaps_in 5, gaps_out 20, scaled by monitor DPI)"
    $mon = (Ctl -j monitors | ConvertFrom-Json) | Where-Object { $_.focused }
    $s = $mon.scale
    $go = [math]::Round(20 * $s); $gi = [math]::Round(5 * $s)
    Settle
    $rects = $h | ForEach-Object { $f = (Frame $_).Split(',') | ForEach-Object { [int]$_ }; , $f }
    foreach ($i in 0..2) { Write-Host "    window $i frame: $($rects[$i] -join ',')   desired: $(($c | Where-Object { $_.title -eq "hyprwin-test-$i" }).at -join ','),$(($c | Where-Object { $_.title -eq "hyprwin-test-$i" }).size -join ',')" }
    foreach ($w in $c) {
        $hw = $h[[int]($w.title -replace '\D', '')]
        $actual = Frame $hw
        $want = "$($w.at[0]),$($w.at[1]),$($w.size[0]),$($w.size[1])"
        Check "frame of $($w.title) is pixel-exact" ($actual -eq $want) "actual=$actual desired=$want"
    }
    $xs = $rects | ForEach-Object { $_[0] }; $ys = $rects | ForEach-Object { $_[1] }
    $work = $mon.reserved
    Check 'outer gap on the left = gaps_out' (($xs | Measure-Object -Minimum).Minimum -eq ($mon.x + $work[0] + $go)) "minX=$(($xs | Measure-Object -Minimum).Minimum) expected=$($mon.x + $work[0] + $go)"
    $sorted = $rects | Sort-Object { $_[0] }
    $gap = $sorted[-1][0] - ($sorted[0][0] + $sorted[0][2])
    Check 'gap between tiles = 2 x gaps_in' ([math]::Abs($gap - 2 * $gi) -le 1) "gap=$gap expected=$(2 * $gi)"

    Write-Host "`n== workspaces"
    $r = Ctl dispatch workspace 2; Settle
    Check 'switch to empty workspace 2' ($r -eq 'ok')
    Check 'all windows cloaked on ws 2' (($h | Where-Object { (Cloaked $_) -ne 0 }).Count -eq 3) "cloaked=$(($h | ForEach-Object { Cloaked $_ }) -join ',')"
    Ctl dispatch workspace 1 | Out-Null; Settle
    Check 'back on ws 1: all visible' (($h | Where-Object { (Cloaked $_) -eq 0 }).Count -eq 3)
    Ctl dispatch focuswindow title:hyprwin-test-0 | Out-Null
    Ctl dispatch movetoworkspacesilent 3 | Out-Null; Settle
    Check 'movetoworkspacesilent hides the window' ((Cloaked $h[0]) -ne 0)
    $c = Clients
    Check 'remaining 2 retile to fill the screen' ((($c | Where-Object { $_.workspace.id -eq 1 }).Count -eq 2))
    Ctl dispatch workspace 3 | Out-Null; Settle
    Check 'ws 3 shows moved window' ((Cloaked $h[0]) -eq 0 -and (Cloaked $h[1]) -ne 0)
    Ctl dispatch movetoworkspace 1 | Out-Null; Settle
    Check 'movetoworkspace follows' (((Ctl -j activeworkspace) | ConvertFrom-Json).id -eq 1 -and (Cloaked $h[0]) -eq 0)

    Write-Host "`n== floating / fullscreen / pseudo / special"
    Ctl dispatch focuswindow title:hyprwin-test-1 | Out-Null
    Ctl dispatch togglefloating | Out-Null; Settle
    $c = Clients
    Check 'togglefloating' (($c | Where-Object { $_.title -eq 'hyprwin-test-1' }).floating -eq $true)
    Ctl dispatch togglefloating | Out-Null
    Ctl dispatch fullscreen 0 | Out-Null; Settle
    $f = (Frame $h[1]).Split(',') | ForEach-Object { [int]$_ }
    Check 'fullscreen 0 covers the monitor' ($f[0] -eq $mon.x -and $f[2] -eq $mon.width -and $f[3] -eq $mon.height) "frame=$($f -join ',')"
    Ctl dispatch fullscreen 0 | Out-Null; Settle
    Ctl dispatch movetoworkspacesilent special:magic | Out-Null; Settle
    Check 'special workspace hides window' ((Cloaked $h[1]) -ne 0)
    Ctl dispatch togglespecialworkspace magic | Out-Null; Settle
    Check 'togglespecialworkspace shows it' ((Cloaked $h[1]) -eq 0)
    Ctl dispatch togglespecialworkspace magic | Out-Null; Settle
    Check 'and hides it again' ((Cloaked $h[1]) -ne 0)

    Write-Host "`n== layout dispatchers"
    $before = Frame $h[0]
    Ctl dispatch focuswindow title:hyprwin-test-0 | Out-Null
    Ctl dispatch layoutmsg togglesplit | Out-Null; Settle
    Check 'layoutmsg togglesplit changes geometry' ((Frame $h[0]) -ne $before) "$before -> $(Frame $h[0])"
    $r = Ctl dispatch resizeactive 80 0; Settle
    Check 'resizeactive accepted' ($r -eq 'ok')
    $r = Ctl dispatch bogus
    Check 'unknown dispatcher reported, not fatal' ($r -match 'unknown dispatcher')

    Write-Host "`n== performance"
    $p = Get-Process -Id $daemon.Id
    $cpu0 = $p.TotalProcessorTime; Start-Sleep -Seconds 10; $p.Refresh(); $cpu1 = $p.TotalProcessorTime
    $idle = ($cpu1 - $cpu0).TotalMilliseconds / 100.0
    $ws = [math]::Round($p.WorkingSet64 / 1MB, 1); $priv = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
    Check 'idle CPU ~0% (10 s sample)' ($idle -lt 0.5) ("{0:N3}% of one core" -f $idle)
    Check 'RAM under 30 MB' ($ws -lt 30) "working set=$ws MB private=$priv MB"
    $sw = [Diagnostics.Stopwatch]::StartNew(); 1..20 | ForEach-Object { Ctl dispatch layoutmsg togglesplit | Out-Null }; $sw.Stop()
    Write-Host ("    20 dispatches round-trip via hyprwinctl: {0:N1} ms each (includes process start)" -f ($sw.Elapsed.TotalMilliseconds / 20))

    Write-Host "`n== keyboard binds (injected keystrokes)"
    Ctl dispatch workspace 1 | Out-Null
    Ctl dispatch focuswindow title:hyprwin-test-2 | Out-Null; Settle
    $wasFloating = ((Clients) | Where-Object { $_.title -eq 'hyprwin-test-2' }).floating
    [K]::Chord(0x5B, 0x56)   # Super+V -> togglefloating
    Settle
    $nowFloating = ((Clients) | Where-Object { $_.title -eq 'hyprwin-test-2' }).floating
    Check 'Super+V toggles floating' ($nowFloating -ne $wasFloating) "before=$wasFloating after=$nowFloating"
    Check 'Start menu did not open' (-not [K]::StartMenuOpen()) "foreground=$([K]::ForegroundClass())"
    [K]::Chord(0x5B, 0x56); Settle
    [K]::Chord(0x5B, 0x32)   # Super+2
    Settle
    Check 'Super+2 switches workspace' (((Ctl -j activeworkspace) | ConvertFrom-Json).id -eq 2)
    [K]::Chord(0x5B, 0x31)   # Super+1
    Settle
    Check 'Super+1 switches back' (((Ctl -j activeworkspace) | ConvertFrom-Json).id -eq 1)
    Check 'Start menu still closed' (-not [K]::StartMenuOpen()) "foreground=$([K]::ForegroundClass())"

    Write-Host "`n== crash safety"
    $guard = Get-Process hyprwin-guard -ErrorAction SilentlyContinue
    Check 'guard is running' ($null -ne $guard)
    Ctl dispatch workspace 2 | Out-Null; Settle
    Check 'windows hidden before crash' (($h | Where-Object { (Cloaked $_) -ne 0 }).Count -eq 3)
    Get-CimInstance Win32_Process -Filter "ProcessId=$($daemon.Id)" | Invoke-CimMethod -MethodName Terminate | Out-Null
    Start-Sleep -Milliseconds 1500
    Check 'after daemon is killed, guard restored every window' (($h | Where-Object { (Cloaked $_) -eq 0 }).Count -eq 3) "cloaked=$(($h | ForEach-Object { Cloaked $_ }) -join ',')"
    Check 'guard exits after restoring' ($null -eq (Get-Process hyprwin-guard -ErrorAction SilentlyContinue))
}
finally {
    Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" | Invoke-CimMethod -MethodName Terminate | Out-Null }
    Start-Sleep -Milliseconds 500
    # Belt and braces: replay any ledger left behind.
    $targets | ForEach-Object { if (-not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
