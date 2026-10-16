# ADR-001 — hyprwin architecture

- **Status:** Proposed. Awaiting approval.
- **Date:** 2026-10-16
- **Evidence:** [research.md](research.md), [spikes/win-api-probe](../spikes/win-api-probe/)

## Decisions

### 1. From scratch, not a fork

- komorebi's licence forbids redistributing forks.
- GlazeWM is excluded by request (and is GPL-3.0 and i3-shaped).
- The .NET and WebView projects miss the memory budget.

Rules for the build:
- Other WMs are used only as a *catalogue of Windows pitfalls*. No code is copied, nothing is vendored, and none of them is a runtime dependency.
- Third-party crates are limited to small, foundational libraries: `windows`/`windows-sys`, `regex`, `serde`/`serde_json`, `tracing`. No UI frameworks and no WebView.

### 2. Language and process model: Rust, small native processes, no runtime

| Binary | Role | Budget |
|---|---|---|
| `hyprwin.exe` | daemon: event loop, state, layouts, IPC server | < 30 MB RAM (target < 15), ~0 % idle CPU |
| `hyprwin-guard.exe` | watchdog: emergency hotkey, crash recovery | < 3 MB |
| `hyprwinctl.exe` | CLI, `hyprctl`-compatible | short-lived |
| `hyprwin-bar.exe` (Phase 5) | our own status bar, Direct2D/DirectWrite, no WebView | < 20 MB |

Cargo workspace layout:

```
crates/
  hyprwin-layout   pure Rust, NO Win32 — dwindle/master trees, workspaces, geometry. 100% unit-tested.
  hyprwin-config   hyprlang parser (+ variables, source=, sections) → typed config + warnings
  hyprwin-ipc      wire protocol shared by daemon/ctl/bar (text + JSON, socket2-style events)
  hyprwin-win32    the only crate allowed to touch Windows: hooks, cloak, DWM, thumbnails, monitors
  hyprwin          daemon binary
  hyprwin-guard    watchdog binary
  hyprwinctl       CLI binary
spikes/            throwaway proofs (PowerShell or Rust)
```

**Threads in the daemon:**
- **Main/WM thread.** Message loop, WinEvent hooks (out-of-context, no injection) and all state mutation. Idle means blocked in `GetMessageW`.
- **Input thread.** `WH_KEYBOARD_LL` (+ `WH_MOUSE_LL` only while needed), at high priority. The callback is a lock-free lookup in an immutable bind table swapped with `ArcSwap`; matches are posted to the main thread. It does no allocation and no I/O, so Windows never silently unhooks it.
- **Animation thread.** Only alive during an animation. It paces on the compositor clock (`DCompositionWaitForCompositorClock`, with `DwmFlush` as a fallback), so it runs at the monitor's real refresh rate (240 Hz here), then exits.
- **IPC thread.** Overlapped named pipes.

**Speed rules:**
- No polling anywhere.
- No `SetWindowPos` when geometry is unchanged.
- Async `SetWindowPos` (`SWP_ASYNCWINDOWPOS`) so a hung app can't stall the WM.
- Layout is computed in pure Rust in microseconds.
- Release builds use LTO + `opt-level=3` + `panic=abort` in the guard (not in the daemon, which needs `catch_unwind` for cleanup).

### 3. Workspaces: our own, hidden by shell-cloaking

- Per-monitor workspaces 1–10 + named + `special:` workspaces, Hyprland semantics. **No Windows virtual desktops**: their COM IIDs break almost every feature update.
- Inactive windows are hidden with `IApplicationView::SetCloak(1, 2)`. This is verified on 26H2: invisible, still composited, still thumbnail-able. If the shell has no view for a window (`TYPE_E_ELEMENTNOTFOUND`), we fall back to parking it off-screen.
- **A ledger is written before every cloak.** `%LOCALAPPDATA%\hyprwin\hidden.json` lists the HWND, PID, process start time and original rect. It is fsync'd before the cloak call. The daemon on exit, the panic hook and the guard on crash all uncloak everything in it.
- A cloaked window that gets minimised, destroyed or activated by an app is removed from the ledger immediately. This is the lesson from glazewm#1358.

### 4. Animations: DWM-thumbnail ghosts on a per-monitor overlay

- Each animated window gets a borderless, top-most, click-through, no-activate *ghost* window that we own. A live `DwmRegisterThumbnail` of the real window renders into it, and only the ghost is animated, using Hyprland's beziers and styles.
- The real window is moved once to its final rect, behind or cloaked under the ghost. At the end it is uncloaked under the ghost, which then fades out.
- Chromium/Electron path: move the window at the end instead, because Chromium stops painting while cloaked.
- Measured cost: about 0.12 ms per thumbnail update, so roughly 30 animated windows per 240 Hz frame.
- `animation =` / `bezier =` lines are fully supported. `animations { enabled = false }` turns everything into instant moves.
- Animations are cancellable. A new layout change retargets them mid-flight, as Hyprland does.
- Before Phase 1 builds on this, `spikes/animation/` will prove the approach visually on Explorer, Chrome, VS Code, Terminal and Settings.

