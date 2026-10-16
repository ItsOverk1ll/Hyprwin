# Hyprland parity

Status of every Hyprland feature in hyprwin.

- ✅ done
- 🟡 approximated (how is explained)
- ❌ impossible on Windows (why is explained)
- ⏳ planned (phase)

The reference is Hyprland 0.54's hyprlang config. 0.55+ Lua config is ⏳ (see ADR-001).

## Layouts

| Feature | Status | Notes |
|---|---|---|
| dwindle layout | ✅ | BSP tree. Split direction comes from W/H × `split_width_multiplier`. |
| `force_split` 0/1/2 | ✅ | 0 uses the cursor position. |
| `preserve_split` | ✅ | |
| `default_split_ratio`, `use_active_for_splits`, `permanent_direction_override`, `special_scale_factor` | ✅ | |
| `smart_split`, `smart_resizing`, `split_bias`, `precise_mouse_move` | ⏳ P3 | Accepted silently. |
| layoutmsg `togglesplit` | ✅ | Also works without `preserve_split`: the toggled split is locked. This is a superset of Hyprland's behaviour. |
| layoutmsg `swapsplit`, `preselect`, `splitratio [exact]` | ✅ | |
| layoutmsg `movetoroot`, `rotatesplit` | ⏳ P3 | |
| pseudotile (`pseudo`) | ✅ | The window's natural size is centred in its tile. |
| master layout | ✅ | |
| `mfact`, `new_status` (master/slave/inherit), `new_on_top`, `orientation` left/right/top/bottom/center, `slave_count_for_center_master`, `allow_small_split` | ✅ | |
| master layoutmsg `swapwithmaster`, `focusmaster`, `cyclenext/prev`, `swapnext/prev`, `addmaster`, `removemaster`, `orientation*`, `mfact`, `rollnext/prev` | ✅ | |
| `new_on_active`, `drop_at_cursor`, `always_keep_position`, `center_master_fallback` other than left | ⏳ P3 | |
| scrolling layout (0.55+), custom Lua layouts | ⏳ later | |
| `gaps_in`, `gaps_out` (incl. CSS 1–4 values) | ✅ | Pixel-exact after invisible-border correction (verified live). Scaled by monitor DPI, like Hyprland's logical px. |
| per-workspace gaps (`workspace = N, gapsin:, gapsout:`) | ✅ | |

## Dispatchers

| Dispatcher | Status | Notes |
|---|---|---|
| `exec`, `execr` | ✅ | Runs through `cmd /C`, detached, with no console. `exec [rules] cmd` rules are ⏳ P3 (stripped for now). |
| `killactive`, `closewindow` | ✅ | Graceful `WM_CLOSE`. |
| `forcekillactive`, `killwindow` | ✅ | `TerminateProcess`. |
| `workspace` (id, `+N`/`-N`, `r±N`, `m±N`, `e±N`, `name:`, `previous`, `empty`, `special[:name]`) | ✅ | |
| `movetoworkspace`, `movetoworkspacesilent` (incl. `,window` selector) | ✅ | |
| `togglespecialworkspace [name]` | ✅ | Shown as an overlay: raised above the regular workspace. |
| `togglefloating`, `setfloating`, `settiled` | ✅ | |
| `fullscreen 0` | 🟡 | The frame covers the monitor and the window is marked fullscreen for the taskbar. The app's own title bar stays visible unless the app supports true fullscreen (F11). Restyling other apps' windows is unsafe. |
| `fullscreen 1` (maximize) | ✅ | Work area minus outer gaps. The app's own maximize button maps to this. |
| `fullscreenstate` | 🟡 | Mapped onto `fullscreen 0/1`. |
| `pin` | ✅ | Floating only, as in Hyprland. Follows workspace switches on its monitor. |
| `pseudo` | ✅ | |
| `movefocus` | ✅ | Crosses monitors at the edge. |
| `movewindow l/r/u/d` | ✅ | Crosses monitors at the edge. |
| `movewindow mon:` | ✅ | |
| `swapwindow` | ✅ | |
| `centerwindow` | ✅ | Floating. |
| `resizeactive`, `moveactive` (px, %, `exact`) | ✅ | |
| `resizewindowpixel`, `movewindowpixel` | ⏳ P3 | |
| `cyclenext [prev] [tiled] [floating]`, `swapnext` | ✅ | |
| `focuswindow` (class/title/initialclass/initialtitle/pid/address/floating/tiled) | ✅ | |
| `focusmonitor` (dir, ±1, name, id) | ✅ | |
| `focuscurrentorlast` | ✅ | |
| `focusurgentorlast` | 🟡 | Same as `focuscurrentorlast`: flash/urgency hints aren't tracked yet. |
| `focusworkspaceoncurrentmonitor` | ✅ | |
| `layoutmsg` | ✅ | See Layouts. |
| `submap` (incl. `reset`) | ✅ | |
| `exit` | ✅ | Restores every window first. |
| `bringactivetotop`, `alterzorder` | ✅ | |
| `forcerendererreload` | 🟡 | Reloads the config. |
| `movecurrentworkspacetomonitor`, `moveworkspacetomonitor`, `swapactiveworkspaces`, `renameworkspace` | ✅ | Monitor by name (`DISPLAY1`), index, direction, `+1`/`-1` or `current`. |
| `movecursor`, `movecursortocorner` | ⏳ P3 | |
| `tagwindow`, `setprop`, `toggleswallow`, `event`, `signal(window)` | ⏳ P3 / ❌ | `signal` is ❌: Windows has no POSIX signals. |
| `pass`, `sendshortcut`, `sendkeystate`, `global` | ⏳ P3 | |
| `dpms`, `forceidle` | ❌ | Display power and idle are owned by Windows. |
| groups: `togglegroup`, `changegroupactive`, `moveintogroup`, … | ⏳ P5 | |

