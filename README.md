# TraySysMonitor (Rust)

Lightweight Windows taskbar hardware monitor written in Rust (`windows-sys`), docked directly on the taskbar without any system tray icon.

## Features
- **No Tray Icon:** Attaches directly onto the Windows taskbar.
- **CPU, RAM, GPU Monitoring:** Uses `GetSystemTimes`, `GlobalMemoryStatusEx`, and `nvml.dll` for minimal CPU overhead.
- **Multi-Monitor Support:** Choose **All Monitors** or select specific monitors (`Monitor 1`, `Monitor 2`, ...).
- **Adjustable Horizontal Position:** Drag left/right directly on the taskbar or nudge via the right-click menu (`Move Left` / `Move Right` / `Reset Position`) to avoid overlapping other taskbar widgets.
- **Auto-Hide on Fullscreen:** Automatically hides on any monitor running a fullscreen app or game.
- **Run at Windows Startup:** Toggleable from the right-click menu.

## Build
```powershell
cargo build --release
```
