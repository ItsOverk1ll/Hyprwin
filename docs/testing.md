# Testing

## Automated

```powershell
cargo test --workspace                                                # 112 unit tests (layout, config, animation, ledger)
cargo build --release
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\phase1.ps1                   # 36 live checks
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\overflow.ps1                 # min-size overflow to the next workspace
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\restart.ps1                  # layout survives clean exit and crash
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\minimize.ps1                 # show desktop / minimize storms keep the layout
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\ux.ps1                       # tray, themes, settings.conf, pause, App Groups (isolated APPDATA)
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\anim.ps1                     # animations: open, move, focus/borders kept, retarget, workspace switch, crash mid-animation, disabled
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\taskbar.ps1                  # hide_taskbar: work area, edge reveal, Start, toggle, pause, crash restore (hides the real taskbar ~15 s)
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\border.ps1                   # drawn gradient border: colours, width, follows focus/swaps, hidden in fullscreen/empty ws/pause
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\leak.ps1                     # 40 swaps, 10 workspace trips, 6 animated closes: handles, GDI/USER objects, memory stable
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\setup.ps1                    # packaged zip: install, update, uninstall (sandboxed; run scripts\package.ps1 first)
.\target\release\animation-spike.exe selftest                                                # 22 animation/DWM checks
powershell -NoProfile -ExecutionPolicy Bypass -File spikes\win-api-probe\probe.ps1           # re-run on every Windows feature update
```

The live tests set two variables so they never touch your session:
- `HYPRWIN_MANAGE_ONLY`: the daemon manages only the test's own coloured windows.
- `HYPRWIN_STATE_DIR`: the tests use their own ledger and saved layout.

They do stop a running hyprwin first; its guard restores your windows. It checks:
- pixel-exact gaps;
- workspaces and the special workspace;
- floating and fullscreen;
- layout messages;
- injected Super+key binds, including that the Start menu stays closed;
- idle CPU and RAM;
- that the guard restores hidden windows after the daemon is killed.

## Manual checklist (real apps)

Run `hyprwin.exe` with no filter. For each app, check that it:
1. tiles or floats sensibly on open;
2. survives `workspace 2` → `workspace 1` (content intact, not black);
3. responds to Super+V / Super+F / Super+Shift+arrows;
4. follows Super+LMB drag and Super+RMB resize;
5. closes cleanly with Super+C;
6. is restored after `Ctrl+Alt+Shift+Win+Esc`.

| App | Expected | Result |
|---|---|---|
| File Explorer | tiles; its dialogs float | |
| Chrome / Edge | tiles; content repaints after a workspace switch (Chromium pauses while cloaked) | |
| VS Code | tiles; command palette not managed (child) | |
| Windows Terminal | tiles | |
| Settings (UWP) | tiles; class shows `systemsettings`, not `applicationframehost` | |
| Office (Word/Excel) | tiles; splash screen floats or is ignored; backstage OK | |
| Discord / Electron | tiles; no fight loop (check log for "keeps repositioning") | |
| A game, exclusive fullscreen | left alone; no stutter | |
| A game, borderless | `fullscreen 0` covers the monitor; taskbar hidden | |
| An elevated window (admin Terminal) | not managed; log says why; binds pause while it's focused (known) | |
| Task Manager | floats (default rule); it's elevated on some setups, so it may be unmanaged | |
| UAC prompt | invisible to hyprwin (secure desktop) | |
| Multi-monitor, different DPIs | gaps scale per monitor; movefocus/movewindow cross monitors; unplug keeps windows | |
| Sleep/resume, resolution change | layout re-applied within about 1.5 s | |
| Explorer restart (`taskkill /f /im explorer.exe`, then start it) | WM keeps working; cloak recovers | |

## Measuring performance

```powershell
hyprwinctl debug | Select-String memory        # working set
Get-Process hyprwin | Select-Object CPU, WS, PM
```
