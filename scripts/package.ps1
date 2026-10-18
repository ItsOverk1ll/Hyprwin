# Builds a release and packs what people download:
#   dist\hyprwin-<version>-setup.exe          single-file installer (just double-click)
#   dist\hyprwin-<version>-windows-x64.zip    the same programs, loose
# Both install per user, no admin needed. Set HYPRWIN_FEEDBACK_WEBHOOK first
# to let the Settings window's Feedback tab send to Discord.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\.."
Set-Location $root
cargo build --release --workspace --exclude animation-spike
if ($LASTEXITCODE -ne 0) { throw 'build failed' }
$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$name = "hyprwin-$version-windows-x64"
$out = "$root\dist\$name"
Remove-Item -Recurse -Force $out, "$root\dist\$name.zip" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
foreach ($f in 'hyprwin.exe', 'hyprwin-guard.exe', 'hyprwinctl.exe', 'hyprwin-settings.exe') {
    Copy-Item "$root\target\release\$f" $out
}
# The same program as hyprwin-settings.exe; its name makes it open in setup mode.
Copy-Item "$root\target\release\hyprwin-settings.exe" "$out\hyprwin-setup.exe"
Copy-Item "$root\LICENSE-MIT", "$root\LICENSE-APACHE" $out
@"
hyprwin $version - Hyprland-style tiling for Windows 11

INSTALL
  Double-click hyprwin-setup.exe and follow the steps. It installs for your
  user only (no admin rights) and adds hyprwin to the Start menu.

FIRST STEPS
  Super (Windows key) + Q   open a terminal
  Super + C                 close the focused window
  Super + 1..9              switch workspace
  Super + Shift + 1..9      move the window to a workspace
  Super + arrows            move focus
  Right-click the tray icon for Settings, Themes, App Groups and Pause.

IF ANYTHING GOES WRONG
  Ctrl + Alt + Shift + Win + Esc puts every window back and quits hyprwin.

UNINSTALL
  Windows Settings > Apps > Installed apps > hyprwin > Uninstall.

More: https://github.com/ItsOverk1ll/Hyprwin
"@ | Set-Content -Encoding UTF8 "$out\README.txt"
Compress-Archive -Path "$out\*" -DestinationPath "$root\dist\$name.zip"

# Single-file installer: the settings program with every program embedded
# (built in its own target folder so target\release keeps the normal one).
$stage = "$root\target\bundle-stage"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
foreach ($f in 'hyprwin.exe', 'hyprwin-guard.exe', 'hyprwinctl.exe', 'hyprwin-settings.exe') { Copy-Item "$root\target\release\$f" $stage }
$env:HYPRWIN_BUNDLE_DIR = $stage
cargo build --release -p hyprwin-settings --features bundle --target-dir "$root\target\bundle-build"
$code = $LASTEXITCODE
Remove-Item Env:\HYPRWIN_BUNDLE_DIR
if ($code -ne 0) { throw 'bundled installer build failed' }
$setup = "$root\dist\hyprwin-$version-setup.exe"
Copy-Item "$root\target\bundle-build\release\hyprwin-settings.exe" $setup -Force
"packed $setup ($([math]::Round((Get-Item $setup).Length / 1MB, 1)) MB)"
$zip = Get-Item "$root\dist\$name.zip"
"packed $($zip.FullName) ($([math]::Round($zip.Length / 1MB, 1)) MB)"
