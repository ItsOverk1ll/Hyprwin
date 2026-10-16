# win-api-probe

This is a throwaway check that the Win32 assumptions in [ADR-001](../../docs/adr-001-architecture.md) hold on the current Windows build. Re-run it after every Windows feature update.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File probe.ps1
```

What it does:
1. Spawns a red test window in a child process.
2. Tries the documented cloak and the shell (`IApplicationView`) cloak on that window.
3. Pixel-checks the result.
4. Registers a DWM thumbnail of the cloaked window and pixel-checks that too.
5. Times thumbnail updates and `SetWindowPos` calls.
6. Measures the invisible borders.
7. Uncloaks the window and closes it.

It doesn't write to the registry or change any settings.

Results on build 26300.9711 are in `results-26300.9711.json`.
