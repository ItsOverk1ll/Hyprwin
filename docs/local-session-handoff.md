# Handoff: verifying `Cloud-Changes` on the real desktop

Paste this whole file into the local (desktop) Claude session, or tell it
to read `docs/local-session-handoff.md` after checking out `Cloud-Changes`.
It explains what changed, why, and how to check every change. A shorter list
of the same changes, with measurements, is in `docs/cloud-changes.md`.

---

You're working on **hyprwin**, a Hyprland-style tiling window manager for
Windows written in Rust.

- Repository on this machine: `P:\Windows-Hyprland`
- GitHub: github.com/ItsOverk1ll/Hyprwin

## 1. Where things stand

- **`main` is at `c592d09`.** That is v0.3.0 plus the CI workflow. `Cargo.toml` still says `0.3.0`.
- **`Cloud-Changes` holds about 50 commits on top of `main`.** A cloud session made them on Linux, with no Windows desktop. To compensate, every commit was:
  - type-checked for `x86_64-pc-windows-msvc`;
  - built and unit-tested by CI on `windows-latest`;
  - run through the live PowerShell suites on GitHub's hosted Windows runners (see `.github/workflows/live.yml`).
- **The latest live run: `Cloud-Changes` passed all 15 suites.**
  - The same run, with the same scripts, on `main` fails 4 suites: chaos, misbehave, system and keys. Those failures are the bugs fixed here.
  - A 10-minute soak test also passed twice, with no handle, GDI, USER-object, memory or thread growth.
- **The runner is not a real desktop.**
  - It is Windows Server at 1024×768, with one monitor, no real keyboard or mouse, and no everyday apps.
  - **Nothing has run on the user's real desktop yet.** Your job is to verify it there.
  - The user then decides whether to merge `Cloud-Changes` into `main` and release it as the next version.

## 2. Rules from the user (non-negotiable)

- **Do not push to `main`, merge, tag or release until the user explicitly says so.** Fixes go on `Cloud-Changes`.
- **No personal email in new commits.** Use the GitHub noreply identity already set in the repo's `.git/config`. Check with `git config user.email` before committing. Past commits are fine as they are.
- **Don't install drivers, inject DLLs or change system settings without approval.**
- **Keep `hyprwin-layout` free of Win32**, so it stays unit-testable.
- **Licence:** MIT/Apache-2.0. **Config format:** hyprlang (Lua may come later). **Elevation:** hyprwin runs as a normal user; elevation is opt-in only.
- **Never run `Start-Process -Wait` on the installer** (`hyprwin-setup.exe`). It hangs.
- **The live tests stop the user's running hyprwin.** When you finish, put the user's hyprwin back:
  - copy the binaries you want them to run to `%LOCALAPPDATA%\hyprwin\bin`;
  - restart it;
  - confirm with `hyprwinctl version`.
- **User data:**
  - Real config: `%APPDATA%\hyprwin\hyprland.conf`. It contains `hyprwin { force_tile = true }`.
  - Daily binaries: `%LOCALAPPDATA%\hyprwin\bin`.
  - Log: `%LOCALAPPDATA%\hyprwin\hyprwin.log`.
  - Check which version they currently run: `hyprwinctl version`.

## 3. Before touching anything: back up

```powershell
$stamp = Get-Date -Format yyyyMMdd-HHmm
Copy-Item -Recurse "$env:APPDATA\hyprwin"            "$env:USERPROFILE\hyprwin-backup-$stamp\config"
Copy-Item -Recurse "$env:LOCALAPPDATA\hyprwin\bin"   "$env:USERPROFILE\hyprwin-backup-$stamp\bin"
```

If anything goes wrong, rollback is:

1. stop hyprwin;
2. copy the backed-up `bin` and config back;
3. start hyprwin again.

## 4. Build and run the automated tests

```powershell
cd P:\Windows-Hyprland
git status                     # must be clean; stash anything local first
git fetch origin
git checkout Cloud-Changes
git pull origin Cloud-Changes
git log --oneline -1           # expect e32f935 or later
cargo test --workspace         # unit, stress and fuzz tests; all must pass
cargo build --release --workspace
```

### Live suites

Each live suite:

