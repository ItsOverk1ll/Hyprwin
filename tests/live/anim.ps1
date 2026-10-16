# Live test: animations (open, move, workspace switch, retargeting, crash
# safety, disabled). Uses its own test windows and state folder only.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\anim.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public struct AR { public int L, T, R, B; }
public static class A6 {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out AR r, int s);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
  [DllImport("gdi32.dll")] public static extern uint GetPixel(IntPtr dc, int x, int y);
}
'@
[void][A6]::SetProcessDpiAwarenessContext([IntPtr](-4))
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Cloaked($h) { $v = 0; [void][A6]::DwmGetWindowAttribute($h, 14, [ref]$v, 4); $v }
function Frame($h) { $r = New-Object AR; [void][A6]::DwmGetWindowAttribute($h, 9, [ref]$r, 16); "$($r.L),$($r.T),$($r.R - $r.L),$($r.B - $r.T)" }
function BorderPx($h) { $r = New-Object AR; [void][A6]::DwmGetWindowAttribute($h, 9, [ref]$r, 16); $dc = [A6]::GetDC([IntPtr]::Zero); $p = [A6]::GetPixel($dc, $r.L, [int](($r.T + $r.B) / 2)); [void][A6]::ReleaseDC([IntPtr]::Zero, $dc); $p }
function Overlay { [A6]::FindWindow('hyprwin.ghost', [NullString]::Value) -ne [IntPtr]::Zero }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" | Invoke-CimMethod -MethodName Terminate | Out-Null } }
# Polls for $ms: did an overlay appear, were the windows cloaked meanwhile?
function Watch($ms, $hs) {
    $seen = @{ overlay = $false; cloaked = $false }
    $t0 = Get-Date
    while (((Get-Date) - $t0).TotalMilliseconds -lt $ms) {
        if (Overlay) { $seen.overlay = $true }
        foreach ($h in $hs) { if ((Cloaked $h) -ne 0) { $seen.cloaked = $true } }
        Start-Sleep -Milliseconds 10
    }
    $seen
}
function Settled($hs) { (-not (Overlay)) -and (@($hs | Where-Object { (Cloaked $_) -ne 0 }).Count -eq 0) }
function Matches-Model {
    $c = Ctl -j clients | ConvertFrom-Json
    $bad = @()
    foreach ($w in $c) {
        $h = [IntPtr][Convert]::ToInt64($w.address, 16)
        $want = "$($w.at[0]),$($w.at[1]),$($w.size[0]),$($w.size[1])"
        if ((Frame $h) -ne $want -and $w.workspace.id -eq ((Ctl -j activeworkspace) | ConvertFrom-Json).id) { $bad += "$($w.title): $(Frame $h) want $want" }
    }
    $bad
}

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$state = "$env:TEMP\hyprwin-anim-state"
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-anim-test.conf"
Get-Content "$root\crates\hyprwin-config\src\default.conf" | Set-Content $cfg
$env:HYPRWIN_STATE_DIR = $state
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-anim-'
$env:HYPRWIN_LOG = 'debug'
$procs = @()
function Start-Daemon { $script:daemon = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru; Start-Sleep -Milliseconds 1200 }
Start-Daemon
try {
    Write-Host "== open (windowsIn popin)"
    $procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '210,60,60', 100, 100, 600, 400, 'hyprwin-anim-a' -PassThru
    Start-Sleep -Milliseconds 1500
    $ha = [A6]::FindWindow([NullString]::Value, 'hyprwin-anim-a')
    $procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '60,60,210', 300, 300, 600, 400, 'hyprwin-anim-b' -PassThru
    $t0 = Get-Date
    do { $hb = [A6]::FindWindow([NullString]::Value, 'hyprwin-anim-b'); Start-Sleep -Milliseconds 5 } while ($hb -eq [IntPtr]::Zero -and ((Get-Date) - $t0).TotalSeconds -lt 5)
    $seen = Watch 700 @($ha, $hb)
    Check 'an overlay was drawn while animating' $seen.overlay
    Check 'windows were cloaked under it' $seen.cloaked
    Start-Sleep -Milliseconds 300
    Check 'afterwards: overlay gone, nothing left cloaked' (Settled @($ha, $hb))
    $bad = Matches-Model; Check 'real windows end exactly at their tiles' ($bad.Count -eq 0) ($bad -join '; ')

    Write-Host "`n== move (swap)"
    $before = Frame $ha
    Ctl dispatch layoutmsg swapsplit | Out-Null
    $seen = Watch 650 @($ha, $hb)
    Start-Sleep -Milliseconds 250
    Check 'swap animated through the overlay' ($seen.overlay -and $seen.cloaked)
    Check 'settled and uncloaked' (Settled @($ha, $hb))
    Check 'window actually moved' ((Frame $ha) -ne $before) "$before -> $(Frame $ha)"
    $bad = Matches-Model; Check 'positions match the model' ($bad.Count -eq 0) ($bad -join '; ')

    Write-Host "`n== focus survives animations (cloaking must not move activation)"
    foreach ($t in 'hyprwin-anim-a', 'hyprwin-anim-b') {
        Ctl dispatch focuswindow "title:$t" | Out-Null; Start-Sleep -Milliseconds 700
        Ctl dispatch layoutmsg swapsplit | Out-Null; Start-Sleep -Milliseconds 900
        $fw = ((Ctl -j activewindow) | ConvertFrom-Json).title
        $fg = ((Ctl -j clients | ConvertFrom-Json) | Where-Object { $_.address -eq ('0x{0:x}' -f [int64][A6]::GetForegroundWindow()) }).title
        Check "focus stays on $t through a swap" ($fw -eq $t -and $fg -eq $t) "model=$fw foreground=$fg"
        $hf = [A6]::FindWindow([NullString]::Value, $t)
        $px = BorderPx $hf
        # default.conf: col.active_border = rgba(33ccffee) -> COLORREF ~0x00FFCC33
        $ok = ([math]::Abs(($px -band 0xFF) - 0x33) -le 40) -and ([math]::Abs((($px -shr 8) -band 0xFF) - 0xCC) -le 40) -and ([math]::Abs((($px -shr 16) -band 0xFF) - 0xFF) -le 40)
        Check "focused border colour survives the animation ($t)" $ok ("0x{0:X6}" -f $px)
    }

    Write-Host "`n== rapid retargeting (6 swaps, 60 ms apart)"
    1..6 | ForEach-Object { Ctl dispatch layoutmsg swapsplit | Out-Null; Start-Sleep -Milliseconds 60 }
    Start-Sleep -Milliseconds 1200
    Check 'settled and uncloaked' (Settled @($ha, $hb))
    $bad = Matches-Model; Check 'positions match the model' ($bad.Count -eq 0) ($bad -join '; ')
    $hiddenList = ((Get-Content "$state\ledger.json" -Raw) | ConvertFrom-Json).hidden | ConvertTo-Json -Compress
    Check 'ledger lists neither window as hidden' (-not ("$hiddenList" -match "($([int64]$ha)|$([int64]$hb))")) "hidden=$hiddenList"

    Write-Host "`n== workspace switch (workspaces fade) and back"
    Ctl dispatch workspace 2 | Out-Null
    $seen = Watch 400 @()
    Start-Sleep -Milliseconds 200
    Check 'switch away animated' $seen.overlay
    Check 'ws 1 windows hidden afterwards' ((Cloaked $ha) -ne 0 -and (Cloaked $hb) -ne 0)
    Check 'overlay gone' (-not (Overlay))
    Ctl dispatch workspace 1 | Out-Null
    $seen = Watch 400 @()
    Start-Sleep -Milliseconds 300
    Check 'switch back animated' $seen.overlay
    Check 'ws 1 windows visible again' (Settled @($ha, $hb))
    Check 'stays on workspace 1 (no bounce)' (((Ctl -j activeworkspace) | ConvertFrom-Json).id -eq 1)
    $fw = ((Ctl -j activewindow) | ConvertFrom-Json).title
    Check 'a ws 1 window has focus' ($fw -like 'hyprwin-anim-*') $fw

    Write-Host "`n== quick back-and-forth (1-2-1 within 80 ms)"
    Ctl dispatch workspace 2 | Out-Null; Start-Sleep -Milliseconds 40
    Ctl dispatch workspace 1 | Out-Null; Start-Sleep -Milliseconds 900
    Check 'ws 1 windows visible, nothing stuck' (Settled @($ha, $hb))
    Check 'still on ws 1' (((Ctl -j activeworkspace) | ConvertFrom-Json).id -eq 1)

    Write-Host "`n== crash mid-animation: the guard restores cloaked windows"
    Ctl dispatch layoutmsg swapsplit | Out-Null
    Start-Sleep -Milliseconds 60
    $cl = ((Cloaked $ha) -ne 0) -or ((Cloaked $hb) -ne 0)
    Get-Process hyprwin -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" | Invoke-CimMethod -MethodName Terminate | Out-Null }
    Start-Sleep -Milliseconds 2500
    Check 'windows were cloaked when it died' $cl
    Check 'guard uncloaked them' ((Cloaked $ha) -eq 0 -and (Cloaked $hb) -eq 0)
    Check 'no overlay left behind' (-not (Overlay))
    Kill-Hyprwin; Start-Sleep -Milliseconds 300

    Write-Host "`n== animations disabled"
    Add-Content $cfg "`nanimations {`n    enabled = false`n}"
    Start-Daemon
    Start-Sleep -Milliseconds 800
    Ctl dispatch layoutmsg swapsplit | Out-Null
    $seen = Watch 500 @($ha, $hb)
    Check 'no overlay, no cloaking' (-not $seen.overlay -and -not $seen.cloaked)
    Start-Sleep -Milliseconds 300
    $bad = Matches-Model; Check 'instant move lands correctly' ($bad.Count -eq 0) ($bad -join '; ')

    $log = Get-Content "$state\hyprwin.log" -ErrorAction SilentlyContinue
    $done = $log | Select-String 'animation finished after (\d+) frames' | ForEach-Object { [int]$_.Matches[0].Groups[1].Value }
    $ms = $log | Select-String 'animation done in (\d+) ms' | ForEach-Object { [int]$_.Matches[0].Groups[1].Value }
    Write-Host "`n  animations: $($done.Count), frames each: $($done -join ',') ; durations ms: $($ms -join ',')"
}
finally {
    Kill-Hyprwin
    Start-Sleep -Milliseconds 500
    $procs | ForEach-Object { if (-not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR, Env:\HYPRWIN_LOG -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
