# Live soak test: minutes of nonstop random use (commands, windows opening and
# dying, minimize storms, reloads), sampling handles, GDI/USER objects, memory
# and threads every minute. Slow leaks only show over thousands of operations.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\soak.ps1      # HYPRWIN_SOAK_MIN=10 by default

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class S10 {
  [DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr p, uint flags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
}
'@
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null } }
function Alive($p) { $p.Refresh(); -not $p.HasExited }
function Cloaked($h) { $v = 0; [void][S10]::DwmGetWindowAttribute($h, 14, [ref]$v, 4); $v }
function Sample($p) {
    $p.Refresh()
    [pscustomobject]@{ handles = $p.HandleCount; gdi = [S10]::GetGuiResources($p.Handle, 0); user = [S10]::GetGuiResources($p.Handle, 1); mb = [math]::Round($p.PrivateMemorySize64 / 1MB, 1); threads = $p.Threads.Count }
}
$script:n = 0
function Spawn {
    $script:n++
    Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '100,140,180', (40 * ($script:n % 20)), 80, 480, 340, "hyprwin-soak-$($script:n)" -PassThru
}
function Targets { @(Get-Process animation-spike -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowTitle -like 'hyprwin-soak-*' }) }

$minutes = if ($env:HYPRWIN_SOAK_MIN) { [int]$env:HYPRWIN_SOAK_MIN } else { 10 }
Kill-Hyprwin; Start-Sleep -Milliseconds 300
$state = "$env:TEMP\hyprwin-soak-state"
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-soak.conf"
$base = (Get-Content "$root\crates\hyprwin-config\src\default.conf") -replace 'col.active_border = .*', 'col.active_border = rgba(33ccffee) rgba(00ff99ee) 45deg'
$base | Set-Content $cfg
$env:HYPRWIN_STATE_DIR = $state
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-soak-'
$procs = [System.Collections.ArrayList]::new()
1..6 | ForEach-Object { [void]$procs.Add((Spawn)) }
Start-Sleep -Milliseconds 1200
$d = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 2000
$cmds = @('workspace 1', 'workspace 2', 'workspace 3', 'workspace e+1', 'workspace previous', 'movetoworkspacesilent 2', 'movetoworkspacesilent 3', 'movetoworkspace 1',
    'togglefloating', 'fullscreen 0', 'fullscreen 1', 'layoutmsg togglesplit', 'layoutmsg swapsplit', 'swapnext', 'cyclenext', 'movefocus l', 'movefocus r',
    'movewindow u', 'movewindow d', 'togglespecialworkspace', 'resizeactive 40 -30', 'resizeactive -60 20', 'centerwindow', 'focuscurrentorlast', 'killactive')
$rnd = [Random]::new(42)
$ops = 0
$samples = @()
try {
    Start-Sleep -Seconds 3
    $samples += Sample $d
    Write-Host "  minute 0: $($samples[-1])"
    $t0 = Get-Date
    for ($m = 1; $m -le $minutes; $m++) {
        while (((Get-Date) - $t0).TotalMinutes -lt $m) {
            $c = $cmds[$rnd.Next($cmds.Count)]
            [void](& $ctl dispatch $c.Split(' ') 2>&1); $ops++
            if ($rnd.Next(30) -eq 0) { [void]$procs.Add((Spawn)) }
            if ($rnd.Next(60) -eq 0) { $v = Targets | Get-Random; if ($v) { $v.Kill() } }
            if ($rnd.Next(80) -eq 0) {
                $t = Targets | Get-Random -Count 3
                $t | ForEach-Object { [void][S10]::ShowWindow($_.MainWindowHandle, 6) }; Start-Sleep -Milliseconds 100
                $t | ForEach-Object { [void][S10]::ShowWindow($_.MainWindowHandle, 9) }
            }
            if ($rnd.Next(200) -eq 0) { ($base -replace 'gaps_out = .*', "gaps_out = $($rnd.Next(0, 30))") | Set-Content $cfg }
            if ((Targets).Count -lt 3) { [void]$procs.Add((Spawn)) }
            Start-Sleep -Milliseconds ($rnd.Next(20, 120))
        }
        if (-not (Alive $d)) { break }
        $samples += Sample $d
        Write-Host "  minute ${m}: $($samples[-1])  ($ops commands, $((Targets).Count) windows)"
    }
    Check 'daemon alive after the soak' (Alive $d) "$ops commands"
    if (Alive $d) {
        & $ctl dispatch workspace 1 | Out-Null
        Start-Sleep -Seconds 3
        $end = Sample $d
        # Compare the second half against the first: a leak keeps growing,
        # warm-up (caches, lazily created windows) does not.
        $mid = $samples[[int]($samples.Count / 2)]
        Write-Host "  end: $end"
        Check 'handles do not keep growing' ($end.handles - $mid.handles -le 40) "$($samples[0].handles) -> $($mid.handles) -> $($end.handles)"
        Check 'GDI objects do not keep growing' ($end.gdi - $mid.gdi -le 10) "$($samples[0].gdi) -> $($mid.gdi) -> $($end.gdi)"
        Check 'USER objects do not keep growing' ($end.user - $mid.user -le 10) "$($samples[0].user) -> $($mid.user) -> $($end.user)"
        Check 'memory does not keep growing (<= +6 MB)' ($end.mb - $mid.mb -le 6) "$($samples[0].mb) -> $($mid.mb) -> $($end.mb) MB"
        Check 'threads back to normal' ($end.threads -le $samples[0].threads + 3) "$($samples[0].threads) -> $($end.threads)"
        $bad = @()
        foreach ($w in (& $ctl -j clients | ConvertFrom-Json)) {
            $h = [IntPtr][Convert]::ToInt64($w.address, 16)
            $hidden = (Cloaked $h) -ne 0
            if ($w.hidden -and -not $w.hyprwin.minimized -and -not $hidden) { $bad += "$($w.title) visible on hidden ws" }
            if (-not $w.hidden -and $hidden) { $bad += "$($w.title) cloaked on shown ws" }
        }
        Check 'model matches the screen at the end' ($bad.Count -eq 0) ($bad -join '; ')
    }
}
finally {
    Kill-Hyprwin
    Start-Sleep -Milliseconds 500
    $procs | ForEach-Object { if ($_ -and -not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