- kills hyprwin;
- starts its own daemon with a temporary config, and a temporary state directory (`HYPRWIN_STATE_DIR`);
- manages only its own test windows, through `HYPRWIN_MANAGE_ONLY`.

So the user's real ledger and config are not touched. Their open windows may still get focus or keystrokes while a suite runs, so ask the user not to type during the run.

```powershell
$results = foreach ($s in 'phase1','overflow','restart','minimize','ux','anim','border','keybinds','leak','taskbar','chaos','misbehave','system','keys') {
  powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\$s.ps1
  "$s exit=$LASTEXITCODE"
}
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\setup.ps1; "setup exit=$LASTEXITCODE"
$results
```

Every suite prints `[PASS]` or `[FAIL]` lines and exits with its number of failures, so `exit=0` means it passed.

What some suites do to the machine:

- **`system.ps1`:**
  - kills and restarts `explorer.exe`;
  - changes the display resolution with `ChangeDisplaySettings`, then restores it.

  On a multi-monitor desktop, check the resolution afterwards. Ask the user before running it if they have anything open that would mind an explorer restart.
- **`keys.ps1`:** injects Super key chords. A failure that looks wrong could come from AutoHotkey or PowerToys catching keys. Note it, rerun with them closed, and report both results.
- **`setup.ps1`:** installs into a temporary directory using the zip from `package.ps1`. It does not touch the real install.

Optional extra tests:

```powershell
# Soak test: minutes of random use while sampling resources.
$env:HYPRWIN_SOAK_MIN = 10
powershell -NoProfile -ExecutionPolicy Bypass -File tests\live\soak.ps1
Remove-Item Env:\HYPRWIN_SOAK_MIN

# A bigger randomized model stress run.
$env:STRESS_SEEDS = 4000; $env:STRESS_STEPS = 600
cargo test --release -p hyprwin-layout --test stress
# To reproduce a failure: $env:STRESS_SEED=<n>; $env:STRESS_TRACE=1; then rerun.
```

**On GitHub:** a commit to `Cloud-Changes` whose message contains `[live]` runs every suite on a Windows runner, for `Cloud-Changes` and `main` side by side. `[soak]` runs the soak test. Results are uploaded as `live-results-*` artifacts.

**If a suite fails here but passed on GitHub,** suspect what the runner lacks: multiple monitors, DPI scaling, real apps, a real keyboard, or input hooks from other software. Capture:

- the suite's output;
- `hyprwinctl debug`;
- the tail of the log the suite used (it is under `%TEMP%`; suites print log lines on a mismatch).

## 5. What changed, why, and how to check each item by hand

The items are in order of importance.

### Bug fixes

#### 1. Settings could wipe the user's `hyprland.conf` (data loss)

- **Why:**
  - `themes::ensure_managed_block` and `themes::apply` read the file with `read_to_string().unwrap_or_default()`.
  - Any read failure — an ANSI or UTF-16 file, one non-UTF-8 byte, a locked file — turned into an empty string. Settings then wrote back only its managed block, erasing everything else.
  - Broken marker pairs could also duplicate text or delete user lines.
- **Now** (`crates/hyprwin-config/src/themes.rs`):
  - `read_for_edit` refuses files that aren't UTF-8 or can't be read, with an error. A missing file counts as empty.
  - `write_for_edit` writes `hyprland.conf.hyprwin-tmp`, then renames it over the original, so the write is atomic.
  - It keeps a one-time backup, `hyprland.conf.before-hyprwin`.
  - `with_managed_block` replaces only the text between the markers.
  - A stray or unclosed marker drops just that marker line.
- **Check by hand:**
  1. Use a copy of the config, not the real one.
  2. Save it as ANSI with a line `# café`.
  3. Apply a theme in Settings: you should get an error, and the file must be unchanged byte for byte.
  4. Resave it as UTF-8 and apply again: the theme applies, the user's lines are intact, and `hyprland.conf.before-hyprwin` exists.
  5. Also run `hyprwin-settings --bind "SUPER, F9, exec, notepad"` and `--apply-theme` on the copy.

#### 2. A config saved in any Notepad encoding now loads

