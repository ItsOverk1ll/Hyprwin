# Live test: per-user install and uninstall from the packaged zip, in a
# sandbox (HYPRWIN_SETUP_TEST=1: %TEMP%\hyprwin-setup-test and registry
# names "hyprwin-test"), so a real installation is never touched.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\setup.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\..\.."
$zip = Get-ChildItem "$root\dist\hyprwin-*-windows-x64.zip" | Sort-Object LastWriteTime | Select-Object -Last 1
if (-not $zip) { throw 'run scripts\package.ps1 first' }
$script:fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { Write-Host "  [PASS] $name $detail" } else { Write-Host "  [FAIL] $name $detail" -ForegroundColor Red; $script:fail++ }
}
$sandbox = "$env:TEMP\hyprwin-setup-test"
$unzipped = "$env:TEMP\hyprwin-setup-unzipped"
Remove-Item -Recurse -Force $sandbox, $unzipped -ErrorAction SilentlyContinue
Expand-Archive $zip.FullName $unzipped
$env:HYPRWIN_SETUP_TEST = '1'
$uninst = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\hyprwin-test'
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
try {
    Write-Host "== package contents ($($zip.Name), $([math]::Round($zip.Length / 1MB, 1)) MB)"
    foreach ($f in 'hyprwin-setup.exe', 'hyprwin.exe', 'hyprwin-guard.exe', 'hyprwinctl.exe', 'hyprwin-settings.exe', 'README.txt') {
        Check "zip has $f" (Test-Path "$unzipped\$f")
    }

    Write-Host "`n== install (quiet, with autostart)"
    $p = Start-Process "$unzipped\hyprwin-setup.exe" -ArgumentList '--install', '--quiet', '--autostart' -PassThru -Wait
    Check 'installer exits cleanly' ($p.ExitCode -eq 0) "code=$($p.ExitCode)"
    foreach ($f in 'hyprwin.exe', 'hyprwin-guard.exe', 'hyprwinctl.exe', 'hyprwin-settings.exe') {
        Check "installed $f" (Test-Path "$sandbox\bin\$f")
    }
    Check 'Start-menu shortcut' (Test-Path "$sandbox\startmenu\hyprwin-test.lnk")
    Check 'Settings shortcut' (Test-Path "$sandbox\startmenu\hyprwin-test Settings.lnk")
    if (Test-Path "$sandbox\startmenu\hyprwin-test.lnk") {
        $t = (New-Object -ComObject WScript.Shell).CreateShortcut("$sandbox\startmenu\hyprwin-test.lnk").TargetPath
        # Compare as short paths: %TEMP% may be in 8.3 form (C:\Users\RUNNER~1\...) or not.
        $long = { param($p) $fso = New-Object -ComObject Scripting.FileSystemObject; if (Test-Path $p) { $fso.GetFile($p).ShortPath } else { $p } }
        Check 'shortcut points at the installed hyprwin.exe' ((& $long $t) -eq (& $long "$sandbox\bin\hyprwin.exe")) $t
    }
    $u = Get-ItemProperty $uninst -ErrorAction SilentlyContinue
    Check 'Installed-apps entry' ($u.DisplayName -eq 'hyprwin' -and $u.UninstallString -match 'hyprwin-settings\.exe" --uninstall') "$($u.UninstallString)"
    Check 'Installed-apps version and size' ($u.DisplayVersion -and $u.EstimatedSize -gt 1000) "v$($u.DisplayVersion) $($u.EstimatedSize) KB"
    $r = (Get-ItemProperty $run -ErrorAction SilentlyContinue).'hyprwin-test'
    Check 'starts at sign-in' ($r -eq "`"$sandbox\bin\hyprwin.exe`"") $r

    Write-Host "`n== reinstall without autostart (update path)"
    $p = Start-Process "$unzipped\hyprwin-setup.exe" -ArgumentList '--install', '--quiet' -PassThru -Wait
    Check 'update exits cleanly' ($p.ExitCode -eq 0)
    Check 'sign-in entry removed' (-not (Get-ItemProperty $run -ErrorAction SilentlyContinue).'hyprwin-test')

    Write-Host "`n== uninstall (the installed copy removes itself)"
    $p = Start-Process "$sandbox\bin\hyprwin-settings.exe" -ArgumentList '--uninstall', '--quiet' -PassThru -Wait
    Check 'uninstaller exits cleanly' ($p.ExitCode -eq 0)
    Start-Sleep -Seconds 4
    Check 'program folder deleted (including the uninstaller itself)' (-not (Test-Path "$sandbox\bin"))
    Check 'shortcuts removed' (-not (Test-Path "$sandbox\startmenu\hyprwin-test.lnk") -and -not (Test-Path "$sandbox\startmenu\hyprwin-test Settings.lnk"))
    Check 'Installed-apps entry removed' (-not (Test-Path $uninst))
}
finally {
    Remove-Item Env:\HYPRWIN_SETUP_TEST -ErrorAction SilentlyContinue
    Remove-Item $uninst -Recurse -ErrorAction SilentlyContinue
    Remove-ItemProperty $run -Name 'hyprwin-test' -ErrorAction SilentlyContinue
}
Write-Host "`n$script:fail failure(s)"
exit $script:fail