## Binds

| Feature | Status | Notes |
|---|---|---|
| `bind = MODS, key, dispatcher, args` | ✅ | |
| Mods SUPER/SHIFT/CTRL/ALT (`_`, space or `+` separated), MOD4, `$vars` | ✅ | |
| Keysyms: letters, digits, F1–F24, arrows, Return, Tab, Escape, space, punctuation, KP_*, XF86Audio*, `code:NN` | ✅ | |
| `mouse:272–276`, `mouse_up` / `mouse_down` | ✅ | |
| flag `e` (repeat), `r` (release), `l` (locked), `n` (non-consuming), `m` (mouse), `i` (ignore mods), `d` (description), `t` | ✅ | `l` is accepted, but LL hooks never see the lock screen anyway. |
| flags `o` (long press), `c` (click), `g` (drag), `s` (keysym combos), `p`, `k` (per-device) | ⏳ P3 / ❌ | `k`: Windows merges all keyboards. |
| `bindm` movewindow / resizewindow (Super+LMB/RMB) | ✅ | Any window, tiled or floating. Tiled resize updates the layout live; a tiled drop re-inserts next to the window under the cursor. |
| `unbind` | ✅ | |
| submaps (incl. nested), `submap = reset` | ✅ | |
| modifier-only binds (`bindr = SUPER, SUPER_L, …`) | ⏳ P3 | |
| Super combos never open the Start menu | ✅ | `vkE8` mask (verified live). |
| **Super+L** | ❌ | Win+L is enforced by winlogon. An opt-in `DisableLockWorkstation` policy (asks you first) will free it. |
| Ctrl+Alt+Del | ❌ | Secure attention sequence. |
| Binds while an admin window is focused | ❌ (default) / ⏳ | UIPI blocks LL hooks over elevated windows unless hyprwin runs elevated (opt-in, ADR-001). |

## Config file

