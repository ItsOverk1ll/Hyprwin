# Step 0 — Research findings

Date: 2026-10-16. Test machine: Windows 11 Home **26H2, build 26300.9711**, RTX 4060, 3840-wide panel @ **240 Hz**, **150 %** scaling, plus a "Meta Virtual Monitor" display adapter.

Everything below was checked against current sources: cloned repositories (default branches as of today), GitHub issue trackers, the Hyprland wiki's git history, and Microsoft documentation. The most important Win32 claims were also **verified live on this machine** with [`spikes/win-api-probe/probe.ps1`](../spikes/win-api-probe/probe.ps1). Results are in [`results-26300.9711.json`](../spikes/win-api-probe/results-26300.9711.json).

> **Clean-room note.** You asked to avoid existing software, and GlazeWM in particular. Other window managers were studied only to learn *what breaks on Windows and why*. No code from them is reused, none is a dependency, and nothing here is a fork. Where a technique is a Windows fact (for example, which COM interface cloaks a window), we implement it ourselves from the Windows API.

---

## 1. Existing Windows tiling WMs

| Project | Lang | License (verified in repo) | Latest release | Status |
|---|---|---|---|---|
| GlazeWM | Rust | GPL-3.0 | v3.10.1 (2026-03-21) | active (last push 2026-06) |
| komorebi | Rust | **Komorebi License 2.0.0** (fork of PolyForm Strict). Personal use only; **no redistribution of hard forks; no commercial use** without a paid individual licence | v0.1.41 (2026-05-03) | active |
| Whim | C# / .NET | MIT | — (last push 2026-03) | slowing |
| FancyWM | C# / .NET | own LICENSE file | v2.19.0 (2026-03-14) | active |
| Workspacer | C# / .NET | MIT | v0.9.11 (**2022**) | effectively dormant |
| bug.n | AutoHotkey | GPL-3.0 | v9.0.2 (**2019**) | dead |
| Seelen UI | Rust + Tauri/WebView (Svelte) | AGPL-3.0 | v2.8.9 (2026-10-12) | very active; full shell replacement |
| Zebar (bar) | Rust + Tauri/WebView | GPL-3.0 | v3.3.1 (2026-03-16) | active |
| YASB (bar) | Python (Qt) | MIT | v2.0.7 (2026-09-11) | very active |

No new native Windows WM offering Hyprland-style dwindle has appeared in 2026. The closest new project is OmniWM, and it is macOS-only.

**Fork verdict: write from scratch.**
- komorebi's licence forbids redistributing a fork, which kills the "distribute to other people" goal.
- GlazeWM is excluded by you. It is also GPL-3.0 and i3-shaped, so its tree model and YAML config don't match Hyprland's dwindle/master semantics and hyprlang config.
- The .NET options miss the RAM/startup budget. WebView-based bars and shells (Seelen, Zebar) idle at 100 MB or more.

### How each solves the hard problems

| Problem | GlazeWM | komorebi | Whim | FancyWM |
|---|---|---|---|---|
| Hiding inactive workspaces | `hide_method`: **Cloak (default)** / Hide / PlaceInCorner. Cloak = undocumented `IApplicationView::SetCloak(1, 2)` | `window_hiding_behaviour`: **Cloak (default)** / Hide (deprecated) / Minimize. Same undocumented call | `ShowWindow(SW_HIDE)` and marks the window minimised | Uses native Windows virtual desktops |
| Hotkeys | own `WH_KEYBOARD_LL` | external daemon (`whkd`) or AutoHotkey | own | own (Shift+Win activation chord) |
| Elevated windows | optional `uiAccess` cargo feature (needs a signed binary in Program Files); otherwise unmanaged | unmanaged unless run elevated; several "admin window unmanaged" issues | unmanaged | unmanaged |
| Invisible borders | `DWMWA_EXTENDED_FRAME_BOUNDS` delta | same | same | same |
| Borders | `DWMWA_BORDER_COLOR` (1 px, solid) | Direct2D overlay windows (thick, any colour), or DWM colour | WinUI focus-indicator window | — |
| Animations | none (has a "fade" from cloak) | **DWM-thumbnail "ghost" windows** (see §3.5) | none | none |
| Crash recovery | separate **watcher process** restores windows when the WM dies | `komorebi.hwnd.json` ledger of hidden HWNDs, restored on next start / exit | — | — |

