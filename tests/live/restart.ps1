# Live test: layout survives a daemon restart (clean exit and crash).
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\restart.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class W3 {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
}
'@
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Cloaked($h) { $v = 0; [void][W3]::DwmGetWindowAttribute($h, 14, [ref]$v, 4); $v }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null } }
function Start-Hyprwin { $script:daemon = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru; Start-Sleep -Milliseconds 1500 }
function Layout {
    (Ctl -j clients | ConvertFrom-Json) | Sort-Object title | ForEach-Object { "$($_.title) ws=$($_.workspace.id) float=$($_.floating) at=$($_.at -join ',') size=$($_.size -join 'x')" }
}

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$env:HYPRWIN_STATE_DIR = "$env:TEMP\hyprwin-test-state"
Remove-Item -Recurse -Force $env:HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-test.conf"
Get-Content "$root\crates\hyprwin-config\src\default.conf" | Set-Content $cfg
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-rs-'

$procs = @()
foreach ($i in 0..3) {
    $procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', "$(40 + 50 * $i),80,200", (100 + 40 * $i), (100 + 40 * $i), 500, 400, "hyprwin-rs-$i" -PassThru
}
Start-Sleep -Milliseconds 1500
try {
    Start-Hyprwin
    Ctl dispatch focuswindow title:hyprwin-rs-1 | Out-Null
    Ctl dispatch layoutmsg splitratio 0.3 | Out-Null
    Ctl dispatch focuswindow title:hyprwin-rs-2 | Out-Null
    Ctl dispatch movetoworkspacesilent 3 | Out-Null
    Ctl dispatch focuswindow title:hyprwin-rs-3 | Out-Null
    Ctl dispatch togglefloating | Out-Null
    Start-Sleep -Milliseconds 1800   # let the debounced save happen
    $before = Layout
    Write-Host "  before:`n    $($before -join "`n    ")"

    Write-Host "`n== clean restart"
    Ctl dispatch exit | Out-Null; Start-Sleep -Milliseconds 1500
    Start-Hyprwin
    $after = Layout
    Check 'identical layout after clean restart' (($before -join '|') -eq ($after -join '|')) "`n    $($after -join "`n    ")"
    $h2 = [W3]::FindWindow([NullString]::Value, 'hyprwin-rs-2')
    Check 'window on workspace 3 is hidden again' ((Cloaked $h2) -ne 0)

    Write-Host "`n== crash and restart"
    Kill-Hyprwin; Start-Sleep -Milliseconds 1500
    Check 'guard restored hidden window after crash' ((Cloaked $h2) -eq 0)
    Start-Hyprwin
    $after = Layout
    Check 'identical layout after crash restart' (($before -join '|') -eq ($after -join '|')) "`n    $($after -join "`n    ")"

    Write-Host "`n== a window closed while hyprwin was down"
    Ctl dispatch exit | Out-Null; Start-Sleep -Milliseconds 1200
    $procs[0].Kill(); Start-Sleep -Milliseconds 500
    Start-Hyprwin
    $c = Ctl -j clients | ConvertFrom-Json
    Check 'remaining windows restored, closed one gone' ($c.Count -eq 3 -and -not ($c | Where-Object { $_.title -eq 'hyprwin-rs-0' })) "count=$($c.Count)"
    Check 'its workspace assignment kept' ((($c | Where-Object { $_.title -eq 'hyprwin-rs-2' }).workspace.id) -eq 3)
}
finally {
    Kill-Hyprwin
    Start-Sleep -Milliseconds 800
    $procs | ForEach-Object { if (-not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