| Feature | Status | Notes |
|---|---|---|
| hyprlang syntax: sections, `cat:key =`, `$vars`, `source =` (with `*`), comments, `##` | ✅ | |
| `windowrulev2 = …` | ✅ | |
| legacy `windowrule = effect, regex` | ✅ | |
| `windowrule = match:…, effect` (0.48+) | ✅ | |
| `windowrule { … }` blocks | ✅ | |
| rule props `class`, `title`, `initial_class`, `initial_title`, `float`, `fullscreen`, `pin`, `focus`, `workspace`, `xwayland`, `negative:` | ✅ | Plus extensions `winclass` (Win32 class) and `exe` (path). |
| rule effects `float`, `tile`, `fullscreen`, `maximize`, `pin`, `pseudo`, `center`, `size`, `move` (px/%), `workspace [silent]`, `monitor`, `noinitialfocus`, `nofocus`, `rounding`, `bordercolor` | ✅ | Applied when the window opens. Dynamic re-evaluation is ⏳ P3. |
| `size`/`move` expressions (`monitor_w*0.5`) | ⏳ P3 | |
| `opacity` | ⏳ P4 | Opt-in, via layered windows (can break some apps). |
| `noanim`, `animation` | ⏳ | Window-rule overrides of animations are not applied yet. |
| `idleinhibit`, `suppressevent`, `stayfocused`, `dimaround`, `noblur`, `noshadow`, … | 🟡 / ❌ | Warned. Some come in P3; blur and shadow are ❌. |
| hyprwin extension `unmanaged` | ✅ | |
| hyprwin option `hyprwin { force_tile = true }` | ✅ | Only real dialogs (owned windows, `#32770`) float automatically. Always-on-top, fixed-size and borderless windows tile too. `windowrule = match:class X, tile on` forces a single app. Apps that keep repositioning themselves stay tiled; hyprwin just pauses re-tiling them for 30 s instead of floating them. |
| `workspace =` rules (`monitor:`, `default:`, `persistent:`, `gapsin:`, `gapsout:`, `layout:`) | ✅ | |
| `exec-once`, `exec`, `exec-shutdown`, `env` | ✅ | |
| `monitor =` resolution/position/scale | ❌ | Set in Windows Display settings. `addreserved` is ✅. |
| hot reload on save | ✅ | Also `hyprwinctl reload`. |
| unknown options | ✅ | A warning naming file and line, never an error. Wayland-only groups are summarised in one line. |
| `class` mapping | ✅ | The process name without `.exe`, case-insensitive. UWP apps resolve through ApplicationFrameHost to the real app. |
| Lua config (Hyprland 0.55+) | ⏳ | After hyprlang (per ADR). |

## Workspaces & monitors

| Feature | Status | Notes |
|---|---|---|
| per-monitor workspaces 1–10+, named workspaces | ✅ | Hidden with the shell cloak, so windows stay alive. Falls back to off-screen parking. |
| special workspaces (scratchpad), named | ✅ | |
| empty workspaces destroyed, `persistent` kept | ✅ | |
| `binds:workspace_back_and_forth` | ✅ | |
| monitor hotplug / resolution / DPI change | ✅ | Workspaces move to a remaining monitor. Re-detected on `WM_DISPLAYCHANGE`/`WM_DPICHANGED`/work-area change/resume. |
| persist layout across daemon restarts | ✅ | `%LOCALAPPDATA%\hyprwin\state.json`, saved 1 s after changes and on exit. It covers the workspaces, the exact split trees/ratios, each window's workspace, float/pin/pseudo state and learned minimum sizes, and the monitors' active workspaces. Survives clean exits and crashes (verified live). Matched by window handle *and* process ID, so it never applies to the wrong windows. Across a reboot the windows are new, so you start fresh. |
| minimize / restore, taskbar "Show desktop" (hyprwin extension) | ✅ | A minimized window keeps its slot (the others close up around it) and comes back to exactly the same place, whatever order Windows restores things in. hyprwin never fights a minimize. Live-tested with burst minimizes and the real Show desktop toggle. |
| apps with a minimum size (Steam, Discord, Spotify, Chromium) | ✅ (hyprwin extension) | Learned when Windows clamps a resize, confirmed on a second attempt. Dwindle split lines move so every window gets its minimum. If a workspace still can't fit everything, its most recently opened windows move silently to the next workspace on that monitor with room. At startup, the least recently used windows move first. A window that fits nowhere, even alone, floats. Windows you move yourself onto a full workspace float there and re-tile when room frees up. Hyprland has no equivalent; Linux apps usually shrink freely. |
| mixed-DPI multi-monitor | ✅ (model) / ⏳ (manual test) | |

## Look & feel