### Known painful bugs (from the issue trackers) and the lesson for us

| Issue | Lesson |
|---|---|
| glazewm#1358 (2026-04, open): clicking an app's **native minimise while cloak-managed makes the window vanish permanently**. It disappears from the taskbar and stays gone even after the WM exits. Users lost unsaved work. | Keep a durable ledger of every HWND *we* cloaked, and uncloak on minimise, close and exit. Never leave a window cloaked that we aren't actively tracking. The watchdog must sweep it. |
| glazewm#589 / #1215 (open since 2024): **Win+key combos also open the Start menu**, worse on quick taps (a race). | On Win key-up after a consumed combo, inject a no-op "mask" key (`vkE8`, the AutoHotkey `MenuMaskKey` technique) so the shell never sees a bare Win tap. Handle it in the hook, synchronously. |
| glazewm#1401, komorebi#781: **JetBrains IDEs flicker** on every focus change because of no-op `SetWindowPos` calls. | Never call `SetWindowPos` when the target rect and z-order are unchanged. Diff against the last applied geometry. |
| glazewm#977: **WSLg windows (`msrdc`) freeze** when managed. | Default ignore rule for `msrdc.exe` / RAIL windows. Document it. |
| glazewm#881: taskbar reappears over borderless-fullscreen apps after a workspace switch. | Our fullscreen sets `ITaskbarList2::MarkFullscreenWindow`. In overlay mode the taskbar is hidden anyway. |
| komorebi#879: **workspace-switching loop**; #1673: minimised window treated as displayed. | Events caused by our own actions must be suppressed (an "expected event" set with a short TTL). Otherwise we fight ourselves. |
| komorebi#988: WM fails to start when an elevated window exists. | Every Win32 call on a foreign window must be fallible and non-fatal (access denied is normal). |
| komorebi#1237: elevated Windows Terminal is unmanaged. | Detect elevation up front (`OpenProcess` + `TokenElevation`). Float the window and tell the user why, instead of silently failing. |
| komorebi#1541: open animation lags. | Animations must never block the event loop. Run them on a separate render thread and make them cancellable. |

---

## 2. Hyprland: the spec we clone

### 2.1 Config-format change you should know about (decision needed)

- Hyprland's latest stable release is **0.56.2 (2026-08-05)**.
- **Since 0.55, Hyprland's config is Lua** (`hl.bind("SUPER + Q", hl.dsp.exec_cmd("kitty"))`, `hl.window_rule({...})`). The classic `hyprland.conf` hyprlang format is **deprecated**, and compatibility is kept "for a few releases". The official wiki now documents Lua only. The last hyprlang docs are the 0.54 pages, which I extracted from the wiki git history (commit before `e99eac8`, dated 2026-04-20).
- The hyprlang format itself also changed: `windowrulev2 = float, class:^(x)$` (older) became `windowrule = match:class x, float on` / `windowrule { name = …; match:class = …; … }` (0.48–0.54).
- **Implication:** "paste your hyprland.conf" now covers three dialects: `windowrulev2`-era, `match:`-era hyprlang, and Lua. The ADR proposes all hyprlang dialects first, with Lua as a follow-up, and this is flagged for your decision.

### 2.2 Dispatchers (hyprlang names, 0.54)

