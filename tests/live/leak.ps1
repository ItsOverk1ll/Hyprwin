# Live test: no resource leaks from animations, borders and closing pictures.
# Runs 40 animated swaps, 10 workspace round trips and 6 animated closes in
# an isolated daemon, then compares handles, GDI/USER objects and memory.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\leak.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class L4 {
  [DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr p, uint flags);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
}
'@
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" | Invoke-CimMethod -MethodName Terminate | Out-Null } }
function Settle { Start-Sleep -Milliseconds 120; $t0 = Get-Date; while ([L4]::FindWindow('hyprwin.ghost', [NullString]::Value) -ne [IntPtr]::Zero -and ((Get-Date) - $t0).TotalMilliseconds -lt 3000) { Start-Sleep -Milliseconds 15 }; Start-Sleep -Milliseconds 60 }
function Sample($p) {
    $p.Refresh()
    [pscustomobject]@{ handles = $p.HandleCount; gdi = [L4]::GetGuiResources($p.Handle, 0); user = [L4]::GetGuiResources($p.Handle, 1); mb = [math]::Round($p.PrivateMemorySize64 / 1MB, 1); threads = $p.Threads.Count }
}

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$state = "$env:TEMP\hyprwin-leak-state"
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-leak-test.conf"
# Gradient border on, so the drawn border is exercised too.
(Get-Content "$root\crates\hyprwin-config\src\default.conf") -replace 'col.active_border = .*', 'col.active_border = rgba(33ccffee) rgba(00ff99ee) 45deg' | Set-Content $cfg
$env:HYPRWIN_STATE_DIR = $state
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-lk-'
$procs = @()
$procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '200,60,60', 100, 100, 600, 400, 'hyprwin-lk-a' -PassThru
$procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '60,60,200', 300, 300, 600, 400, 'hyprwin-lk-b' -PassThru
Start-Sleep -Milliseconds 1000
$d = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1500; Settle
try {
    # Warm up once so lazily created things (overlay class, border strips) exist.
    Ctl dispatch layoutmsg swapsplit | Out-Null; Settle
    Ctl dispatch workspace 2 | Out-Null; Settle; Ctl dispatch workspace 1 | Out-Null; Settle
    $before = Sample $d
    1..40 | ForEach-Object { Ctl dispatch layoutmsg swapsplit | Out-Null; Settle }
    1..10 | ForEach-Object { Ctl dispatch workspace 2 | Out-Null; Settle; Ctl dispatch workspace 1 | Out-Null; Settle }
    1..6 | ForEach-Object {
        $procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '60,200,60', 500, 500, 500, 400, "hyprwin-lk-c$_" -PassThru
        Start-Sleep -Milliseconds 1200; Settle
        Ctl dispatch focuswindow "title:hyprwin-lk-c$_" | Out-Null; Settle
        Ctl dispatch killactive | Out-Null; Start-Sleep -Milliseconds 500; Settle
    }
    Start-Sleep -Milliseconds 800
    $after = Sample $d
    Write-Host "  before: $before"
    Write-Host "  after:  $after"
    Check 'GDI objects stable (<= +4)' ($after.gdi - $before.gdi -le 4) "$($before.gdi) -> $($after.gdi)"
    Check 'USER objects stable (<= +4)' ($after.user - $before.user -le 4) "$($before.user) -> $($after.user)"
    Check 'kernel handles stable (<= +20)' ($after.handles - $before.handles -le 20) "$($before.handles) -> $($after.handles)"
    Check 'threads back to baseline' ($after.threads -le $before.threads + 1) "$($before.threads) -> $($after.threads)"
    Check 'private memory stable (<= +3 MB)' ($after.mb - $before.mb -le 3) "$($before.mb) -> $($after.mb) MB"
    Check 'no snapshot windows left' ([L4]::FindWindow('hyprwin.snapshot', [NullString]::Value) -eq [IntPtr]::Zero)
}
finally {
    Kill-Hyprwin
    Start-Sleep -Milliseconds 400
    $procs | ForEach-Object { if (-not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
