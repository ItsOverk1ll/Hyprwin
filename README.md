# hyprwin

hyprwin is a Hyprland-style tiling window manager for Windows 11: same keybinds, same layouts (dwindle, master), same workspace model, and the same `hyprland.conf`.

It's a small native Rust daemon that runs on top of Explorer. It doesn't install drivers, inject DLLs or patch system files.

> Status: **Phase 1 done** (core tiling, workspaces, config, IPC, crash safety). Animations, taskbar hiding and the bar come next. See [docs/parity.md](docs/parity.md).

## Quick start

```powershell
cargo build --release
.\target\release\hyprwin.exe          # writes %APPDATA%\hyprwin\hyprland.conf on first run
.\target\release\hyprwinctl.exe clients
```

- Config: `%USERPROFILE%\.config\hypr\hyprland.conf`, else `%APPDATA%\hyprwin\hyprland.conf`. It hot-reloads on save.
- Log: `%LOCALAPPDATA%\hyprwin\hyprwin.log` (`HYPRWIN_LOG=debug` for more detail).
- **Emergency exit: Ctrl + Alt + Shift + Win + Esc.** This restores every window and quits. It's hard-coded and can't be unbound.
- If hyprwin is killed or crashes, `hyprwin-guard` puts every hidden window back.

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
