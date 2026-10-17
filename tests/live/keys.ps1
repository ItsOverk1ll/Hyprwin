# Live test: the keyboard hook under abuse.
# - A Super key-up the hook never sees (what happens when you press Win+L: the
#   key goes up on the lock screen) must not leave Super "held": plain typing
#   afterwards must not fire Super binds.
# - Hundreds of bind chords mashed as fast as possible.
#
#   cargo build --release
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\keys.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$bin = "$root\target\release"
$ctl = "$bin\hyprwinctl.exe"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices; using System.Threading;
public static class K9 {
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern short GetAsyncKeyState(int vk);
  // hyprwin's own marker: its hook ignores input carrying it.
  public static readonly UIntPtr MARK = (UIntPtr)0x48595052u;
  public static void Down(byte vk) { keybd_event(vk, 0, 0, UIntPtr.Zero); Thread.Sleep(25); }
  public static void Up(byte vk) { keybd_event(vk, 0, 2, UIntPtr.Zero); Thread.Sleep(25); }
  public static void UpUnseen(byte vk) { keybd_event(vk, 0, 2, MARK); Thread.Sleep(25); }
  // An unassigned key, invisible to the hook: stops a lone Super opening Start.
  public static void Mask() { keybd_event(0xE8, 0, 0, MARK); keybd_event(0xE8, 0, 2, MARK); Thread.Sleep(10); }
  public static void Tap(byte vk) { Down(vk); Up(vk); }
  public static void Chord(byte mod, byte key) { Down(mod); Tap(key); Up(mod); }
}
'@
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
function Ctl { (& $ctl @args 2>&1) -join "`n" }
function Kill-Hyprwin { Get-Process hyprwin, hyprwin-guard -ErrorAction SilentlyContinue | ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" -ErrorAction SilentlyContinue | Invoke-CimMethod -MethodName Terminate -ErrorAction SilentlyContinue | Out-Null } }
function Alive($p) { $p.Refresh(); -not $p.HasExited }
function Ws { ((Ctl -j activeworkspace) | ConvertFrom-Json).id }
$LWIN = 0x5B; $SHIFT = 0xA0; $J = 0x4A; $K = 0x4B

Kill-Hyprwin; Start-Sleep -Milliseconds 300
$state = "$env:TEMP\hyprwin-keys-state"
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$cfg = "$env:TEMP\hyprwin-keys.conf"
# Two test binds on keys with no Windows meaning.
(Get-Content "$root\crates\hyprwin-config\src\default.conf") + @('bind = SUPER, J, workspace, 4', 'bind = SUPER SHIFT, K, workspace, 5') | Set-Content $cfg
$env:HYPRWIN_STATE_DIR = $state
$env:HYPRWIN_MANAGE_ONLY = '^hyprwin-key-'
$procs = @()
$procs += Start-Process "$bin\animation-spike.exe" -ArgumentList 'target', '60,120,200', 100, 100, 500, 400, 'hyprwin-key-a' -PassThru
Start-Sleep -Milliseconds 800
$d = Start-Process "$bin\hyprwin.exe" -ArgumentList '--config', $cfg -PassThru
Start-Sleep -Milliseconds 1500
try {
    Ctl dispatch focuswindow title:hyprwin-key-a | Out-Null; Start-Sleep -Milliseconds 300

    Write-Host "== the bind works"
    [K9]::Chord($LWIN, $J); Start-Sleep -Milliseconds 500
    Check 'Super+J switches to workspace 4' ((Ws) -eq 4) "ws=$(Ws)"
    Ctl dispatch workspace 1 | Out-Null; Start-Sleep -Milliseconds 400

    Write-Host "`n== a Super key-up the hook never sees (Win+L / lock screen)"
    [K9]::Down($LWIN)
    [K9]::Mask()
    [K9]::UpUnseen($LWIN)
    Check 'Windows thinks Super is up' (([K9]::GetAsyncKeyState($LWIN) -band 0x8000) -eq 0)
    [K9]::Tap($J); Start-Sleep -Milliseconds 500
    Check 'plain J afterwards does not fire Super+J' ((Ws) -eq 1) "ws=$(Ws)"
    [K9]::Down($SHIFT); [K9]::Tap($K); [K9]::Up($SHIFT); Start-Sleep -Milliseconds 500
    Check 'plain Shift+K does not fire Super+Shift+K' ((Ws) -eq 1) "ws=$(Ws)"
    [K9]::Chord($LWIN, $J); Start-Sleep -Milliseconds 500
    Check 'a real Super+J still works afterwards' ((Ws) -eq 4) "ws=$(Ws)"
    Ctl dispatch workspace 1 | Out-Null; Start-Sleep -Milliseconds 400

    Write-Host "`n== a lost J key-up must not block J's bind"
    [K9]::Down($LWIN); [K9]::Down($J); [K9]::UpUnseen($J); [K9]::Mask(); [K9]::Up($LWIN); Start-Sleep -Milliseconds 400
    Ctl dispatch workspace 1 | Out-Null; Start-Sleep -Milliseconds 1500   # longer than any auto-repeat pause
    [K9]::Chord($LWIN, $J); Start-Sleep -Milliseconds 500
    Check 'Super+J fires again (not mistaken for auto-repeat)' ((Ws) -eq 4) "ws=$(Ws)"
    Ctl dispatch workspace 1 | Out-Null; Start-Sleep -Milliseconds 400

    Write-Host "`n== 300 chords mashed"
    $keys = 0x31, 0x32, 0x33, $J, 0x56  # 1 2 3 J V
    $rnd = [Random]::new(7)
    1..300 | ForEach-Object {
        $k = [byte]$keys[$rnd.Next($keys.Count)]
        if ($rnd.Next(4) -eq 0) { [K9]::Down($LWIN); [K9]::Down($SHIFT); [K9]::Tap($k); [K9]::Up($SHIFT); [K9]::Up($LWIN) } else { [K9]::Chord($LWIN, $k) }
    }
    Start-Sleep -Milliseconds 1500
    Check 'daemon survived' (Alive $d)
    $t0 = Get-Date; $v = Ctl version
    Check 'and answers promptly' ($v -match 'hyprwin' -and ((Get-Date) - $t0).TotalMilliseconds -lt 500)
    Check 'no modifier left held by the mash' (([K9]::GetAsyncKeyState($LWIN) -band 0x8000) -eq 0 -and ([K9]::GetAsyncKeyState($SHIFT) -band 0x8000) -eq 0)
    $before = Ws
    [K9]::Tap($J); Start-Sleep -Milliseconds 400
    Check 'plain typing after the mash is plain typing' ((Ws) -eq $before) "ws $before -> $(Ws)"
}
finally {
    0x5B, 0xA0, 0xA1, 0xA2, 0xA4, $J, $K | ForEach-Object { [K9]::Up([byte]$_) }
    Kill-Hyprwin
    Start-Sleep -Milliseconds 400
    $procs | ForEach-Object { if ($_ -and -not $_.HasExited) { $_.Kill() } }
    Remove-Item Env:\HYPRWIN_MANAGE_ONLY, Env:\HYPRWIN_STATE_DIR -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
