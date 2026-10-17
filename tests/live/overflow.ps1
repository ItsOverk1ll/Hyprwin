# Live test: a window whose minimum size cannot fit on its workspace moves to
# the next workspace where it fits, without switching the user's workspace.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\overflow.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class W2 {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
}
'@
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Cloaked($h) { $v = 0; [void][W2]::DwmGetWindowAttribute($h, 14, [ref]$v, 4); $v }
# Waits until the layout is quiet: no animation running for 700 ms in a row
# (minimum sizes are learned in a few rounds), up to $max ms.
# Waits until the layout is quiet: the window list (workspaces, positions,
# sizes) unchanged for 1 s and no animation running, up to $max ms. Minimum
# sizes are learned in a few quick rounds without animating.
function Settle-Layout($max = 10000) {
    $t0 = Get-Date; $last = $null; $since = Get-Date
    while (((Get-Date) - $t0).TotalMilliseconds -lt $max) {
        $now = Ctl -j clients
        $busy = [W2]::FindWindow('hyprwin.ghost', [NullString]::Value) -ne [IntPtr]::Zero
        if ($busy -or $now -ne $last) { $last = $now; $since = Get-Date }
        elseif (((Get-Date) - $since).TotalMilliseconds -ge 1000) { break }
        Start-Sleep -Milliseconds 100
    }
}
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null } }

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$cfg = "$env:TEMP\hyprwin-test.conf"
Get-Content "$root\crates\hyprwin-config\src\default.conf" | Set-Content $cfg
$env:HYPRWIN_STATE_DIR = "$env:TEMP\hyprwin-test-state"
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-ovf-'
$daemon = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1200

$procs = @()
try {
    # Two windows of at least 36% x 51% of the screen, then one (2/3 of the screen wide) that refuses to be narrower than 2/3 of the screen.
    $mon = (Ctl -j monitors | ConvertFrom-Json) | Where-Object { $_.focused }
    # Relative to the monitor (designed on 4K: 1400x1100 and 2/3 wide). Windows
    # ignores a minimum larger than the screen, so fixed sizes broke on small ones.
    $wide = [int]($mon.width * 0.66)
    $minW = [int]($mon.width * 0.36); $minH = [int]($mon.height * 0.51)
    $procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '230,20,20', 100, 100, 500, 400, 'hyprwin-ovf-a', "$minW,$minH" -PassThru
    Start-Sleep -Milliseconds 600
    $procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '20,40,230', 200, 200, 500, 400, 'hyprwin-ovf-b', "$minW,$minH" -PassThru
    Start-Sleep -Milliseconds 600
    $procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '20,200,40', 300, 300, 500, 400, 'hyprwin-ovf-wide', "$wide,$minH" -PassThru
    Start-Sleep -Milliseconds 1000; Settle-Layout

    $c = Ctl -j clients | ConvertFrom-Json
    $w = $c | Where-Object { $_.title -eq 'hyprwin-ovf-wide' }
    $hw = [W2]::FindWindow([NullString]::Value, 'hyprwin-ovf-wide')
    Write-Host "  wide window: ws=$($w.workspace.id) floating=$($w.floating) size=$($w.size -join 'x') (min width $wide)"
    Check 'window that cannot fit moved to another workspace' ($w.workspace.id -ne 1) "ws=$($w.workspace.id)"
    Check 'it is tiled there, not floating' (-not $w.floating)
    Check 'user stays on workspace 1' (((Ctl -j activeworkspace) | ConvertFrom-Json).id -eq 1)
    Check 'it is hidden (on an inactive workspace)' ((Cloaked $hw) -ne 0)
    Check 'the other two still tile on ws 1' ((($c | Where-Object { $_.workspace.id -eq 1 -and -not $_.floating }).Count) -eq 2)
    Ctl dispatch workspace $w.workspace.id | Out-Null; Settle-Layout
    $c = Ctl -j clients | ConvertFrom-Json
    $w = $c | Where-Object { $_.title -eq 'hyprwin-ovf-wide' }
    Check 'switching there shows it, tiled at full size' ((Cloaked $hw) -eq 0 -and $w.size[0] -ge ($wide - 40)) "size=$($w.size -join 'x')"
    Write-Host "`n== moving it back to workspace 1 by hand (it must tile, not float)"
    Ctl dispatch focuswindow title:hyprwin-ovf-wide | Out-Null
    Ctl dispatch movetoworkspace 1 | Out-Null; Start-Sleep -Milliseconds 500; Settle-Layout
    $c = Ctl -j clients | ConvertFrom-Json
    $w = $c | Where-Object { $_.title -eq 'hyprwin-ovf-wide' }
    Check 'moved-back window is on workspace 1' ($w.workspace.id -eq 1) "ws=$($w.workspace.id)"
    Check 'and tiled, not floating' (-not $w.floating)
    $others = $c | Where-Object { $_.title -ne 'hyprwin-ovf-wide' }
    Check 'another window made room (moved on), none floating' ((($others | Where-Object { $_.workspace.id -ne 1 }).Count -ge 1) -and (($c | Where-Object { $_.floating }).Count -eq 0)) "$(($others | ForEach-Object { "$($_.title)=ws$($_.workspace.id)" }) -join ' ')"
    Ctl dispatch workspace 1 | Out-Null
}
finally {
    Kill-Hyprwin
    Start-Sleep -Milliseconds 800
    $procs | ForEach-Object { if (-not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
