# Live test: dragging tiled windows by the title bar and by an edge.
#
# Dropping a tile on another tile moves it there; dragging an edge resizes
# the tile (not mistaken for a move). Moving to another monitor needs more
# than one monitor; the rule that makes such a drop a move (even when Windows
# rescales the window on the way) is covered by unit tests.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\drag.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public struct DR { public int L, T, R, B; }
[StructLayout(LayoutKind.Sequential)] public struct DMI { public int dx, dy; public uint data, flags, time; public IntPtr extra; }
[StructLayout(LayoutKind.Sequential)] public struct DIN { public uint type; public DMI mi; }
public static class DG {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out DR r, int s);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll")] public static extern uint SendInput(uint n, DIN[] i, int size);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, IntPtr l, uint f, uint t, out IntPtr r);
  public static int Hit(IntPtr h, int x, int y) { IntPtr r; SendMessageTimeout(h, 0x84, IntPtr.Zero, (IntPtr)((y << 16) | (x & 0xffff)), 2, 500, out r); return (int)r; }
  static void Send(int x, int y, uint extra) {
    int vx = GetSystemMetrics(76), vy = GetSystemMetrics(77), vw = GetSystemMetrics(78), vh = GetSystemMetrics(79);
    var i = new DIN[1]; i[0].type = 0;
    i[0].mi.dx = (int)(((long)(x - vx) * 65535) / (vw - 1)); i[0].mi.dy = (int)(((long)(y - vy) * 65535) / (vh - 1));
    i[0].mi.flags = 0x0001 | 0x8000 | 0x4000 | extra;
    SendInput(1, i, Marshal.SizeOf(typeof(DIN)));
  }
  public static void Drag(int x0, int y0, int x1, int y1) {
    Send(x0, y0, 0); System.Threading.Thread.Sleep(80);
    Send(x0, y0, 0x0002); System.Threading.Thread.Sleep(120);
    for (int s = 1; s <= 20; s++) { Send(x0 + (x1 - x0) * s / 20, y0 + (y1 - y0) * s / 20, 0); System.Threading.Thread.Sleep(25); }
    System.Threading.Thread.Sleep(150);
    Send(x1, y1, 0x0004); System.Threading.Thread.Sleep(100);
  }
}
'@
[void][DG]::SetProcessDpiAwarenessContext([IntPtr](-4))
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null } }
function Settle { Start-Sleep -Milliseconds 200; $t0 = Get-Date; while ([DG]::FindWindow('hyprwin.ghost', [NullString]::Value) -ne [IntPtr]::Zero -and ((Get-Date) - $t0).TotalMilliseconds -lt 3000) { Start-Sleep -Milliseconds 20 }; Start-Sleep -Milliseconds 500 }
function Rect($h) { $r = New-Object DR; [void][DG]::DwmGetWindowAttribute($h, 9, [ref]$r, 16); $r }
function Show($r) { "$($r.L),$($r.T)-$($r.R),$($r.B)" }

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$state = "$env:TEMP\hyprwin-drag-state"
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$env:HYPRWIN_STATE_DIR = $state
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-drag-'
$cfg = "$env:TEMP\hyprwin-drag-test.conf"
Copy-Item "$root\crates\hyprwin-config\src\default.conf" $cfg
$procs = @()
# Plain WinForms windows: the animation spike's targets can't be dragged by
# their title bar (their render loop keeps them in place).
foreach ($i in 0, 1, 2) {
    $form = "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.Text = 'hyprwin-drag-$i'; `$f.BackColor = [Drawing.Color]::FromArgb($(60 + 60 * $i), 120, 160); `$f.StartPosition = 'Manual'; `$f.Left = $(100 + 60 * $i); `$f.Top = 100; `$f.Width = 600; `$f.Height = 400; [Windows.Forms.Application]::Run(`$f)"
    $procs += Start-Process powershell -ArgumentList '-NoProfile', '-WindowStyle', 'Hidden', '-Command', $form -PassThru
}
Start-Sleep -Milliseconds 1500
Start-Sleep -Milliseconds 900
$h = foreach ($i in 0, 1, 2) { [DG]::FindWindow([NullString]::Value, "hyprwin-drag-$i") }
$d = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1500; Settle
try {
    $r = $h | ForEach-Object { Rect $_ }
    Write-Host "== start: $(($r | ForEach-Object { Show $_ }) -join '  ')"
    # Dwindle with three windows: one fills a half, two share the other half.
    $full = @(0, 1, 2 | Where-Object { ($r[$_].B - $r[$_].T) -gt ($r[0].B - $r[0].T + $r[1].B - $r[1].T + $r[2].B - $r[2].T) / 2 })
    if ($full.Count -ne 1) { throw "unexpected layout" }
    $a = $full[0]
    $others = @(0, 1, 2 | Where-Object { $_ -ne $a } | Sort-Object { $r[$_].T })
    $top = $others[0]; $bottom = $others[1]
    $aOnLeft = $r[$a].L -lt $r[$bottom].L
    # Grab points the window itself reports: title bar (HTCAPTION = 2) and edge.
    $cx = $r[$a].L + 200
    $cy = ($r[$a].T + 2)..($r[$a].T + 80) | Where-Object { [DG]::Hit($h[$a], $cx, $_) -eq 2 } | Select-Object -First 1
    if (-not $cy) { throw 'no title bar found' }
    $cy += 8

    Write-Host "`n== title-bar drag of the big tile onto the far side of the bottom tile"
    $b = $r[$bottom]
    $dropX = if ($aOnLeft) { $b.R - 60 } else { $b.L + 60 }
    [DG]::Drag($cx, $cy, $dropX, [int](($b.T + $b.B) / 2))
    Settle
    $na = Rect $h[$a]; $nb = Rect $h[$bottom]
    $sideOk = if ($aOnLeft) { $na.L -gt $nb.L } else { $na.R -lt $nb.R }
    Check 'the dragged tile now shares the bottom row, on the side it was dropped' ($sideOk -and [math]::Abs($na.T - $nb.T) -le 2) "dragged=$(Show $na) neighbour=$(Show $nb)"
    $c = Ctl -j clients | ConvertFrom-Json | ForEach-Object { $_ } | Where-Object { $_.title -eq "hyprwin-drag-$a" }
    Check 'still tiled, not floating' (-not $c.floating)

    Write-Host "`n== edge drag between the two bottom tiles resizes them"
    Settle
    # The bottom row now holds two tiles: drag the edge between them.
    $top = if ((Rect $h[$bottom]).L -gt (Rect $h[$a]).L) { $bottom } else { $a }
    $t = Rect $h[$top]
    $ey = [int](($t.T + $t.B) / 2)
    $leftEdge = $t.L -gt 100
    $ex = if ($leftEdge) { ($t.L - 3)..($t.L + 10) | Where-Object { [DG]::Hit($h[$top], $_, $ey) -eq 10 } | Select-Object -First 1 } else { ($t.R + 3)..($t.R - 10) | Where-Object { [DG]::Hit($h[$top], $_, $ey) -eq 11 } | Select-Object -First 1 }
    if (-not $ex) { throw 'no resize edge found' }
    $before = $t.R - $t.L
    [DG]::Drag($ex, $ey, $(if ($leftEdge) { $ex - 250 } else { $ex + 250 }), $ey)
    Settle
    $t2 = Rect $h[$top]
    $after = $t2.R - $t2.L
    Check 'the tile got wider (a resize, not a move)' ($after -gt $before + 100) "width $before -> $after"
    Check 'and stays in its row' ([math]::Abs($t2.T - $t.T) -le 2) (Show $t2)} finally {
    Ctl dispatch exit | Out-Null; Start-Sleep -Milliseconds 800
    $procs | ForEach-Object { Stop-Process -Id $_.Id -ErrorAction SilentlyContinue }
    Kill-Hyprwin
}
Write-Host "`n$($script:fail) failure(s)"
exit $script:fail
