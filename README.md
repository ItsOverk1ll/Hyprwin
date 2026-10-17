# hyprwin

hyprwin is a Hyprland-style tiling window manager for Windows 11: same keybinds, same layouts (dwindle, master), same workspace model, and the same `hyprland.conf`.

It's a small native Rust daemon that runs on top of Explorer. It doesn't install drivers, inject DLLs or patch system files.

> Status: tiling, workspaces, animations, themes, App Groups, taskbar hiding, a Settings window and an installer all work. See [docs/parity.md](docs/parity.md) for exactly what matches Hyprland.

## Install (no admin rights needed)

1. Download `hyprwin-<version>-windows-x64.zip` and unzip it.
2. Double-click **hyprwin-setup.exe** and press **Install**.
3. hyprwin starts and shows a notification. Its icon sits in the system tray.

To update, run the newer `hyprwin-setup.exe` (Settings › About › Check for updates tells you when there is one). Uninstall from **Windows Settings › Apps › Installed apps › hyprwin**. Your config, themes and groups are kept unless you tick the box to delete them.

**Emergency exit: Ctrl + Alt + Shift + Win + Esc.** This restores every window and the taskbar, then quits. It always works and can't be unbound. If hyprwin is killed or crashes, `hyprwin-guard` puts everything back.

## First steps

| Keys | Does |
|---|---|
| Super + Q | open a terminal |
| Super + C | close the focused window |
| Super + arrows | move focus |
| Super + Shift + arrows | move the window |
| Super + 1…9 | switch workspace |
| Super + Shift + 1…9 | move the window to a workspace |
| Super + V | float / tile the window |
| Super + F | fullscreen |
| Super + Alt + P | pause / resume tiling |
| Super + Alt + B | hide / show the taskbar |
| Super + left-drag / right-drag | move / resize with the mouse |

The full list is in the Settings window (Keybinds tab).

## Build from source

```powershell
cargo build --release
.\target\release\hyprwin.exe          # writes %APPDATA%\hyprwin\hyprland.conf on first run
.\target\release\hyprwinctl.exe clients
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1   # builds dist\hyprwin-<version>-windows-x64.zip
```

- Config: `%USERPROFILE%\.config\hypr\hyprland.conf`, else `%APPDATA%\hyprwin\hyprland.conf`. It hot-reloads on save.
- Log: `%LOCALAPPDATA%\hyprwin\hyprwin.log` (`HYPRWIN_LOG=debug` for more detail).

## For everyone: the tray icon

While hyprwin runs it sits in the system tray. Click it for:

- **Pause / Resume tiling.** Paused, every window behaves like normal Windows again. Resuming puts your layout back.
- **App Groups.** Open a saved group of apps, or save the current workspace as one. Saving remembers the apps and how they're arranged. Opening launches anything that isn't running (or reuses what is) and arranges everything as saved.
- **Themes…** Pick a look: colours, borders, corners and gaps. Themes are plain `.conf` files in `%APPDATA%\hyprwin\themes`, so they're easy to share and make. Six ship built in.
- **Settings…** Layout, gaps, border colours, corners, focus-follows-mouse, "tile everything", start with Windows.
- **Exit hyprwin.**

Settings and themes are written to `%APPDATA%\hyprwin\settings.conf` and to a small marked block at the end of your config. Nothing else in your `hyprland.conf` is ever rewritten. Scripts can apply a theme with `hyprwin-settings --apply-theme nord`.

## Default keys (Hyprland's)

| Keys | Action |
|---|---|
| Super+Q / C / M / E / V / R / P | terminal / close / exit / file manager / float / launcher / pseudo |
| Super+F, Super+Shift+F | fullscreen / maximize |
| Super+arrows or H J K | focus (Super+L is Windows' lock; see below) |
| Super+Shift+arrows or H J K L | move window |
| Super+Ctrl+arrows | resize |
| Super+1…0, Super+Shift+1…0 | workspace / move to workspace |
| Super+S, Super+Shift+S | scratchpad / move to scratchpad |
| Super+T | toggle split |
| Super+LMB / RMB drag | move / resize any window |
| Super+scroll | next/previous workspace |

## Docs

- [docs/adr-001-architecture.md](docs/adr-001-architecture.md): architecture decisions
- [docs/research.md](docs/research.md): what Windows allows, and why
- [docs/parity.md](docs/parity.md): every Hyprland feature and its status
- [docs/testing.md](docs/testing.md): automated tests and the manual checklist
- [spikes/animation](spikes/animation/README.md): how animations will work, with measurements

## License

MIT OR Apache-2.0.
