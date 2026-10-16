# Live test: minimize/restore storms ("show desktop") keep the layout intact,
# never fight the minimize, and never force-float anything.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\minimize.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class W4 {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
}
'@
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" | Invoke-CimMethod -MethodName Terminate | Out-Null } }
function Layout { (Ctl -j clients | ConvertFrom-Json) | Sort-Object title | ForEach-Object { "$($_.title) ws=$($_.workspace.id) float=$($_.floating) at=$($_.at -join ',') size=$($_.size -join 'x')" } }

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$env:HYPRWIN_STATE_DIR = "$env:TEMP\hyprwin-test-state"
Remove-Item -Recurse -Force $env:HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-test.conf"
Get-Content "$root\crates\hyprwin-config\src\default.conf" | Set-Content $cfg
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-mn-'

$procs = @(); $h = @()
foreach ($i in 0..4) {
    $procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', "$(40 + 40 * $i),120,200", (100 + 40 * $i), (100 + 40 * $i), 500, 400, "hyprwin-mn-$i" -PassThru
}
Start-Sleep -Milliseconds 1500
foreach ($i in 0..4) { $h += [W4]::FindWindow([NullString]::Value, "hyprwin-mn-$i") }
$daemon = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1500
try {
    Ctl dispatch focuswindow title:hyprwin-mn-2 | Out-Null
    Ctl dispatch layoutmsg splitratio 0.3 | Out-Null
    Start-Sleep -Milliseconds 600
    $before = Layout

    Write-Host "== burst minimize, scrambled restore (x3)"
    foreach ($round in 1..3) {
        foreach ($i in 3, 0, 4, 1, 2) { [void][W4]::ShowWindowAsync($h[$i], 6) }   # SW_MINIMIZE
        Start-Sleep -Milliseconds 900
        $stillUp = ($h | Where-Object { -not [W4]::IsIconic($_) }).Count
        Check "round ${round}: every window stayed minimized (no fighting)" ($stillUp -eq 0) "not minimized: $stillUp"
        foreach ($i in 1, 4, 2, 0, 3) { [void][W4]::ShowWindowAsync($h[$i], 9) }   # SW_RESTORE
        Start-Sleep -Milliseconds 1200
        $after = Layout
        Check "round ${round}: identical layout after restore" (($before -join '|') -eq ($after -join '|')) "`n    $($after -join "`n    ")"
    }

    Write-Host "`n== Windows 'Show desktop' toggle (x2)"
    $shell = New-Object -ComObject Shell.Application
    foreach ($round in 1..2) {
        $shell.ToggleDesktop(); Start-Sleep -Milliseconds 1200
        $shell.ToggleDesktop(); Start-Sleep -Milliseconds 1500
        $after = Layout
        Check "toggle ${round}: identical layout" (($before -join '|') -eq ($after -join '|')) "`n    $($after -join "`n    ")"
    }
    $floated = ((Ctl -j clients | ConvertFrom-Json) | Where-Object { $_.floating }).Count
    Check 'nothing was force-floated' ($floated -eq 0) "floating=$floated"
    $log = Get-Content "$env:HYPRWIN_STATE_DIR\hyprwin.log" -ErrorAction SilentlyContinue
    Check 'no "keeps repositioning" in log' (-not ($log | Select-String 'keeps repositioning'))
}
finally {
    Kill-Hyprwin
    Start-Sleep -Milliseconds 800
    $procs | ForEach-Object { if (-not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
