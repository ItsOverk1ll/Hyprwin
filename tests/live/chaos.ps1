# Live test: try to break hyprwin. Bursts of windows, parallel dispatch floods
# from several clients, malformed and silent IPC clients, a stalled event
# reader, config reloads (including garbage), minimize storms and windows
# killed mid-animation. Afterwards the daemon must still be alive and
# responsive, its model must match the screen, and on exit every window must
# be handed back.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\chaos.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public struct RC { public int L, T, R, B; }
public static class C6 {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr p, uint flags);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out RC r, int s);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
}
'@
[void][C6]::SetProcessDpiAwarenessContext([IntPtr](-4))
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Cloaked($h) { $v = 0; [void][C6]::DwmGetWindowAttribute($h, 14, [ref]$v, 4); $v }
function Frame($h) { $r = New-Object RC; [void][C6]::DwmGetWindowAttribute($h, 9, [ref]$r, 16); $r }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null } }
function Settle { Start-Sleep -Milliseconds 150; $t0 = Get-Date; while ([C6]::FindWindow('hyprwin.ghost', [NullString]::Value) -ne [IntPtr]::Zero -and ((Get-Date) - $t0).TotalMilliseconds -lt 4000) { Start-Sleep -Milliseconds 20 }; Start-Sleep -Milliseconds 400 }
function Alive($p) { $p.Refresh(); -not $p.HasExited }
function Sample($p) {
    $p.Refresh()
    [pscustomobject]@{ handles = $p.HandleCount; gdi = [C6]::GetGuiResources($p.Handle, 0); user = [C6]::GetGuiResources($p.Handle, 1); mb = [math]::Round($p.PrivateMemorySize64 / 1MB, 1); threads = $p.Threads.Count }
}
$colors = '200,60,60', '60,60,200', '60,200,60', '200,200,60', '200,60,200', '60,200,200', '120,120,120'
$script:n = 0
function Spawn {
    $script:n++
    $c = $colors[$script:n % $colors.Count]
    Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', $c, (100 + 20 * $script:n), (100 + 10 * $script:n), 500, 350, "hyprwin-cx-$($script:n)" -PassThru
}
function Targets { Get-Process animation-spike -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowTitle -like 'hyprwin-cx-*' } }

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$state = "$env:TEMP\hyprwin-chaos-state"
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-chaos.conf"
$base = (Get-Content "$root\crates\hyprwin-config\src\default.conf") -replace 'col.active_border = .*', 'col.active_border = rgba(33ccffee) rgba(00ff99ee) 45deg'
$base | Set-Content $cfg
$env:HYPRWIN_STATE_DIR = $state
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-cx-'
$procs = [System.Collections.ArrayList]::new()
1..4 | ForEach-Object { [void]$procs.Add((Spawn)) }
Start-Sleep -Milliseconds 1200
$d = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1500; Settle
$pipeTag = ($env:USERNAME -replace '[^A-Za-z0-9]', '_')
try {
    $before = Sample $d
    Write-Host "  baseline: $before"

    Write-Host "== burst: 10 windows at once"
    1..10 | ForEach-Object { [void]$procs.Add((Spawn)) }
    Start-Sleep -Milliseconds 2500; Settle
    $c = Ctl -j clients | ConvertFrom-Json
    Check 'all 14 windows managed' ($c.Count -eq 14) "managed=$($c.Count)"

    Write-Host "`n== a stalled event reader (connects, never reads)"
    $stall = New-Object System.IO.Pipes.NamedPipeClientStream('.', "hyprwin-events-$pipeTag", [System.IO.Pipes.PipeDirection]::In)
    $stall.Connect(2000)
    $evFile = "$env:TEMP\hyprwin-chaos-events.txt"
    $events = Start-Process $ctl -ArgumentList 'events' -RedirectStandardOutput $evFile -PassThru -WindowStyle Hidden

    Write-Host "`n== silent and malformed IPC clients"
    $silent = 1..3 | ForEach-Object {
        $s = New-Object System.IO.Pipes.NamedPipeClientStream('.', "hyprwin-$pipeTag", [System.IO.Pipes.PipeDirection]::InOut)
        $s.Connect(2000); $s
    }
    $t0 = Get-Date; $v = Ctl version
    Check 'requests still answered while 3 clients sit silent on the pipe' ($v -match 'hyprwin') "$([int]((Get-Date) - $t0).TotalMilliseconds) ms"
    foreach ($bytes in @(
            [byte[]](0xFF, 0xFF, 0xFF, 0x7F),                     # absurd length
            [byte[]](5, 0, 0, 0, 0xC3, 0x28, 0xA0, 0xA1, 0xFF),   # not UTF-8
            [byte[]](3, 0, 0, 0, 0x61),                           # truncated
            [byte[]](0, 0, 0, 0))) {                              # empty request
        try {
            $s = New-Object System.IO.Pipes.NamedPipeClientStream('.', "hyprwin-$pipeTag", [System.IO.Pipes.PipeDirection]::InOut)
            $s.Connect(2000); $s.Write($bytes, 0, $bytes.Length); $s.Flush(); Start-Sleep -Milliseconds 100; $s.Dispose()
        } catch { Write-Host "    (client error: $_)" }
    }
    # A 200 KB request (fine) and its reply, straight over the pipe.
    $req = [Text.Encoding]::UTF8.GetBytes('dispatch exec rem ' + ('x' * 200000))
    $s = New-Object System.IO.Pipes.NamedPipeClientStream('.', "hyprwin-$pipeTag", [System.IO.Pipes.PipeDirection]::InOut)
    $s.Connect(2000); $s.Write([BitConverter]::GetBytes([uint32]$req.Length), 0, 4); $s.Write($req, 0, $req.Length); $s.Flush()
    $reply = (New-Object System.IO.StreamReader($s)).ReadToEnd(); $s.Dispose()
    Check 'a 200 KB request gets an answer' ($reply.Length -gt 0) $reply
    # ~900 KB of nested batch prefixes: used to recurse until the stack overflowed.
    $bomb = [Text.Encoding]::UTF8.GetBytes('[[BATCH]]' * 100000 + 'version')
    try {
        $s = New-Object System.IO.Pipes.NamedPipeClientStream('.', "hyprwin-$pipeTag", [System.IO.Pipes.PipeDirection]::InOut)
        $s.Connect(2000); $s.Write([BitConverter]::GetBytes([uint32]$bomb.Length), 0, 4); $s.Write($bomb, 0, $bomb.Length); $s.Flush()
        [void](New-Object System.IO.StreamReader($s)).ReadToEnd(); $s.Dispose()
    } catch { Write-Host "    (bomb client error: $_)" }
    Start-Sleep -Milliseconds 500
    Check 'daemon survives a nested-batch bomb' (Alive $d)
    Check 'daemon alive after malformed requests' (Alive $d)
    Check 'still answering after malformed requests' ((Ctl version) -match 'hyprwin')

    Write-Host "`n== 4 parallel clients, 150 random dispatches each"
    $cmds = @('workspace 1', 'workspace 2', 'workspace 3', 'workspace e+1', 'workspace previous', 'movetoworkspacesilent 2', 'movetoworkspacesilent 3',
        'movetoworkspace 1', 'togglefloating', 'fullscreen 0', 'fullscreen 1', 'pseudo', 'layoutmsg togglesplit', 'layoutmsg swapsplit',
        'swapnext', 'cyclenext', 'movefocus l', 'movefocus r', 'movewindow u', 'movewindow d', 'togglespecialworkspace', 'movetoworkspacesilent special',
        'resizeactive 40 -30', 'resizeactive -80 20', 'centerwindow', 'pin', 'focuscurrentorlast', 'swapwindow l', 'splitratio 0.2', 'workspace 1')
    $flood = 1..4 | ForEach-Object {
        Start-Job -ScriptBlock {
            param($ctl, $cmds, $seed)
            $rnd = [Random]::new($seed); $errs = 0
            1..150 | ForEach-Object {
                $c = $cmds[$rnd.Next($cmds.Count)]
                $o = (& $ctl dispatch $c.Split(' ') 2>&1) -join ' '
                if ($LASTEXITCODE -ne 0 -and $o -match 'cannot connect|busy') { $errs++ }
            }
            $errs
        } -ArgumentList $ctl, $cmds, $_
    }
    # Meanwhile: minimize/restore storms and windows dying mid-animation.
    1..6 | ForEach-Object {
        $t = Targets | Get-Random -Count 3
        $t | ForEach-Object { [void][C6]::ShowWindow($_.MainWindowHandle, 6) }
        Start-Sleep -Milliseconds 150
        $t | ForEach-Object { [void][C6]::ShowWindow($_.MainWindowHandle, 9) }
        Start-Sleep -Milliseconds 200
    }
    1..3 | ForEach-Object {
        $victim = Targets | Get-Random
        Ctl dispatch workspace 2 | Out-Null
        if ($victim) { $victim.Kill() }
        [void]$procs.Add((Spawn))
        Start-Sleep -Milliseconds 300
    }
    $errs = $flood | Wait-Job -Timeout 240 | Receive-Job
    $flood | Remove-Job -Force
    Check 'no dispatch refused as busy/unreachable under parallel load' ((($errs | Measure-Object -Sum).Sum) -eq 0) "errors=$(($errs | Measure-Object -Sum).Sum)"
    Check 'daemon survived the flood' (Alive $d)

    Write-Host "`n== config reload storm (including garbage)"
    1..15 | ForEach-Object {
        $g = Get-Random -Minimum 0 -Maximum 40
        $text = $base -replace 'gaps_out = .*', "gaps_out = $g"
        if ($_ % 5 -eq 0) { $text = $text + "`n}}}{{ = = bind = SUPER,, nope`n`$x = `$x`$x`$x`nsource = $cfg`n" }
        $text | Set-Content $cfg
        Start-Sleep -Milliseconds (Get-Random -Minimum 20 -Maximum 250)
    }
    $base | Set-Content $cfg
    Start-Sleep -Milliseconds 1500
    Check 'daemon survived the reload storm (incl. a config that sources itself)' (Alive $d)

    Write-Host "`n== the dust settles: model vs screen"
    Ctl dispatch workspace 1 | Out-Null; Settle
    $c = Ctl -j clients | ConvertFrom-Json
    $mon = (Ctl -j monitors | ConvertFrom-Json)
    $live = @(Targets).Count
    Check 'model tracks exactly the live windows' ($c.Count -eq $live) "model=$($c.Count) live=$live"
    $bad = @()
    foreach ($w in $c) {
        $h = [IntPtr][Convert]::ToInt64($w.address, 16)
        if (-not [C6]::IsWindow($h)) { $bad += "$($w.title): gone"; continue }
        $hidden = (Cloaked $h) -ne 0
        if ($w.hidden -and -not $w.hyprwin.minimized -and -not $hidden) { $bad += "$($w.title): should be hidden (ws $($w.workspace.id))" }
        if (-not $w.hidden -and $hidden) { $bad += "$($w.title): cloaked but on a shown workspace" }
    }
    if ($bad.Count -gt 0) {
        # Name the cause: what the daemon logged about those windows.
        $addrs = $c | Where-Object { $bad -match [regex]::Escape($_.title) } | ForEach-Object { $_.address.TrimStart('0x').TrimStart('0') }
        $lines = Get-Content "$state\hyprwin.log" -ErrorAction SilentlyContinue | Where-Object { $l = $_; $addrs | Where-Object { $l -match $_ } }
        Write-Host "    log lines for those windows (last 30):"; $lines | Select-Object -Last 30 | ForEach-Object { Write-Host "      $_" }
    }
    Check 'every window is shown/hidden as the model says' ($bad.Count -eq 0) ($bad -join '; ')
    $shown = $c | Where-Object { -not $_.hidden -and -not $_.floating -and $_.fullscreen -eq 0 }
    $over = @()
    for ($i = 0; $i -lt $shown.Count; $i++) { for ($j = $i + 1; $j -lt $shown.Count; $j++) {
            $a = $shown[$i]; $b = $shown[$j]
            if ($a.workspace.id -ne $b.workspace.id) { continue }
            $ra = Frame ([IntPtr][Convert]::ToInt64($a.address, 16)); $rb = Frame ([IntPtr][Convert]::ToInt64($b.address, 16))
            if ($ra.L -lt $rb.R -and $rb.L -lt $ra.R -and $ra.T -lt $rb.B -and $rb.T -lt $ra.B) { $over += "$($a.title) x $($b.title)" }
        } }
    Check 'no two shown tiled windows overlap on screen' ($over.Count -eq 0) ($over -join '; ')
    $t0 = Get-Date; [void](Ctl -j clients); $ms = [int]((Get-Date) - $t0).TotalMilliseconds
    Check 'still fast: clients query < 500 ms' ($ms -lt 500) "$ms ms"

    $stall.Dispose()
    if (-not $events.HasExited) { $events.Kill() }
    Start-Sleep -Milliseconds 300
    $n = @(Get-Content $evFile -ErrorAction SilentlyContinue).Count
    Check 'a second event reader kept getting events despite the stalled one' ($n -gt 50) "events=$n"

    # The 3 silent clients are still connected: the daemon must have dropped
    # them after its 2 s request timeout instead of keeping a thread each.
    Start-Sleep -Seconds 3
    $after = Sample $d
    Write-Host "  after: $after"
    Check 'threads back near baseline (silent clients timed out)' ($after.threads -le $before.threads + 2) "$($before.threads) -> $($after.threads)"
    $dropped = $silent | Where-Object { try { $_.WriteByte(1); $_.Flush(); $false } catch { $true } }
    Check 'silent clients were disconnected by the daemon' (@($dropped).Count -eq 3) "$(@($dropped).Count)/3"
    Check 'GDI objects stable' ($after.gdi - $before.gdi -le 10) "$($before.gdi) -> $($after.gdi)"
    Check 'USER objects stable' ($after.user - $before.user -le 10) "$($before.user) -> $($after.user)"
    Check 'memory stable (<= +15 MB)' ($after.mb - $before.mb -le 15) "$($before.mb) -> $($after.mb) MB"

    Write-Host "`n== exit hands everything back"
    $silent | ForEach-Object { $_.Dispose() }
    Ctl dispatch exit | Out-Null
    $t0 = Get-Date; while ((Alive $d) -and ((Get-Date) - $t0).TotalSeconds -lt 10) { Start-Sleep -Milliseconds 100 }
    Check 'daemon exits cleanly' (-not (Alive $d))
    Start-Sleep -Milliseconds 800
    $stuck = @(Targets | Where-Object { (Cloaked $_.MainWindowHandle) -ne 0 } | ForEach-Object { $_.MainWindowTitle })
    Check 'no window left hidden after exit' ($stuck.Count -eq 0) ($stuck -join ', ')
    $ledger = Get-Content "$state\ledger.json" -Raw -ErrorAction SilentlyContinue
    Check 'ledger empty after exit' ($null -eq $ledger -or $ledger -match '"hidden":\[\]') $ledger
}
finally {
    Get-Job | Remove-Job -Force -ErrorAction SilentlyContinue
    Get-Process hyprwinctl -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Kill-Hyprwin
    Start-Sleep -Milliseconds 400
    $procs | ForEach-Object { if ($_ -and -not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
