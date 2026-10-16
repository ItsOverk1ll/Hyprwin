# Live test: tray icon, themes, settings.conf, pause/resume, App Groups.
# Runs fully isolated: APPDATA and HYPRWIN_STATE_DIR point at a temp folder,
# so your real config, themes, groups and saved layout are never touched.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\ux.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public struct NII { public uint cbSize; public IntPtr hWnd; public uint uID; public Guid guidItem; }
public struct RCT { public int L, T, R, B; }
public static class U5 {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("shell32.dll")] public static extern int Shell_NotifyIconGetRect(ref NII id, out RCT r);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out RCT r, int s);
  [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
  [DllImport("gdi32.dll")] public static extern uint GetPixel(IntPtr dc, int x, int y);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
}
'@
[void][U5]::SetProcessDpiAwarenessContext([IntPtr](-4))
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Cloaked($h) { $v = 0; [void][U5]::DwmGetWindowAttribute($h, 14, [ref]$v, 4); $v }
# The border colour as drawn on screen: the pixel on the window's left frame edge
# (DWMWA_BORDER_COLOR can be set on other apps' windows but not read back).
function Border($h) {
    $r = New-Object RCT; [void][U5]::DwmGetWindowAttribute($h, 9, [ref]$r, 16)
    $dc = [U5]::GetDC([IntPtr]::Zero); $px = [U5]::GetPixel($dc, $r.L, [int](($r.T + $r.B) / 2)); [void][U5]::ReleaseDC([IntPtr]::Zero, $dc)
    $px   # COLORREF 0x00BBGGRR
}
function Near($a, $b) { $d = { param($x, $s) ($x -shr $s) -band 0xFF }; ([math]::Abs((& $d $a 0) - (& $d $b 0)) -le 24) -and ([math]::Abs((& $d $a 8) - (& $d $b 8)) -le 24) -and ([math]::Abs((& $d $a 16) - (& $d $b 16)) -le 24) }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" | Invoke-CimMethod -MethodName Terminate | Out-Null } }
function Notepads { (Ctl -j clients | ConvertFrom-Json) | Where-Object { $_.class -eq 'notepad' } }

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$sandbox = "$env:TEMP\hyprwin-ux-test"
Remove-Item -Recurse -Force $sandbox -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$sandbox\hyprwin" | Out-Null
$realAppData = $env:APPDATA
$env:APPDATA = $sandbox
$env:HYPRWIN_STATE_DIR = "$sandbox\state"
$env:HYPRWIN_MANAGE_ONLY = 'Notepad$|^hyprwin-ux-'
$cfg = "$sandbox\hyprwin\hyprland.conf"
Get-Content "$root\crates\hyprwin-config\src\default.conf" | Set-Content $cfg