- **Why:** one invalid byte meant the daemon ran with no config at all.
- **Now** (`parse::read_text` / `decode_text`):
  - UTF-8 with or without a BOM loads;
  - UTF-16 LE and BE load;
  - anything else is read as Windows-1252, with a warning.
- **Check:** save the config as "UTF-16 LE" in Notepad, then reload. Binds still work, and `hyprwinctl configerrors` is empty or shows only the encoding warning.

#### 3. Ordinary typing fired hotkeys after Win+L, Ctrl+Alt+Del or UAC

- **Why:**
  - Key releases that happen on the secure desktop never reach low-level hooks.
  - The hook therefore still believed Super was held, so a plain `J` fired `Super+J`.
  - Likewise, a bound key whose release was lost was treated as auto-repeat forever, so its bind never fired again.
- **Now** (`crates/hyprwin/src/input.rs`):
  - `drop_stale_modifiers()` checks the real key state with `GetAsyncKeyState` whenever a modifier is believed held.
  - A "repeat" more than 1.1 s after the last key-down counts as a new press (`REPEAT_GAP_MS`). Windows' longest auto-repeat gap is about 1 s.
- **Check:**
  1. Press Win+L, unlock, then type `qjk` in Notepad: no binds fire. Super+Q and the other binds still work.
  2. Repeat after a UAC prompt, and again after Ctrl+Alt+Del → Cancel.
  3. **Repeat all of that with AutoHotkey and/or PowerToys running**, if the user has them.
  4. Hold a bind that has the repeat flag (`binde`, e.g. the default Super+Ctrl+arrow resize) to confirm auto-repeat still works.

#### 4. A hung ("Not Responding") app froze the whole daemon

- **Why:** `SetWindowPos` and `ShowWindow` on another process's window wait for that process. Three calls did this:
  - in `keep_floating_on_top`;
  - in the Super+drag raise;
  - in `place()`'s restore of a maximized window.
- **Now:**
  - All three are asynchronous: `SWP_ASYNCWINDOWPOS` and `ShowWindowAsync`.
  - The "never leave a tiled window always-on-top" safety check moved to `check_restacked()`, at the start of each verify pass.
- **Check:**
  1. Make an app hang. Either:
     - use `$env:HYPRWIN_TARGET_MODE='hang'; target\release\animation-spike.exe target 200,80,80 100 100 500 400 hang-test` (it hangs about 1.5 s after starting); or
     - pause a process in a debugger.
  2. While it hangs, float another window, change focus around, Super+drag windows, and switch workspaces. Everything must stay instant.
- **Regression check (important):**
  - Floating windows must still stay above tiled ones.
  - **No tiled app may end up always-on-top.** Discord and Spotify were past offenders.
  - Search the log for `became always-on-top`.

#### 5. Windows from hidden workspaces appeared on screen

- **Why:**
  - Windows itself uncloaks a cloaked window when it is restored or activated: taskbar click, Alt+Tab, Win+D, a toast notification.
  - hyprwin ignored `EVENT_OBJECT_UNCLOAKED` for windows it had hidden.
- **Now:**
  - `keep_hidden()` hides such a window again. It runs on that event and on every verify pass.
  - A window that has just become the foreground window is left to the activation logic, which switches to its workspace instead.
- **Check:**
  1. Put a window on workspace 2 and go to workspace 1. Click that window in the taskbar: hyprwin switches to workspace 2, with no flicker back to 1.
  2. Do the same with Alt+Tab.
  3. Press Win+D twice, then minimize/restore several windows quickly. No window from another workspace may stay visible.
  4. Search the log for `uncloaked by the system`.

#### 6. IPC (`hyprwinctl`, status bars)

- **Why:**
  - A second `hyprwinctl` call during another one was told "hyprwin isn't running".
  - On `main`, a single client that connected and sent nothing froze IPC completely.
  - One stalled event reader blocked events for everyone, and its queue grew without limit.
  - Nested `[[BATCH]]` requests (about 100k levels) overflowed the stack.
- **Now:**
  - Each client is served on its own thread.
  - A silent client is dropped after 2 s (`read_request_timeout`).
  - Clients retry a missing pipe for 50 ms.
  - Each event subscriber has a bounded queue (4096 events) and its own writer thread; a subscriber that falls behind is dropped.
  - Nested batches are refused with an error.
  - `hyprwinctl events` exits when whatever reads its output closes.