exec, execr, pass, sendshortcut, sendkeystate, killactive, forcekillactive, closewindow, killwindow, signal, signalwindow, **workspace, movetoworkspace, movetoworkspacesilent, togglefloating, setfloating, settiled, fullscreen, fullscreenstate**, dpms, forceidle, **pin, movefocus, movewindow, swapwindow, centerwindow, resizeactive, moveactive**, resizewindowpixel, movewindowpixel, cyclenext, swapnext, tagwindow, focuswindow, focusmonitor, movecursortocorner, movecursor, renameworkspace, exit, forcerendererreload, movecurrentworkspacetomonitor, focusworkspaceoncurrentmonitor, moveworkspacetomonitor, swapactiveworkspaces, bringactivetotop (deprecated), alterzorder, **togglespecialworkspace**, focusurgentorlast, **togglegroup, changegroupactive**, focuscurrentorlast, lockgroups, lockactivegroup, moveintogroup, moveoutofgroup, movewindoworgroup, movegroupwindow, denywindowfromgroup, setignoregrouplock, global, **submap**, event, setprop, toggleswallow, **layoutmsg**, **pseudo**.

Lua (0.55+) renames these under `hl.dsp.*` (for example `hl.dsp.window.float`, `hl.dsp.workspace.toggle_special`). If Lua is supported, the mapping is mechanical.

### 2.3 Binds

- Syntax: `bind[flags] = MODS, key, dispatcher, params`.
- Mods: `SUPER`, `SHIFT`, `CTRL`/`CONTROL`, `ALT`, `MOD2`–`MOD5`, joined by `_` or space.
- Keys are xkb keysyms (`Q`, `Return`, `left`, `XF86AudioRaiseVolume`), `code:NN`, `mouse:272/273/274/275/276`, `mouse_up` / `mouse_down` (wheel), or `switch:`.
- Flags:
  - `l` locked
  - `r` release
  - `c` click
  - `g` drag
  - `o` long-press
  - `e` repeat
  - `n` non-consuming
  - `m` mouse
  - `t` transparent
  - `i` ignore-mods
  - `s` separate (keysym combos)
  - `d` description
  - `p` bypass inhibit
  - `u` submap-universal
  - `k` per-device
- `bindm = SUPER, mouse:272, movewindow` and `bindm = SUPER, mouse:273, resizewindow`.
- `unbind`, submaps (`submap = name` … `submap = reset`, nesting, catch-all).

### 2.4 Layouts

- **Dwindle**
  - BSP tree. Split direction comes from the parent node's W/H (W > H means side-by-side) unless `preserve_split` is set.
  - Options: `force_split` (0 = follow mouse, 1 = left/top, 2 = right/bottom), `preserve_split`, `smart_split`, `smart_resizing`, `permanent_direction_override`, `special_scale_factor`, `split_width_multiplier`, `use_active_for_splits`, `default_split_ratio` (0.1–1.9), `split_bias`, `precise_mouse_move`.
  - layoutmsg: `togglesplit`, `swapsplit`, `preselect <dir>`, `movetoroot`, `rotatesplit`, `splitratio`.
  - Pseudotile is a dispatcher.
- **Master**
  - Options: `allow_small_split`, `special_scale_factor`, `mfact`, `new_status` (master/slave/inherit), `new_on_top`, `new_on_active`, `orientation` (left/right/top/bottom/center), `slave_count_for_center_master`, `center_master_fallback`, `smart_resizing`, `drop_at_cursor`, `always_keep_position`.
  - layoutmsg: `swapwithmaster`, `focusmaster`, `cyclenext/prev`, `swapnext/prev`, `addmaster`, `removemaster`, `orientation*`, `mfact`, `rollnext/prev`.
- Newer layouts (0.55+): `scrolling` (niri-like) and user-defined Lua layouts. These are out of scope for v1, but noted.

### 2.5 Animations

- Syntax: `animation = NAME, ONOFF, SPEED(ds), CURVE[, STYLE]` and `bezier = NAME, x0, y0, x1, y1`.
- The animation tree:
  - `global`
    - `windows` → `windowsIn` / `windowsOut` / `windowsMove`. Styles: `slide`, `popin [%]`, `gnomed`.
    - `layers` → `layersIn` / `layersOut`
    - `fade` → `fadeIn` / `fadeOut` / `fadeSwitch` / `fadeShadow` / `fadeDim` / `fadeLayers*` / `fadePopups*` / `fadeDpms`
    - `border`
    - `borderangle` (`once` / `loop`)
    - `workspaces` → `workspacesIn` / `workspacesOut` / `specialWorkspace(In|Out)`. Styles: `slide`, `slidevert`, `fade`, `slidefade [%]`, `slidefadevert [%]`.
    - `zoomFactor`
    - `monitorAdded`

