# TraySysMonitor

Lightweight Windows taskbar & system-tray monitor for **CPU**, **RAM**, and **GPU** usage.

## Features
- Compact dark pill docked automatically on the Windows taskbar to the left of the clock (or draggable anywhere).
- System tray icon with live mini-bars and detailed hover tooltip.
- Right-click menu to toggle which metrics to monitor (**CPU**, **RAM**, **GPU**), show/hide the taskbar bar, re-dock left of the clock, or enable **Run at Windows Startup**.
- Auto-hides when a fullscreen app or game is active.
- Uses Win32 `GlobalMemoryStatusEx`, `PerformanceCounter`, and `nvml.dll` directly for minimal CPU overhead.

## Build (Windows)
```powershell
C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe /target:winexe /optimize+ /out:TraySysMonitor.exe Program.cs
```
