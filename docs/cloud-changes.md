# Cloud-Changes: review guide

Work done on the `Cloud-Changes` branch in a cloud session (Linux, no Windows
desktop). Every change type-checks for Windows; CI built and unit-tested every
push on a real Windows runner. **Nothing here has run on a desktop yet.** This
page lists what changed and how to check each item by hand before merging to
`main`.

## Before you start

```powershell
git fetch origin
git checkout Cloud-Changes
cargo test --workspace
cargo build --release
```

Then run the live suites as usual (see [testing.md](testing.md)), especially
`phase1.ps1`, `ux.ps1`, `border.ps1`, `taskbar.ps1`, `leak.ps1`, `keybinds.ps1`
and `restart.ps1`.

## Bugs fixed

| # | Bug | Where | How to check by hand |
|---|---|---|---|
| 1 | After **Win+L** (or Ctrl+Alt+Del / a UAC prompt) the keyboard hook thought Super was still held: plain typing could fire Super binds (e.g. `q` opens a terminal). A key whose release was lost also stopped firing its binds. | `input.rs` | Press Win+L, unlock, type `q` in Notepad: a letter appears, no terminal opens. Super+Q still works. |
| 2 | A keybind or drag that arrived while hyprwin was busy was dropped. | `main.rs` | Hard to provoke. Hammer workspace keys during an animation; none should be lost. |
| 3 | A window squeezed by a neighbour's large minimum size got a 0-size tile and showed as a 1 px window on top of the neighbour. | layout | Tile several windows including Discord or Steam (large minimums) on one workspace. No window should collapse onto another. |
| 4 | Moving a **fullscreen** window to another workspace left the old workspace "fullscreen": focusing there un-fullscreened the moved window, and moving it back made it fullscreen again. | layout | Super+F a window, Super+Shift+2, Super+1, focus a window there, then Super+2. The moved window behaves normally. |
| 5 | A pinned window that had been floated automatically (it did not fit) kept "pinned" when re-tiled, and the next workspace switch carried it away while its tile stayed behind. | layout | Rare. Covered by the stress test. |
| 6 | Moving a **minimized** window with "follow" gave it keyboard focus, so Super+C could close a window you can't see. | layout | Minimize a window, then `hyprwinctl dispatch movetoworkspace 3,address:0x…`. Super+C must not close it. |
| 7 | With many tiled windows in the scratchpad and wide gaps, a tile could end up outside its own box or off the monitor. | layout | Rare. Covered by the stress test. |
| 8 | **IPC:** a second `hyprwinctl` call while another was being served was told "hyprwin isn't running". | IPC | Run two `hyprwinctl clients` loops in parallel. No errors. |
| 9 | **IPC:** an event subscriber that stopped reading blocked event delivery to everyone, and the backlog grew in memory forever. | IPC | Start `hyprwinctl` event listening, suspend it (Ctrl+S in its console), switch workspaces a lot, resume. hyprwin's memory stays flat. |
| 10 | **Taskbar** hidden + `follow_mouse = 0`: touching the screen edge never revealed the taskbar, so the tray (and hyprwin's icon) was unreachable. | input/taskbar | Set `input { follow_mouse = 0 }` and `hyprwin { hide_taskbar = true }`, then touch the bottom edge. The taskbar shows. |
| 11 | **Guard** after a restart or update: the new guard quit because the old one still held the lock (no crash recovery or emergency key for that session). The old guard could also un-hide the *new* daemon's windows. | guard | Run `hyprwinctl dispatch exit`, start hyprwin again at once, then check the log for `guard: watching daemon pid …` for the **new** pid. Windows on other workspaces must stay hidden. |
| 12 | The tray icon leaked a handle on every pause, resume and notification. | tray | Pause and resume about 20 times. The USER object count in Task Manager stays flat (`leak.ps1` covers objects). |
| 13 | **App Groups:** saving a workspace that had a minimized tiled window scrambled the arrangement when the group was opened. | groups | Tile A, B and C, minimize B, save the group, close everything, open the group. A and C come back in their own places. |
| 14 | Reloading the config did not apply a changed `addreserved` until a monitor change. | reload | Change `addreserved` and save. Tiles move at once. |
| 15 | The keybind editor showed `movefocus, left` (valid Hyprland) as a custom action instead of "Move focus left". | config | Put `bind = SUPER, A, movefocus, left` in the config and open Settings > Keybinds. It shows as Move focus. |

## Speed

| Change | Effect |
|---|---|
| Windows sends create/show/name events for every tooltip, menu and child control. Each one used to open its process (two queries). They are now rejected with cheap checks first. | Much less work per system event, all the time |
| One process query per new window instead of two | Faster window adoption |
| Minimum-size fitting checks each candidate arrangement arithmetically instead of cloning and laying out the tree | Worst case per window change **203 ms → 6 ms** |
| `sync` and verify lay out each workspace once, not once per window | Restoring ~50 windows at startup **147 ms → 11 ms**; most operations about 2× faster |
| The crash ledger is written once per workspace switch, not once or twice per hidden window, and unchanged ledgers are not rewritten | Fewer disk writes on the UI thread when switching workspaces |
| Duplicate location-change events for one window are coalesced | Less work during drags and animations |
| The drawn border is moved, not re-rendered pixel by pixel, when only its position changes | Smoother dragging with thick or gradient borders |
| Taskbar reveal strips are cached instead of enumerating monitors on every mouse move | Less work per mouse move while the taskbar is hidden |
| `focuswindow class:…` compiles its regex once, not once per window | Faster selectors |
| Focus changes skip walking every top-level window when no floating window is on screen | Cheaper focus changes in the common all-tiled case |

## Tests added

- `crates/hyprwin-layout/tests/stress.rs`: the randomized model test that found bugs 3–7. 4000 seeds × 600 steps pass.
- `crates/hyprwin-config/tests/fuzz.rs`: 28,000 mutated or junk inputs per run. No parser panics.
- Unit tests for border translation invariance, the pruned group layout and long direction names.
- `tests/live/ux.ps1`: border-colour checks now wait for the colour to settle (up to 4 s) instead of a fixed sleep, and print a full diagnosis on failure. This should fix the one-in-ten flake.
- CI also runs on pushes to `Cloud-Changes`.

## Things worth watching

- **Tile floor (bug 3):** tiles never get smaller than 24 px plus gaps. With extreme `splitratio` values on small screens, a split may stop a few pixels short of the requested ratio. That is intended.
- **Stuck-modifier fix (bug 1):** it trusts Windows' key state for modifiers. If another tool (AutoHotkey, PowerToys) swallows Super, hyprwin now follows what apps see. Test with your usual tools running.
- **IPC threads:** each request now gets its own short-lived thread. `leak.ps1` should still show a flat handle count.
