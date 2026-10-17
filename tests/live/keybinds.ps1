# Live test: keybind changes from the Settings app reach the running daemon,
# and a newly bound key really works. Isolated APPDATA and state folder.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\keybinds.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class K7 {
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  public static void Combo(byte mod, byte key) {
    keybd_event(mod, 0, 0, IntPtr.Zero); keybd_event(key, 0, 0, IntPtr.Zero);
    keybd_event(key, 0, 2, IntPtr.Zero); keybd_event(mod, 0, 2, IntPtr.Zero);
  }
}
'@
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" | Invoke-CimMethod -MethodName Terminate | Out-Null } }
function Settings([string[]]$a) { (Start-Process "$bin\hyprwin-settings.exe" -ArgumentList $a -PassThru -Wait).ExitCode }
# PowerShell 5 passes a JSON array down the pipeline as one object: unroll it.
function Binds { $b = Ctl -j binds | ConvertFrom-Json; foreach ($x in $b) { $x } }
# The leading comma keeps a one-element result an array (PowerShell unrolls it otherwise).
function On($mods, $key) { return , @(Binds | Where-Object { $_.mods -eq $mods -and $_.key -eq $key -and -not $_.submap }) }

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$sandbox = "$env:TEMP\hyprwin-kb-test"
Remove-Item -Recurse -Force $sandbox -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$sandbox\hyprwin" | Out-Null
$realAppData = $env:APPDATA
$env:APPDATA = $sandbox
$env:HYPRWIN_STATE_DIR = "$sandbox\state"
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-kb-'
$cfg = "$sandbox\hyprwin\hyprland.conf"
Get-Content "$root\crates\hyprwin-config\src\default.conf" | Set-Content $cfg
$original = Get-Content $cfg -Raw
$procs = @()
$procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '90,90,160', 100, 100, 600, 400, 'hyprwin-kb-a' -PassThru
Start-Sleep -Milliseconds 800
$d = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1500
try {
    Write-Host "== binds JSON looks like hyprctl's"
    $q = On 'SUPER' 'Q'
    Check 'Super+Q is exec wt, in plain fields' ($q.Count -eq 1 -and $q[0].dispatcher -eq 'exec' -and $q[0].arg -eq 'wt') "$($q | ConvertTo-Json -Compress)"

    Write-Host "`n== add a new keybind"
    Check 'bind accepted' ((Settings '--bind', '"SUPER, N, workspace, 3"') -eq 0)
    Start-Sleep -Milliseconds 800
    $n = On 'SUPER' 'N'
    Check 'daemon has Super+N -> workspace 3' ($n.Count -eq 1 -and $n[0].dispatcher -eq 'workspace' -and $n[0].arg -eq '3') "$($n | ConvertTo-Json -Compress)"
    [K7]::Combo(0x5B, 0x4E); Start-Sleep -Milliseconds 700
    Check 'pressing Super+N really switches to workspace 3' (((Ctl -j activeworkspace) | ConvertFrom-Json).id -eq 3)
    Ctl dispatch workspace 1 | Out-Null; Start-Sleep -Milliseconds 500

    Write-Host "`n== change the keys of a default bind (Super+Q -> Super+Return)"
    Check 'unbind accepted' ((Settings '--unbind', '"SUPER, Q"') -eq 0)
    Check 'bind accepted' ((Settings '--bind', '"SUPER, Return, exec, wt"') -eq 0)
    Start-Sleep -Milliseconds 800
    Check 'Super+Q is free' ((On 'SUPER' 'Q').Count -eq 0)
    $r = On 'SUPER' 'Return'
    Check 'Super+Return opens the terminal' ($r.Count -eq 1 -and $r[0].action -eq 'Open Windows Terminal') "$($r.action)"

    Write-Host "`n== rebinding keys that were taken replaces, never doubles"
    Check 'bind accepted' ((Settings '--bind', '"SUPER, C, togglefloating"') -eq 0)
    Start-Sleep -Milliseconds 800
    $c = On 'SUPER' 'C'
    Check 'exactly one Super+C, the new action' ($c.Count -eq 1 -and $c[0].dispatcher -eq 'togglefloating') "$($c.Count) x $($c.dispatcher -join ',')"

    Write-Host "`n== bad input is refused and changes nothing"
    $before = Get-Content "$sandbox\hyprwin\binds.conf" -Raw
    Check 'unknown dispatcher refused' ((Settings '--bind', '"SUPER, Y, nosuchthing"') -ne 0)
    Check 'unknown key refused' ((Settings '--bind', '"SUPER, NotAKey, exec, wt"') -ne 0)
    Check 'binds.conf unchanged by refused edits' ((Get-Content "$sandbox\hyprwin\binds.conf" -Raw) -eq $before)

    Write-Host "`n== the user's own config is untouched"
    $now = Get-Content $cfg -Raw
    Check 'hyprland.conf only gained the managed block at the end' ($now.StartsWith($original.TrimEnd()) -and $now -match 'binds\.conf')
    Check 'no config warnings' (-not (Ctl configerrors).Trim()) (Ctl configerrors)

    Write-Host "`n== reset"
    Set-Content "$sandbox\hyprwin\binds.conf" ''
    Ctl reload | Out-Null; Start-Sleep -Milliseconds 800
    Check 'defaults back after clearing binds.conf' ((On 'SUPER' 'Q').Count -eq 1 -and (On 'SUPER' 'N').Count -eq 0 -and (On 'SUPER' 'C')[0].dispatcher -eq 'killactive')
}
finally {
    Kill-Hyprwin
    Start-Sleep -Milliseconds 400
    $procs | ForEach-Object { if (-not $_.HasExited) { $_.Kill() } }
    $env:APPDATA = $realAppData
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