### 5. Input

- `WH_KEYBOARD_LL` for all binds (`RegisterHotKey` can't override the shell's Win+X combos).
- `vkE8` mask-key injection so Super combos never open Start.
- `WH_MOUSE_LL` is installed only when `follow_mouse ≠ 0` or while a `bindm` modifier is held.
- **Win+L and Ctrl+Alt+Del can't be intercepted.** Freeing Win+L means an opt-in policy key, set only after asking you, and reverted on uninstall.

### 6. Elevation: run as a normal user by default

- **Default (A):** run as a normal user. Elevated windows are detected and left floating and untouched. `hyprwinctl debug` and the log say why.
  - **Limitation:** binds don't fire while an elevated window has focus.
- **Opt-in (B):** "manage admin windows" creates a logon Scheduled Task at highest privileges (asks first; the uninstaller removes it).
  - In this mode every `exec` is **de-elevated** through Explorer's shell token, so your terminal doesn't silently become admin.
- **Later (C):** a `uiAccess` build once code signing exists. It needs a signed binary in Program Files.

### 7. Shell: overlay mode by default

- Explorer keeps running.
- We hide the primary and secondary taskbars with `SW_HIDE`, re-hide them on `TaskbarCreated`, and own the work-area calculation. No global `SPI_SETWORKAREA` and no change to your taskbar settings.
- The taskbar is restored on exit, by the guard on crash, and by the emergency key.
- Shell-replacement mode (per-user Winlogon `Shell`) is a post-MVP opt-in, with the watchdog and uninstaller as escape hatches. It asks you before writing the registry.

### 8. Failsafe

- The emergency combo is **Ctrl + Alt + Shift + Win + Esc**. It's hard-coded and can't be bound or unbound by config.
- It's registered by `hyprwin-guard` with `RegisterHotKey`, a separate process with no LL hook, so it works even if the daemon is hung.
  - **Assumption to check in Phase 1:** whether the shell already claims this combo, which would make `RegisterHotKey` fail. If it does, the guard falls back to its own LL hook.
- On trigger, the guard:
  1. uncloaks everything in the ledger;
  2. restores the taskbar(s);
  3. resets DWM border colours and corners;
  4. kills the daemon;
  5. exits.
- The guard does the same if the daemon process dies (it waits on the process handle). The daemon's panic hook does it too. At daemon start, any stale ledger is replayed and cleared first.

### 9. Config: hyprlang syntax and IPC

- `%USERPROFILE%\.config\hypr\hyprland.conf`, then `%APPDATA%\hyprwin\hyprland.conf`.
- The parser accepts **all hyprlang dialects**:
  - `bind*`, `bindm`, `$vars`, `source =`, sections, `exec`/`exec-once`
  - `windowrulev2 = …` (old style)
  - `windowrule = match:… , effect` and `windowrule { … }` blocks (0.48–0.54 style)
  - `workspace =`, `monitor =`, `animation =`, `bezier =`
- Unknown keys produce a warning line, never an error. Config is hot-reloaded on save.
- **Class mapping:**
  - Hyprland `class` matches the **process name without `.exe`**, case-insensitive (`firefox`, `code`, `windowsterminal`).
  - `title` matches the window title.
  - Extensions: `match:winclass` (Win32 class name), `match:exe` (full path).
  - UWP apps hosted in `ApplicationFrameHost` resolve to the real app's process.
- **IPC:**
  - `\\.\pipe\hyprwin-<user-SID>` for request/response (hyprctl commands, `-j`, `--batch`).
  - `\\.\pipe\hyprwin-events-<SID>` for the socket2 stream (`EVENT>>DATA`).
  - The pipe ACL allows the current user only.

### 10. Ecosystem: build our own, minimal

- The bar is our own native Rust (Phase 5). No Zebar/YASB, per your instruction to limit existing software.
- The launcher is not built in for the MVP. `bind = SUPER, R, exec, <anything>` works with whatever launcher you like, and a tiny native launcher can be a Phase 5 option.

## Consequences and risks

- We depend on two undocumented but long-stable COM interfaces, `IApplicationViewCollection` and `IApplicationView`. Mitigations:
  - Probe them at startup.
  - If they break, fall back to off-screen parking automatically and log loudly.
  - Ship `spikes/win-api-probe` as a regression check for new Windows builds.
- Running non-elevated means admin windows aren't tiled and binds pause over them. This is a deliberate trade for safety.
- Animation quality depends on the ghost approach holding up for Chromium. The spike decides this before Phase 4 polish.
