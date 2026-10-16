# Animation spike — result: GO, using the overlay strategy

Question: can a third-party program animate *other apps'* windows on Windows 11 smoothly enough to feel like Hyprland?

Technique:
- Real windows are moved **once**.
- What you see animate is a live DWM thumbnail of each window, drawn into a transparent, click-through, top-most surface that we own.
- Windows leaving the screen are shell-cloaked; windows arriving are uncloaked under their thumbnail before it is removed.

## Run it

```powershell
cargo build --release -p animation-spike
.\target\release\animation-spike.exe selftest            # automated, uses its own test windows
.\target\release\animation-spike.exe swap                # swaps YOUR two top-most windows
.\target\release\animation-spike.exe workspace           # slides YOUR windows out and back
#   --strategy ghost|overlay   --ms 400   --naive   --curve 0.05,0.9,0.1,1.1
```

## Results (build 26300.9711, 3840×2160 @ 240 Hz, 150 %, RTX 4060)

From [`selftest.txt`](selftest.txt): **22/22 checks pass.**

| Run (300 ms, Hyprland `default` bezier) | frames | work per frame (mean / worst) | frame interval (mean / worst) | late frames |
|---|---|---|---|---|
| per-window ghost windows | 73 | 0.50 / 2.06 ms | 4.17 / 4.29 ms | 0 |
| **per-monitor overlay** | 73 | **0.09 / 0.17 ms** | 4.17 / 4.28 ms | 0 |
| workspace slide (3 windows) | 73 | 0.14 / 0.22 ms | 4.17 / 4.27 ms | 0 |
| naive `SetWindowPos` each frame | 73 | 0.03–0.14 ms | 4.17 / 5.80 ms | 0 |

Each row is **one full frame per vblank at 240 Hz**, paced by `DCompositionWaitForCompositorClock`.

The naive row looks cheap only because the test windows are trivial (a solid colour). Real apps (Chrome, VS Code, Office) take one or more frames to repaint after each resize, which is the jank this design avoids. The thumbnail path costs the same no matter how heavy the app is.

### Verified behaviours
- Visible frames land pixel-exactly where requested after correcting the invisible borders (9 px L/R/B for these windows).
- Thumbnails render on both surface types. Cropping with `rcSource` = frame bounds removes the invisible border and shadow, so the edge pixel is real content.
- Both surfaces are click-through (`WindowFromPoint` passes through). The colour-keyed overlay is transparent outside its thumbnails.
- A cloaked window is invisible, and its thumbnail still renders. That is the key fact for workspace animations.
- After every animation, the real windows are at their targets, uncloaked, and showing real content.

## Decision

Use **one colour-keyed overlay per monitor**, created only for the duration of an animation. It's about 5× cheaper per frame than per-window ghosts, and it moves all thumbnails in one atomic compositor update.

## Issues found
- **Exit hang (fixed).** Releasing a cached COM proxy from a thread-local destructor during process exit deadlocked under the loader lock, and the stuck process ignored `Stop-Process` (it took a CIM `Terminate`). The fix: hold the cache in `ManuallyDrop` and release it explicitly with `cloak::release()`. This would have hit the real daemon too.

## Still to verify (needs your real apps — run `swap` / `workspace`)
- Chromium/Electron (Chrome, VS Code, Discord) stop painting while cloaked, so the final frame may be stale for one frame or so.
- UWP apps (Settings), Office, and games.
- Mixed-DPI and second-monitor overlays. One monitor was tested here; the Meta Virtual Monitor wasn't exercised.
