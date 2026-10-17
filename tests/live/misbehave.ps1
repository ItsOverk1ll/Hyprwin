# Live test: badly behaved apps. A hung app (stops processing messages), an
# app that fights every move, and one that renames itself every few ms. The
# daemon must stay responsive, cheap and correct around all of them, and
# hand every window back on exit, the hung one included.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\misbehave.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class M7 {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("user32.dll")] public static extern bool IsHungAppWindow(IntPtr h);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
}
'@
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Cloaked($h) { $v = 0; [void][M7]::DwmGetWindowAttribute($h, 14, [ref]$v, 4); $v }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null } }
function Alive($p) { $p.Refresh(); -not $p.HasExited }
function Timed([scriptblock]$b) { $t0 = Get-Date; [void](& $b); [int]((Get-Date) - $t0).TotalMilliseconds }
function CpuPct($p, $seconds) {
    $p.Refresh(); $c0 = $p.TotalProcessorTime; Start-Sleep -Seconds $seconds; $p.Refresh()
    [math]::Round(($p.TotalProcessorTime - $c0).TotalMilliseconds / ($seconds * 10), 1)
}
function Spawn($title, $mode, $x) {
    if ($mode) { $env:HYPRWIN_TARGET_MODE = $mode } else { Remove-Item Env:\HYPRWIN_TARGET_MODE -ErrorAction SilentlyContinue }
    $p = Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '150,90,40', $x, 120, 500, 380, $title -PassThru
    Remove-Item Env:\HYPRWIN_TARGET_MODE -ErrorAction SilentlyContinue
    $p
}

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$state = "$env:TEMP\hyprwin-misbehave-state"
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-misbehave.conf"
Get-Content "$root\crates\hyprwin-config\src\default.conf" | Set-Content $cfg
$env:HYPRWIN_STATE_DIR = $state
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-bad-'
$procs = @()
$procs += Spawn 'hyprwin-bad-a' $null 100
$procs += Spawn 'hyprwin-bad-b' $null 300
Start-Sleep -Milliseconds 800
$d = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1500
try {
    Write-Host "== a hung app"
    $procs += Spawn 'hyprwin-bad-hung' 'hang' 500
    # It stops answering after 1.5 s; Windows calls it hung after 5 s more.
    Start-Sleep -Milliseconds 7500
    $hh = [M7]::FindWindow([NullString]::Value, 'hyprwin-bad-hung')
    Check 'the app really is hung' ([M7]::IsHungAppWindow($hh)) "IsHungAppWindow=$([M7]::IsHungAppWindow($hh))"
    $slow = 0
    1..8 | ForEach-Object {
        $ms = Timed { Ctl dispatch workspace 2 }; if ($ms -gt 1000) { $slow++ }
        $ms = Timed { Ctl dispatch workspace 1 }; if ($ms -gt 1000) { $slow++ }
    }
    Check 'workspace switches stay fast with a hung app on screen' ($slow -eq 0) "slow=$slow/16"
    $ms = Timed { Ctl dispatch focuswindow title:hyprwin-bad-hung; Ctl dispatch togglefloating; Ctl dispatch togglefloating; Ctl dispatch fullscreen 1; Ctl dispatch fullscreen 1 }
    Check 'commands on the hung window do not block the daemon' ($ms -lt 3000) "$ms ms for 5 commands"
    $ms = Timed { Ctl dispatch killactive }
    Start-Sleep -Milliseconds 800
    $v = Timed { Ctl version }
    Check 'closing the hung window does not freeze the daemon' ($ms -lt 1500 -and $v -lt 500) "close=$ms ms, next request=$v ms"
    Ctl dispatch movetoworkspacesilent 3 | Out-Null; Start-Sleep -Milliseconds 600
    Check 'a hung window can still be hidden (cloaked by the shell)' ((Cloaked $hh) -ne 0) "cloaked=$(Cloaked $hh)"

    Write-Host "`n== an app that fights every move"
    $procs += Spawn 'hyprwin-bad-fight' 'fight' 700
    Start-Sleep -Milliseconds 1500
    $cpu = CpuPct $d 6
    Check 'no endless tug of war: daemon CPU low while the app fights' ($cpu -lt 10) "$cpu% of one core"
    $log = Get-Content "$state\hyprwin.log" -Raw -ErrorAction SilentlyContinue
    Check 'the fight is detected and paused' ($log -match 'keeps repositioning itself') ''
    Check 'daemon responsive' ((Timed { Ctl version }) -lt 500)

    Write-Host "`n== an app renaming itself every 5 ms"
    $evFile = "$env:TEMP\hyprwin-misbehave-events.txt"
    $ev = Start-Process $ctl -ArgumentList 'events' -RedirectStandardOutput $evFile -PassThru -WindowStyle Hidden
    $procs += Spawn 'hyprwin-bad-titles' 'titles' 900
    Start-Sleep -Milliseconds 1200
    $cpu = CpuPct $d 6
    Check 'title spam stays cheap for the daemon' ($cpu -lt 15) "$cpu% of one core"
    Check 'daemon responsive during title spam' ((Timed { Ctl -j clients }) -lt 500)
    if (-not $ev.HasExited) { $ev.Kill() }
    Start-Sleep -Milliseconds 300
    $n = @(Get-Content $evFile -ErrorAction SilentlyContinue | Where-Object { $_ -like 'windowtitle*' }).Count
    Check 'title events reach subscribers' ($n -gt 50) "windowtitle events=$n"
    $p = Get-Process -Id $d.Id; $p.Refresh()
    Check 'memory stays small' ($p.PrivateMemorySize64 / 1MB -lt 40) "$([math]::Round($p.PrivateMemorySize64 / 1MB, 1)) MB"

    Write-Host "`n== exit hands everything back, the hung window included"
    Ctl dispatch exit | Out-Null
    $t0 = Get-Date; while ((Alive $d) -and ((Get-Date) - $t0).TotalSeconds -lt 10) { Start-Sleep -Milliseconds 100 }
    Check 'daemon exits promptly despite the hung app' (-not (Alive $d)) "$([int]((Get-Date) - $t0).TotalMilliseconds) ms"
    Start-Sleep -Milliseconds 500
    $stuck = @('hyprwin-bad-a', 'hyprwin-bad-b', 'hyprwin-bad-hung', 'hyprwin-bad-fight') | Where-Object { $h = [M7]::FindWindow([NullString]::Value, $_); $h -ne [IntPtr]::Zero -and (Cloaked $h) -ne 0 }
    Check 'no window left hidden after exit' (@($stuck).Count -eq 0) ($stuck -join ', ')
}
finally {
    Get-Process hyprwinctl -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Kill-Hyprwin
    Start-Sleep -Milliseconds 400
    $procs | ForEach-Object { if ($_ -and -not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