$procs = @()
$procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '200,60,60', 100, 100, 600, 400, 'hyprwin-ux-a' -PassThru
$procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '60,60,200', 300, 300, 600, 400, 'hyprwin-ux-b' -PassThru
Start-Sleep -Milliseconds 1200
$ha = [U5]::FindWindow([NullString]::Value, 'hyprwin-ux-a'); $hb = [U5]::FindWindow([NullString]::Value, 'hyprwin-ux-b')
$daemon = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1800
try {
    Write-Host "== tray icon"
    $main = [U5]::FindWindow('hyprwin.main', [NullString]::Value)
    $id = New-Object NII; $id.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($id); $id.hWnd = $main; $id.uID = 1
    $r = New-Object RCT
    Check 'tray icon is present in the notification area' ([U5]::Shell_NotifyIconGetRect([ref]$id, [ref]$r) -eq 0) "rect=$($r.L),$($r.T)-$($r.R),$($r.B)"

    Write-Host "`n== themes (via hyprwin-settings --apply-theme)"
    Check 'built-in themes installed' ((Get-ChildItem "$sandbox\hyprwin\themes" -Filter *.conf).Count -ge 6)
    Ctl dispatch focuswindow title:hyprwin-ux-a | Out-Null; Start-Sleep -Milliseconds 300
    $p = Start-Process "$bin\hyprwin-settings.exe" -ArgumentList '--apply-theme', 'nord' -PassThru -Wait
    Check 'apply-theme exits cleanly' ($p.ExitCode -eq 0)
    Start-Sleep -Milliseconds 1200
    Check 'config gained the managed theme block' ((Get-Content $cfg -Raw) -match 'themes\\nord\.conf')
    Check 'focused window border is Nord frost blue (88c0d0)' (Near (Border $ha) 0x00D0C088) ("0x{0:X6}" -f (Border $ha))
    Check 'other window border is Nord grey (3b4252)' (Near (Border $hb) 0x0052423B) ("0x{0:X6}" -f (Border $hb))
    $p = Start-Process "$bin\hyprwin-settings.exe" -ArgumentList '--apply-theme', 'does-not-exist' -PassThru -Wait
    Check 'unknown theme is rejected' ($p.ExitCode -ne 0)

    Write-Host "`n== settings.conf overrides the theme"
    Set-Content "$sandbox\hyprwin\settings.conf" "general:col.active_border = rgba(ff0000ff)`ngeneral:gaps_out = 0"
    Start-Sleep -Milliseconds 1200
    Check 'settings.conf colour wins' (Near (Border $ha) 0x000000FF) ("0x{0:X6}" -f (Border $ha))
    $c = Ctl -j clients | ConvertFrom-Json
    Check 'settings.conf gaps applied (window touches the screen edge)' ((($c | ForEach-Object { $_.at[0] }) | Measure-Object -Minimum).Minimum -eq 0)
    Set-Content "$sandbox\hyprwin\settings.conf" ""
    Start-Sleep -Milliseconds 800

    Write-Host "`n== pause / resume"
    Ctl dispatch focuswindow title:hyprwin-ux-b | Out-Null
    Ctl dispatch movetoworkspacesilent 2 | Out-Null; Start-Sleep -Milliseconds 500
    Check 'window on ws 2 is hidden before pausing' ((Cloaked $hb) -ne 0)
    Ctl pause | Out-Null; Start-Sleep -Milliseconds 800
    Check 'status reports paused' (((Ctl status) | ConvertFrom-Json).paused -eq $true)
    Check 'pausing hands every window back (ws 2 window visible)' ((Cloaked $hb) -eq 0)
    Check 'borders reset while paused (no longer red)' (-not (Near (Border $ha) 0x000000FF)) ("0x{0:X6}" -f (Border $ha))
    Ctl resume | Out-Null; Start-Sleep -Milliseconds 1500
    $c = Ctl -j clients | ConvertFrom-Json
    Check 'resume restores the layout (b back on ws 2, hidden)' ((($c | Where-Object { $_.title -eq 'hyprwin-ux-b' }).workspace.id -eq 2) -and ((Cloaked $hb) -ne 0))

    Write-Host "`n== App Groups (two Notepad windows)"
    Get-Process notepad -ErrorAction SilentlyContinue | Out-Null
    Ctl dispatch workspace 5 | Out-Null; Start-Sleep -Milliseconds 400
    Start-Process notepad.exe; Start-Sleep -Milliseconds 2500
    Start-Process notepad.exe; Start-Sleep -Milliseconds 2500
    $np = @(Notepads | Where-Object { $_.workspace.id -eq 5 })
    Check 'two Notepad windows tiled on ws 5' ($np.Count -eq 2) "count=$($np.Count)"
    $before = ($np | Sort-Object { $_.at[0] } | ForEach-Object { "$($_.size -join 'x')" }) -join '|'
    $r = Ctl groupsave "UX Test Group"
    Check 'group saved' ($r -match 'saved') $r
    $gfile = Get-ChildItem "$sandbox\hyprwin\groups" -Filter *.json | Select-Object -First 1
    $g = Get-Content $gfile.FullName -Raw | ConvertFrom-Json
    Check 'group file lists both apps with a launch id' ($g.apps.Count -eq 2 -and ($g.apps | Where-Object { $_.aumid -or $_.exe }).Count -eq 2) "aumid=$($g.apps[0].aumid)"
    foreach ($w in $np) { [void][U5]::PostMessage([IntPtr][Convert]::ToInt64($w.address, 16), 0x10, [IntPtr]::Zero, [IntPtr]::Zero) }
    Start-Sleep -Milliseconds 2500
    Check 'notepads closed' (@(Notepads).Count -eq 0) "left=$(@(Notepads).Count)"
    $r = Ctl groupopen "UX Test Group"
    Check 'group open started' ($r -match 'opening') $r
    $deadline = (Get-Date).AddSeconds(25)
    do { Start-Sleep -Milliseconds 700; $np = @(Notepads) } while ($np.Count -lt 2 -and (Get-Date) -lt $deadline)
    Start-Sleep -Milliseconds 1500
    $np = @(Notepads)
    $ws = (Ctl -j activeworkspace | ConvertFrom-Json).id
    Check 'both apps relaunched' ($np.Count -eq 2) "count=$($np.Count)"
    Check 'opened on a fresh workspace, tiled' (($np | Where-Object { $_.workspace.id -eq $ws -and -not $_.floating }).Count -eq 2) "ws=$ws"
    $after = ($np | Sort-Object { $_.at[0] } | ForEach-Object { "$($_.size -join 'x')" }) -join '|'
    Check 'arranged as saved (same tile sizes)' ($after -eq $before) "before=$before after=$after"
    foreach ($w in $np) { [void][U5]::PostMessage([IntPtr][Convert]::ToInt64($w.address, 16), 0x10, [IntPtr]::Zero, [IntPtr]::Zero) }
    Start-Sleep -Milliseconds 1000
}
finally {
    Kill-Hyprwin
    Start-Sleep -Milliseconds 800
    $procs | ForEach-Object { if (-not $_.HasExited) { $_.Kill() } }
    $env:APPDATA = $realAppData
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