| Feature | Status | Notes |
|---|---|---|
| active/inactive border colour | ✅ | `DWMWA_BORDER_COLOR`. |
| border width, gradient borders | 🟡 | DWM borders are about 1 px and solid; a gradient uses its first colour. Overlay borders (any width/gradient) are ⏳ P4, cost to be measured. |
| `rounding` | 🟡 | Snaps to Windows' presets: 0 → square, 1–5 → small (4 px), 6+ → round (8 px). Arbitrary radii are ❌. |
| animations (`animation =`, `bezier =`) | ✅ | `windowsMove`, `windowsIn` (popin/slide/gnomed), `workspacesIn/Out` (slide/slidevert/fade/slidefade[vert]), inheritance through the animation tree, `animations:enabled`. Live DWM thumbnails on one click-through overlay at the monitor's refresh rate (242 fps measured); new changes retarget in-flight animations. Real windows move once, cloaked meanwhile (recorded in the crash ledger). Not yet: `windowsOut` (a closed window has no picture left), `fade*`, `border`/`borderangle`, `layers`, `specialWorkspace`. |
| blur, shadows, dim on other apps | ❌ | DWM owns composition. Our own bar and overlays can use acrylic/mica. |
| hide taskbar (`hyprwin:hide_taskbar`, `toggletaskbar`, Settings checkbox) | ✅ | Off by default. `SW_HIDE` on every taskbar, recorded in the ledger first (restored on exit, pause, crash via the guard; re-hidden after Explorer restarts). Tiles reclaim only the edge the taskbar used. Touching that screen edge reveals it until the mouse leaves; Start still opens on the Win key. |
| `decoration:active_opacity` / `inactive_opacity` | ⏳ P4 | Opt-in. |

## IPC

| Feature | Status | Notes |
|---|---|---|
| `hyprwinctl dispatch`, `clients`, `workspaces`, `activeworkspace`, `monitors`, `activewindow`, `binds`, `version`, `reload`, `-j`, `--batch` | ✅ | Hyprland-shaped JSON. |
| `hyprwinctl debug` | ✅ | Every top-level window with class/process/styles/verdict, for writing rules and filing bugs. |
| event stream (socket2) | ✅ | `hyprwinctl events`, or connect to `\\.\pipe\hyprwin-events-<user>`. |
| events emitted | ✅ | `workspace(v2)`, `focusedmon`, `activewindowv2`, `openwindow`, `closewindow`, `movewindow(v2)`, `createworkspace`, `destroyworkspace`, `activespecial`, `changefloatingmode`, `fullscreen`, `pin`, `minimized`, `submap`, `windowtitle(v2)`, `configreloaded`. |
| `keyword`, `getoption`, `notify`, `seterror`, `setcursor`, `switchxkblayout` | ⏳ P3 | |

## hyprwin extras (beyond Hyprland)

| Feature | Status | Notes |
|---|---|---|
| tray icon: pause/resume, App Groups, Themes, Settings, edit config, open log, exit | ✅ | Native Win32 in the daemon (no extra process). Re-added if Explorer restarts. |
| Settings window | ✅ | `hyprwin-settings.exe` (Rust/egui) runs only while open. Writes `settings.conf`, sourced last. |
| Themes | ✅ | Shareable hyprlang `.conf` files with `# name/author/description` headers. 6 built in, "save current look as theme", `--apply-theme`. |
| App Groups (save apps + arrangement, reopen on demand) | ✅ | Launches by AppUserModelID (Store/modern apps) or exe path, and reuses already-open single-window apps. Splash screens never fill a slot. A 45 s timeout arranges whatever arrived. Command-line arguments and window contents are not saved. |
| `togglepause`, `opengroup NAME` dispatchers; `hyprwinctl pause/resume/status/groups/groupsave/groupopen/groupdelete/settings` | ✅ | |

## Ecosystem

| Feature | Status | Notes |
|---|---|---|
| status bar (Waybar-like) | ⏳ P5 | Our own native Rust bar on the event pipe. |
| launcher | 🟡 | `$menu` opens the Apps folder by default; bind any launcher with `exec`. |
| groups / tabbed | ⏳ P5 | |