- **Check:**
  1. Run two `while (1) { hyprwinctl clients | Out-Null }` loops in two terminals, plus `hyprwinctl events` or the user's status bar.
  2. Use the desktop normally: no errors, and no lag.

#### 7. Config parser hardening

- **Why:** the parser had no limits and didn't escape what it wrote:
  - chains of variables that each expand the previous one tenfold exhausted memory;
  - `source` loops never ended;
  - binds saved from Settings such as `exec, start https://x.com/#top` were cut off at the `#`.
- **Now:**
  - `$var` values are capped at 64 KB, with a warning.
  - A source cycle is skipped with a warning, and at most 256 files are read.
  - Written values escape `#` as `##` and turn line breaks into spaces.
- **Check:** in the Settings Keybinds tab, add an exec bind with a `#` in it, save, then reopen Settings. The bind must be intact. The bind works.

#### 8. With the taskbar hidden and `follow_mouse = 0`, the taskbar could never be revealed

- **Why:** the mouse hook only reported movement when `follow_mouse` was on. The edge reveal never fired, so the tray was unreachable.
- **Now:** a `watch_edges` flag keeps the mouse hook reporting movement while the taskbar is hidden.
- **Check:** set `input { follow_mouse = 0 }` and `hyprwin { hide_taskbar = true }`, then touch the bottom edge: the taskbar appears.

#### 9. Guard (crash recovery and the emergency hotkey)

- **Why:**
  - After a restart or update, the new guard found the old guard's mutex and quit. The session then had no crash recovery and no emergency key.
  - The old guard, on exit, could restore the ledger that now belonged to the new daemon, showing all its hidden windows.
- **Now:**
  - The new guard waits up to 10 s for the old one to exit.
  - A guard restores the ledger only if the ledger's `daemon_pid` is the daemon it watched.
- **Check:**
  1. Run `hyprwinctl dispatch exit` and start hyprwin again immediately. The guard log line reads `watching daemon pid <new pid>; emergency key Ctrl+Alt+Shift+Win+Esc armed`. Windows on hidden workspaces stay hidden, and Ctrl+Alt+Shift+Win+Esc still works.
  2. **Also run `hyprwin-setup.exe` over a running install** (launch it normally, never with `-Wait`), then check the same things.

#### 10. Layout model bugs found by the randomized stress test (`crates/hyprwin-layout/tests/stress.rs`)

Each of these was a real bug and is now fixed:

- **Zero-size tiles:** tiles now have a minimum size of 24 px, scaled for DPI (`MIN_TILE`).
- **Leftover fullscreen:** moving a fullscreen window to another workspace left the old workspace stuck "fullscreen". Fixed by `leave_ws` and `drop_fullscreen`.
- **Pinned windows:** an auto-floated pinned window stayed pinned after it was tiled again. Also, `move_pinned` now moves only floating windows.
- **Minimized focus:** minimized windows could take focus. `set_focus` now refuses them.
- **Leaking tiles:** a tile narrower than its gaps was drawn outside its box. `shrink` now keeps a centred 1 px sliver inside it.
- **Huge workspace numbers** overflowed. The arithmetic now saturates.
- **`moveworkspacetomonitor special`** treated the scratchpad as a regular workspace and corrupted the model.
- **Restoring layouts:** a restored layout now keeps fullscreen only for a window that actually lives on that workspace.

**Check:**

1. Super+F a window, Super+Shift+2 to move it, then go back: workspace 1 is not stuck fullscreen.
2. Open Discord or Steam, which have large minimum sizes, next to 3–4 other tiles.
3. Toggle a pinned float.

#### 11. App Groups

- **Why:** saving a group while a tiled window on the workspace was minimized scrambled the restored layout.
- **Now:**
  - The saved layout contains only the group's members (`workspace_layout_json_keeping`).
  - A group is limited to 32 apps.
  - Reserved Windows file names (`con`, `nul`, `com1`, and so on) are stored with a leading underscore, e.g. `_con`.
- **Check:** save a group with one window minimized, then open the group: the layout matches.

#### 12. Smaller fixes

