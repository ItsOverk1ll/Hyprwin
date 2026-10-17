# Live test: drawn borders (thick / gradient) around the focused window.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\border.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public struct BR { public int L, T, R, B; }
public static class B9 {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out BR r, int s);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
  [DllImport("gdi32.dll")] public static extern uint GetPixel(IntPtr dc, int x, int y);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
}
'@
[void][B9]::SetProcessDpiAwarenessContext([IntPtr](-4))
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null } }
function Frame($h) { $r = New-Object BR; [void][B9]::DwmGetWindowAttribute($h, 9, [ref]$r, 16); $r }
function Px($x, $y) { $dc = [B9]::GetDC([IntPtr]::Zero); $p = [B9]::GetPixel($dc, $x, $y); [void][B9]::ReleaseDC([IntPtr]::Zero, $dc); $p }
# COLORREF 0x00BBGGRR -> is it close to rgb?
function Near($p, $rgb, $tol = 40) {
    $r = $p -band 0xFF; $g = ($p -shr 8) -band 0xFF; $b = ($p -shr 16) -band 0xFF
    ([math]::Abs($r - (($rgb -shr 16) -band 0xFF)) -le $tol) -and ([math]::Abs($g - (($rgb -shr 8) -band 0xFF)) -le $tol) -and ([math]::Abs($b - ($rgb -band 0xFF)) -le $tol)
}
function Hex($p) { '{0:X6}' -f $p }
function Settle { Start-Sleep -Milliseconds 150; $t0 = Get-Date; while ([B9]::FindWindow('hyprwin.ghost', [NullString]::Value) -ne [IntPtr]::Zero -and ((Get-Date) - $t0).TotalMilliseconds -lt 3000) { Start-Sleep -Milliseconds 20 }; Start-Sleep -Milliseconds 350 }

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$state = "$env:TEMP\hyprwin-border-state"
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-border-test.conf"
# 4 px border, pure blue on the left to pure red on the right (0deg), square corners.
(Get-Content "$root\crates\hyprwin-config\src\default.conf") `
    -replace 'col.active_border = .*', 'col.active_border = rgba(0000ffff) rgba(ff0000ff) 0deg' `
    -replace 'border_size = \d+', 'border_size = 4' `
    -replace 'gaps_in = \d+', 'gaps_in = 10' `
    -replace '^(\s*)rounding = \d+', '$1rounding = 0' | Set-Content $cfg
$env:HYPRWIN_STATE_DIR = $state
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-bd-'
$procs = @()
$procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '40,40,40', 100, 100, 600, 400, 'hyprwin-bd-a' -PassThru
$procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '60,60,60', 300, 300, 600, 400, 'hyprwin-bd-b' -PassThru
Start-Sleep -Milliseconds 1200
$ha = [B9]::FindWindow([NullString]::Value, 'hyprwin-bd-a'); $hb = [B9]::FindWindow([NullString]::Value, 'hyprwin-bd-b')
$d = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1500; Settle
try {
    $scale = ((Ctl -j monitors | ConvertFrom-Json) | Where-Object { $_.focused }).scale
    $w = [int][math]::Round(4 * $scale)
    Write-Host "== border around the focused window (expected ${w}px at scale $scale)"
    Ctl dispatch focuswindow title:hyprwin-bd-a | Out-Null; Settle
    $f = Frame $ha; $my = [int](($f.T + $f.B) / 2)
    $left = Px ($f.L - 2) $my; $right = Px ($f.R + 1) $my
    Check 'left edge is blue (gradient start)' (Near $left 0x0000FF 60) (Hex $left)
    Check 'right edge is red (gradient end)' (Near $right 0xFF0000 60) (Hex $right)
    $outside = Px ($f.L - $w - 2) $my
    Check 'border is about the configured width (nothing blue beyond it)' (-not (Near $outside 0x0000FF 60)) (Hex $outside)
    $top = Px ([int](($f.L + $f.R) / 2)) ($f.T - 2)
    Check 'top edge mid-way is a blend of the two' (-not (Near $top 0x0000FF 30) -and -not (Near $top 0xFF0000 30) -and ((($top -band 0xFF) + (($top -shr 16) -band 0xFF)) -gt 150)) (Hex $top)
    $fb = Frame $hb
    $bl = Px ($fb.L - 2) ([int](($fb.T + $fb.B) / 2))
    Check 'unfocused window has no drawn border' (-not (Near $bl 0x0000FF 60)) (Hex $bl)
    Check 'four border strips exist and are visible' (@(Get-Process -Id $d.Id) -and [B9]::IsWindowVisible([B9]::FindWindow('hyprwin.border', [NullString]::Value)))

    Write-Host "`n== follows focus and moves"
    Ctl dispatch focuswindow title:hyprwin-bd-b | Out-Null; Settle
    $fb = Frame $hb; $fa = Frame $ha
    Check 'border moved to the newly focused window' (Near (Px ($fb.R + 1) ([int](($fb.T + $fb.B) / 2))) 0xFF0000 60) (Hex (Px ($fb.R + 1) ([int](($fb.T + $fb.B) / 2))))
    Check 'old window lost it' (-not (Near (Px ($fa.L - 2) ([int](($fa.T + $fa.B) / 2))) 0x0000FF 60))
    Ctl dispatch layoutmsg swapsplit | Out-Null; Settle
    $fb = Frame $hb
    Check 'after an animated swap the border hugs the new spot' (Near (Px ($fb.L - 2) ([int](($fb.T + $fb.B) / 2))) 0x0000FF 60) "frame L=$($fb.L)"

    Write-Host "`n== hidden when it should be"
    Ctl dispatch workspace 2 | Out-Null; Settle
    Check 'no border on an empty workspace' (-not [B9]::IsWindowVisible([B9]::FindWindow('hyprwin.border', [NullString]::Value)))
    Ctl dispatch workspace 1 | Out-Null; Settle
    Check 'back on ws 1 it returns' ([B9]::IsWindowVisible([B9]::FindWindow('hyprwin.border', [NullString]::Value)))
    Ctl dispatch fullscreen 0 | Out-Null; Settle
    Check 'no border in fullscreen' (-not [B9]::IsWindowVisible([B9]::FindWindow('hyprwin.border', [NullString]::Value)))
    Ctl dispatch fullscreen 0 | Out-Null; Settle
    $before = [B9]::IsWindowVisible([B9]::FindWindow('hyprwin.border', [NullString]::Value))
    $pr = Ctl pause; Start-Sleep -Milliseconds 600
    Write-Host "  (visible before pause: $before; pause reply: $pr; status: $(Ctl status))"
    Check 'no border while paused' (-not [B9]::IsWindowVisible([B9]::FindWindow('hyprwin.border', [NullString]::Value)))
    Ctl resume | Out-Null; Start-Sleep -Milliseconds 1500; Settle
    Check 'back after resume' ([B9]::IsWindowVisible([B9]::FindWindow('hyprwin.border', [NullString]::Value)))
}
finally {
    Kill-Hyprwin
    Start-Sleep -Milliseconds 400
    $procs | ForEach-Object { if (-not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