### 2.6 Rules, IPC, hyprctl

- **Window rules (0.48–0.54):**
  - Props: `match:class`, `title`, `initial_class`, `initial_title`, `tag`, `xwayland`, `float`, `fullscreen`, `pin`, `focus`, `group`, `workspace`, `content`, …
  - Effects: `float`, `tile`, `fullscreen`, `maximize`, `move`, `size`, `center`, `pseudo`, `monitor`, `workspace`, `no_initial_focus`, `pin`, `group`, `opacity`, `border_color`, `rounding`, `no_anim`, `animation`, …
  - Also `workspace = N, monitor:X, default:true, gapsin:…, gapsout:…, layout:…` (workspace rules).
- **socket1 (hyprctl):**
  - Commands: `dispatch`, `keyword`, `reload`, `kill`, `setcursor`, `output`, `switchxkblayout`, `seterror`, `getprop`, `notify`, `dismissnotify`.
  - Info: `clients`, `workspaces`, `activeworkspace`, `monitors`, `activewindow`, `binds`, `devices`, `layers`, `version`, `getoption`, `cursorpos`, `animations`, …
  - `--batch` and `-j` (JSON).
- **socket2 events** (`EVENT>>DATA\n`): `workspace(v2)`, `focusedmon(v2)`, `activewindow(v2)`, `fullscreen`, `monitorremoved/added(v2)`, `createworkspace/destroyworkspace/moveworkspace(v2)`, `renameworkspace`, `activespecial(v2)`, `activelayout`, `openwindow`, `closewindow`, `kill`, `movewindow(v2)`, `openlayer`, `closelayer`, `submap`, `changefloatingmode`, `urgent`, `screencast(v2)`, `windowtitle(v2)`, `togglegroup`, `moveintogroup`, `moveoutofgroup`, `ignoregrouplock`, `lockgroups`, `configreloaded`, `pin`, `minimized`, `bell`.

---

## 3. Windows APIs and their limits on the current build

### 3.1 Window events — `SetWinEventHook`