- The tray icon leaked a handle on every refresh; icons are now cached.
- Reload re-applies `addreserved` (calls `refresh_monitors()`).
- The Settings keybind editor recognises `movefocus, left` (it understood only `l`).
- `hyprwinctl -j` works at the start or end of the command, except inside `dispatch` arguments.
- The update checker only opens this repo's release pages.
- The bezier solver can no longer run away on extreme curves.
- Messages deferred while the daemon is busy keep their data. Before, binds and drags pressed during an animation could be dropped.
- The guard's crash-recovery ledger is no longer rewritten when it hasn't changed.

### Speed

Re-check that everything still feels instant:

- **Prefilter:** a cheap check (`classify::quick_reject`) runs before any process query. Tooltips, menus and child windows no longer cost a process open.
- **One process query per window**, not two (`window::class_from_path`).
- **Ledger writes:** the ledger is written once per workspace switch instead of once per window, and skipped if unchanged.
- **Event coalescing:** queued `LOCATIONCHANGE` events for the same window are merged.
- **Dwindle minimum-size fitting** screens candidate arrangements with arithmetic first. The worst case went from 203 ms to 6 ms.
- **Tile cache:** a per-workspace cache is used by sync and verify. The worst case for adding a window went from 29 ms to 1.4 ms, and restoring about 50 windows went from 147 ms to 11 ms.
- **Drawn borders** are moved instead of re-rendered while dragging.
- **Smaller savings:**
  - taskbar reveal strips are cached;
  - the window-rule regex is compiled once;
  - the floating-on-top pass skips its window walk when there are no floating windows.

## 6. Desktop-only checks the cloud could not do

Report each one as pass, fail or not possible:

- **Multi-monitor and mixed DPI** (e.g. 100% + 150%):
  - gaps and borders are correct on each monitor;
  - `movefocus` and `movewindow` work across monitors;
  - `movetoworkspace` works onto the other monitor;
  - unplug a monitor and plug it back (or change its resolution or scale in Settings) with windows open: none are lost or left off-screen.
- **Real apps:**
  - File Explorer, Chrome/Edge, VS Code, Windows Terminal, Settings (a UWP app), Office, Discord, Steam;
  - Task Manager and an elevated (admin) window;
  - a game in exclusive fullscreen and in borderless mode.

  Check that tiling, floating, minimize/restore, borders and workspace switching all behave.
- **Lock screen and UAC with input software:** Win+L, UAC prompts and Ctrl+Alt+Del with AutoHotkey or PowerToys running (item 3).
- **The user's own `hyprland.conf`,** used through Settings: apply a theme, edit a keybind, change a setting. Test on a copy first, then on the real file once the copy passes. Confirm nothing outside the managed block changes (diff against the backup).
- **A day of normal use.** Afterwards:
  - check memory and handle counts in `hyprwinctl debug`;
  - search `%LOCALAPPDATA%\hyprwin\hyprwin.log` for `keeps repositioning`, `uncloaked by the system`, `became always-on-top`, `panic` and `error`.

## 7. Reporting back to the user

Give the user one summary with:

- the `cargo test` result;
- each live suite's exit code;
- the result of every manual check from section 5, with item numbers;
- the section 6 results;
- anything that felt slower than v0.3.0.

For every failure, include:

- the steps to reproduce it;
- `hyprwinctl debug` output;
- the relevant lines from the log.

## 8. Fixing problems and releasing

- **If something fails:**
  - fix it on `Cloud-Changes`, with a regression test where possible;
  - commit using the noreply identity, with `[live]` in the message, and push;
  - check that the GitHub live run is still 15/15.
- **Only on the user's explicit go-ahead:**
  1. Merge `Cloud-Changes` into `main`.
  2. Bump `version` in the root `Cargo.toml` (e.g. `0.4.0`) and commit.
  3. Tag `v0.4.0`. The tag must match `Cargo.toml`.
  4. Push the tag. `.github/workflows/release.yml` builds the release zip with `scripts\package.ps1`.
  5. Install it with `hyprwin-setup.exe`, never with `Start-Process -Wait`.
  6. Confirm `hyprwinctl version` shows the new version, and the user's config and binds still work.