- Use `WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS`. This needs **no DLL injection**. Events are queued to our thread's message loop, so idle means blocked in `GetMessage`, which means **0 % CPU**.
- Events we need: `OBJECT_CREATE/DESTROY/SHOW/HIDE/CLOAKED/UNCLOAKED/NAMECHANGE/LOCATIONCHANGE`, `SYSTEM_FOREGROUND/MINIMIZESTART/MINIMIZEEND/MOVESIZESTART/MOVESIZEEND`.
- `LOCATIONCHANGE` is very chatty (cursor and caret too). Register it in a separate narrow hook and filter on `idObject == OBJID_WINDOW` before doing any work.
- Out-of-context hooks work across elevation boundaries for *receiving* events (they're just notifications). *Acting* on an elevated window is blocked by UIPI.

### 3.2 Positioning — `SetWindowPos` / `DeferWindowPos`, invisible borders

- `DeferWindowPos` batching applies only to windows that share a parent. For top-level windows of different processes it falls back to one call per window, and one hung app can stall the whole batch.
- **Decision:** call `SetWindowPos` per window with `SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | SWP_NOZORDER` (plus `SWP_NOCOPYBITS` for size changes). A hung app then never blocks the WM.
- **Measured:** 60 `SetWindowPos` calls on a foreign window took **12.9–25.6 ms**, about **0.2–0.4 ms each**.
- **Invisible borders, measured:** `GetWindowRect` vs `DWMWA_EXTENDED_FRAME_BOUNDS` differ by **11 / 0 / 11 / 11 px (L/T/R/B)** at 150 %. That's 7 px × 1.5. Top is 0 because the caption has no resize margin.
  - Gaps are pixel-exact only if we read the delta **per window, every time**. It varies with DPI, window style and app (Chromium and UWP report 0).
  - We set `outer = target + delta`.
  - Read the frame bounds *after* the first move, because some apps change their frame on DPI change.
- **Apps that fight positioning:**
  - Respect `WM_GETMINMAXINFO` minimums. If a tile is smaller than the minimum, float the window or overflow, like FancyWM.
  - Cap retries: if an app moves itself back more than 3 times within 1 s, stop fighting, mark it "unmanageable-geometry" and float it.
  - Never apply a no-op move (JetBrains flicker).

### 3.3 DWM attributes — all verified on a foreign process's window

| Call | Result on 26300 | Notes |
|---|---|---|
| `DwmSetWindowAttribute(DWMWA_CLOAK)` | **`0x80070005` E_ACCESSDENIED** | As documented, only works on your own windows |
| `IApplicationView::SetCloak(1, 2)` via `ImmersiveShell` → `IApplicationViewCollection(1841C6D7…)` → `GetViewForHwnd` | **S_OK**. `DWMWA_CLOAKED` becomes **2 (shell-cloaked)**. Pixel check confirms it is **invisible**. Uncloak S_OK and returns to 0 | Undocumented. Fails with `TYPE_E_ELEMENTNOTFOUND (0x8002802B)` for windows the shell has no "view" for (for example a window that was never shown) |
| `DWMWA_BORDER_COLOR` (34) | **S_OK** | Solid colour, about 1 px, Windows 11 only. `DWMWA_COLOR_NONE` hides it; `DWMWA_COLOR_DEFAULT` restores it |
| `DWMWA_WINDOW_CORNER_PREFERENCE` (33) | **S_OK** | Only `DEFAULT` / `DONOTROUND` / `ROUND` (8 px) / `ROUNDSMALL` (4 px). **Arbitrary radii are impossible.** |

The undocumented IIDs used by both major WMs (`IApplicationViewCollection 1841C6D7-…`, `IApplicationView 372E1D3B-…`) still work on 26H2. They have been stable since 2021 (Ciantic's AltTabAccessor, Windows 10 21H2+). The virtual-desktop interfaces (`IVirtualDesktopManagerInternal`, …) are a different story: their IIDs changed in 21H2, 22H2, 23H2 and 24H2, which is why libraries publish per-build tables. **We don't use Windows virtual desktops.**

Cloaked windows still show on the taskbar unless removed (`ITaskbarList::DeleteTab`). That's irrelevant while the taskbar is hidden, but it matters for anyone who keeps it.

### 3.4 Hiding methods compared

| Method | Window stays "alive" (keeps rendering, no minimise animation) | Taskbar | Risk |
|---|---|---|---|
| **Shell cloak** (`IApplicationView::SetCloak`) | Yes. DWM keeps the surface (thumbnail of a cloaked window renders, see below) | entry kept | undocumented; lost-window bug if a minimise races (glazewm#1358); Chromium pauses rendering while cloaked (occlusion tracker) |
| `SW_HIDE` | App receives `WM_SHOWWINDOW`; some apps treat hidden as closed or exit the tray | entry removed | breaks some apps; no taskbar entry |
| Minimise | Minimise animation, Alt-Tab order changes, apps throttle | entry kept | visible animation, slow |
| Move off-screen | Yes | entry kept | visible on multi-monitor edge cases; apps "restore" to on-screen positions |

**Choice:** shell cloak by default, falling back per window to off-screen parking when `GetViewForHwnd` fails. Everything goes into a durable ledger.

### 3.5 Animations — `DwmRegisterThumbnail`

Verified live:
- A thumbnail registered into **our own** top-most host window renders the source **even while the source is shell-cloaked** (red pixel confirmed).
- **120 `DwmUpdateThumbnailProperties` calls took 14 ms, about 0.12 ms each.** At 240 Hz (4.17 ms per frame) we can move about 30 thumbnails per frame on one core, comfortably.

Constraints:
- The destination must be a top-level window owned by us, and the source must be top-level.
- Thumbnails can be scaled and moved (`rcDestination`), cropped (`rcSource`), given opacity and toggled visible.
- Thumbnails **can't be rotated or blurred**. Hyprland's `popin` (scale) and `slide` / `fade` map fine. `gnomed` (vertical squash) maps via a non-uniform `rcDestination`.

komorebi already ships this technique ("ghost" windows), which confirms it in the field. Their lessons, which we'll test ourselves in `spikes/animation/`:
1. Move the real window to its target first, then animate the thumbnail, so the final frame is crisp, not upscaled.
2. Chromium/Electron (`Chrome_WidgetWin_*`) stops painting while cloaked. For those, keep the window at its *start* rect for the duration and move it at the end, accepting a 1-frame stale texture.
3. Uncloak the real window *under* the still-visible ghost, then cross-fade to hide the swap.

Frame pacing: drive the animation from `DCompositionWaitForCompositorClock` (Windows 11), falling back to `DwmFlush` in a loop, never a fixed-interval timer. This gives the monitor's real refresh rate.

### 3.6 Keyboard — `WH_KEYBOARD_LL` vs `RegisterHotKey`

- `RegisterHotKey` **can't** take most Win+X combos that the shell already owns (Win+E, Win+D, Win+1…0 taskbar launchers, and so on). It's also all-or-nothing per combo, with no release or repeat semantics. **It can't do Hyprland binds.**
- `WH_KEYBOARD_LL` sees everything before the shell except Secure Attention Sequences. It needs a message loop on the installing thread.
  - **If the callback exceeds `LowLevelHooksTimeout` (about 1 s, lower on recent builds), Windows silently removes the hook.** So the callback must only look up a precomputed bind table and post to a channel, with no I/O and no locks shared with the WM thread. A dedicated high-priority hook thread does this.
  - A watchdog re-installs the hook if keystrokes stop arriving while the user is clearly typing. That is detectable via `GetLastInputInfo`.
- **Impossible to override:**
  - **Ctrl+Alt+Del** (SAS, handled by winlogon).
  - **Win+L**. Locking is enforced by winlogon. The only way to "free" Win+L is the per-user policy `HKCU\Software\Microsoft\Windows\CurrentVersion\Policies\System\DisableLockWorkstation = 1`, which also **removes Lock from the Start menu and Ctrl+Alt+Del screen**. This is opt-in, asks you first, and is reverted on uninstall.
- **Start-menu leak:** after a consumed `Win+key`, inject `vkE8` (unassigned) before Win key-up so the shell doesn't open Start. This fixes the GlazeWM #589/#1215 class of bug.
- **UIPI and keys:** an LL hook in a non-elevated process **does not receive keystrokes while an elevated window has focus**. Binds stop working whenever Task Manager or an admin terminal is focused (see §3.8).
- **Mouse (`bindm`, `follow_mouse`):** `WH_MOUSE_LL` with the same rules (trivial callback). Install it **only while it's needed**: when `follow_mouse != 0`, or while the bind modifier is held for `bindm`. When nothing needs it, there is zero cost to system-wide mouse latency.

### 3.7 Foreground / focus

- `SetForegroundWindow` is restricted: only the process that received the last input may steal focus. LL hooks don't count.
- Standard workaround (used by most WMs): a zero-movement `SendInput` mouse event or an Alt tap, then `SetForegroundWindow`. `AllowSetForegroundWindow` / `uiAccess` also lift the restriction.
- We'll use `SendInput` with `vkE8` (no side effects) and fall back to `AttachThreadInput` only if needed. `AttachThreadInput` is avoided by default because it can deadlock with hung apps.

### 3.8 Elevation strategy options (UIPI)

| Option | Manages elevated windows | Hotkeys work over elevated windows | Cost / risk |
|---|---|---|---|
| A. Normal user (`asInvoker`) | No. Detect them and leave them floating, untouched | No | none |
| B. Run elevated at logon via **Scheduled Task** (`HighestAvailable`) | Yes | Yes | WM runs as admin, so a WM bug has admin power. **Every `exec` would launch elevated apps** unless we de-elevate: launch through Explorer's token (`IShellDispatch2::ShellExecute` via the desktop's shell view), the documented Raymond Chen technique. A task changes system state, so it needs your consent and a clean uninstall. |
| C. `uiAccess="true"` | Yes (input, positioning, foreground) | Yes | Requires an **Authenticode-signed** binary installed in a **secure location** (Program Files). Doesn't work for unsigned dev builds. Microsoft says it's intended for assistive tech. |

### 3.9 Shell replacement vs overlay

- **Overlay (default):** Explorer keeps running (tray, notifications, Start, Settings and file dialogs all keep working).
  - Hide `Shell_TrayWnd` and every `Shell_SecondaryTrayWnd` with `ShowWindow(SW_HIDE)`. This is reversible and doesn't change your saved taskbar settings, unlike `ABM_SETSTATE` auto-hide.
  - Re-hide when Explorer restarts (`TaskbarCreated` broadcast).
  - We compute our own work area (monitor rect minus our bar's reserved edge) rather than calling `SPI_SETWORKAREA`, which would change a global setting.
  - Restore on exit, crash (watchdog) and uninstall.
- **Shell replacement (opt-in, later):** per-user `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon\Shell`. The supported equivalent is the `CustomShell` Winlogon policy.
  - Without Explorer you lose the tray host, the notification area, the Start menu, many `Win+` shortcuts and some shell COM services.
  - Escape hatches:
    - The watchdog relaunches `explorer.exe` if hyprwin fails to start twice.
    - The emergency combo restores Explorer.
    - Ctrl+Alt+Del → Task Manager → "Run new task: explorer" always works.
    - The uninstaller removes the value.
  - This changes the registry, so it asks you first and comes after the MVP.

### 3.10 DPI

- The manifest declares **PerMonitorV2**, so all coordinates are physical pixels.
- `WM_DPICHANGED` is the target app's concern. We re-read `EXTENDED_FRAME_BOUNDS` after moving a window across monitors and re-apply once if the result is off. Mixed DPI is handled by computing layout in physical pixels per monitor.
- `gaps_in`, `gaps_out` and `border_size` are **logical** in Hyprland (scaled by monitor scale). We multiply by each monitor's `GetDpiForMonitor / 96`.

### 3.11 Things that are impossible on Windows (and the closest approximation)

| Hyprland feature | Why it's impossible | Approximation |
|---|---|---|
| Blur / shadows / custom rounding on other apps' windows | DWM owns composition; no API for foreign windows | Rounding: 4 presets. Blur: only on our bar and overlays (`DWMSBT_*` backdrop / acrylic) |
| Arbitrary border width or gradient | `DWMWA_BORDER_COLOR` is solid, about 1 px | Our own click-through overlay window per visible tiled window (Direct2D). Measure cost in Phase 4. |
| Animating real windows smoothly | Apps repaint at their own pace | DWM-thumbnail ghost windows (§3.5) |
| Overriding Win+L, Ctrl+Alt+Del | winlogon / SAS | Policy opt-in for Win+L only |
| `dpms`, `forceidle`, xkb layouts, `switch:`, `setcursor` (themes), per-device binds | Different input and display stack | Partial maps (dpms → `SC_MONITORPOWER`; layouts → `ActivateKeyboardLayout`) or ❌ |
| Real `layerrule`s | No Wayland layers | Applies only to our own bar and overlays |
| Fullscreen for exclusive-fullscreen games | The game owns the display mode | Detect it (`SHQueryUserNotificationState` == `QUNS_RUNNING_D3D_FULL_SCREEN`) and suspend tiling and animations for that monitor |

---

## 4. Rust ecosystem check (crates.io today)

| Crate | Version | Purpose |
|---|---|---|
| `windows` | 0.62.2 | Win32 / COM bindings (official Microsoft) |
| `windows-sys` | 0.61.2 | zero-cost raw bindings (hot paths: hooks) |
| `regex` | 1.13.1 | window-rule matching |
| `serde_json` | 1.0.154 | `-j` output, state files |
| `notify` | 8.2.0 | config hot reload (or raw `ReadDirectoryChangesW`) |
| `tracing` | 0.1.44 | structured logging |
| `interprocess` | 2.4.5 | named pipes (or hand-rolled over `windows`) |
| `mlua` | 0.12.2 | *only if* we support Lua config (vendored Lua 5.4, about 300–400 KB) |

Rust isn't installed on this machine yet (`rustc` / `cargo` not found); `git` 2.50 and `gh` are.

---

## Sources

- Repos read locally (shallow clones, 2026-10-16): [glzr-io/glazewm](https://github.com/glzr-io/glazewm), [LGUG2Z/komorebi](https://github.com/LGUG2Z/komorebi), [dalyIsaac/Whim](https://github.com/dalyIsaac/Whim), [glzr-io/zebar](https://github.com/glzr-io/zebar), [amnweb/yasb](https://github.com/amnweb/yasb), [hyprwm/hyprland-wiki](https://github.com/hyprwm/hyprland-wiki)
- komorebi licensing: [komorebi.lgug2z.com/about/licensing](https://komorebi.lgug2z.com/about/licensing/), [license registration](https://komorebi.lgug2z.com/guides/license-registration/)
- Issues: [glazewm#1358](https://github.com/glzr-io/glazewm/issues/1358), [glazewm#1215](https://github.com/glzr-io/glazewm/issues/1215), [glazewm#589](https://github.com/glzr-io/glazewm/issues/589), [glazewm#1401](https://github.com/glzr-io/glazewm/issues/1401), [glazewm#977](https://github.com/glzr-io/glazewm/issues/977), [glazewm#881](https://github.com/glzr-io/glazewm/issues/881), [komorebi#879](https://github.com/LGUG2Z/komorebi/issues/879), [komorebi#988](https://github.com/LGUG2Z/komorebi/issues/988), [komorebi#1237](https://github.com/LGUG2Z/komorebi/issues/1237), [komorebi#1541](https://github.com/LGUG2Z/komorebi/issues/1541)
- Seelen UI: [github.com/eythaann/Seelen-UI](https://github.com/eythaann/Seelen-UI); FancyWM: [github.com/FancyWM/fancywm](https://github.com/FancyWM/fancywm); Whim README: [github.com/dalyIsaac/Whim](https://github.com/dalyIsaac/Whim/blob/main/README.md); OmniWM: [github.com/OmniNull/OmniWM](https://github.com/OmniNull/OmniWM/releases)
- Hyprland: [Wikipedia (0.56.2)](https://en.wikipedia.org/wiki/Hyprland), [Phoronix: 0.55 Lua config](https://www.phoronix.com/news/Hyprland-0.55-Wayland-Comp), [windowrule syntax discussion #13115](https://github.com/hyprwm/Hyprland/discussions/13115), [wiki 0.54 Dispatchers](https://wiki.hypr.land/0.54.0/Configuring/Dispatchers/), [current wiki](https://wiki.hypr.land/Configuring/Basics/Window-Rules/)
- Microsoft: [DwmRegisterThumbnail](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmregisterthumbnail), [DWM thumbnail overview](https://learn.microsoft.com/en-us/windows/win32/dwm/thumbnail-ovw), [LowLevelKeyboardProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc), [UI Automation security / uiAccess](https://learn.microsoft.com/en-gb/windows/win32/winauto/uiauto-securityoverview), [Winlogon policy CSP (CustomShell)](https://learn.microsoft.com/ro-ro/windows/client-management/mdm/policy-csp-admx-winlogon), [Win+L / DisableLockWorkstation Q&A](https://learn.microsoft.com/en-us/answers/questions/1306201/i-want-to-disable-windows-l-key-on-windows10-ltsc), [ABM_SETSTATE Q&A](https://learn.microsoft.com/en-us/answers/questions/1161409/taskbar-auto-hide-is-enabled)
- AutoHotkey mask key: [A_MenuMaskKey](https://www.autohotkey.com/docs/v2/lib/A_MenuMaskKey.htm), [#MenuMaskKey](https://www.autohotkey.com/docs/v1/lib/_MenuMaskKey.htm)
- Virtual-desktop IID churn: [Slions.VirtualDesktop](https://packages.nuget.org/packages/Slions.VirtualDesktop/6.7.0), [winvda](https://pypi.org/project/winvda/)
- Crate versions: crates.io API, 2026-10-16
