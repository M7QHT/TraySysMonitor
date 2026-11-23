using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Text;
using System.IO;
using System.Runtime.InteropServices;
using System.Windows.Forms;
using Microsoft.Win32;

namespace TraySysMonitor
{
    static class Program
    {
        [STAThread]
        static void Main()
        {
            bool createdNew;
            using (var mutex = new System.Threading.Mutex(true, "TraySysMonitor_SingleInstance_Mutex", out createdNew))
            {
                if (!createdNew)
                {
                    return;
                }
                Application.EnableVisualStyles();
                Application.SetCompatibleTextRenderingDefault(false);
                Application.Run(new MonitorBarForm());
            }
        }
    }

    public class Settings
    {
        public bool ShowCpu = true;
        public bool ShowRam = true;
        public bool ShowGpu = true;
        public bool ShowBar = true;
        public bool AutoDock = true;
        public int CustomLeft = -1;
        public int CustomTop = -1;

        private static string SettingsDir
        {
            get
            {
                return Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                    "TraySysMonitor");
            }
        }

        private static string SettingsPath
        {
            get { return Path.Combine(SettingsDir, "settings.ini"); }
        }

        public static Settings Load()
        {
            var s = new Settings();
            try
            {
                if (!File.Exists(SettingsPath)) return s;
                foreach (string rawLine in File.ReadAllLines(SettingsPath))
                {
                    string line = rawLine.Trim();
                    if (line.Length == 0 || line.StartsWith("#")) continue;
                    int idx = line.IndexOf('=');
                    if (idx <= 0) continue;
                    string key = line.Substring(0, idx).Trim();
                    string val = line.Substring(idx + 1).Trim();
                    if (key == "ShowCpu") bool.TryParse(val, out s.ShowCpu);
                    else if (key == "ShowRam") bool.TryParse(val, out s.ShowRam);
                    else if (key == "ShowGpu") bool.TryParse(val, out s.ShowGpu);
                    else if (key == "ShowBar") bool.TryParse(val, out s.ShowBar);
                    else if (key == "AutoDock") bool.TryParse(val, out s.AutoDock);
                    else if (key == "CustomLeft") int.TryParse(val, out s.CustomLeft);
                    else if (key == "CustomTop") int.TryParse(val, out s.CustomTop);
                }
                if (!s.ShowCpu && !s.ShowRam && !s.ShowGpu)
                {
                    s.ShowCpu = true;
                }
            }
            catch { }
            return s;
        }

        public void Save()
        {
            try
            {
                if (!Directory.Exists(SettingsDir))
                    Directory.CreateDirectory(SettingsDir);

                var lines = new List<string>();
                lines.Add("ShowCpu=" + ShowCpu);
                lines.Add("ShowRam=" + ShowRam);
                lines.Add("ShowGpu=" + ShowGpu);
                lines.Add("ShowBar=" + ShowBar);
                lines.Add("AutoDock=" + AutoDock);
                lines.Add("CustomLeft=" + CustomLeft);
                lines.Add("CustomTop=" + CustomTop);
                File.WriteAllLines(SettingsPath, lines.ToArray());
            }
            catch { }
        }
    }

    public class HardwareReader : IDisposable
    {
        private PerformanceCounter cpuCounter;
        private bool nvmlReady = false;
        private IntPtr nvmlDevice = IntPtr.Zero;

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Auto)]
        private class MEMORYSTATUSEX
        {
            public uint dwLength;
            public uint dwMemoryLoad;
            public ulong ullTotalPhys;
            public ulong ullAvailPhys;
            public ulong ullTotalPageFile;
            public ulong ullAvailPageFile;
            public ulong ullTotalVirtual;
            public ulong ullAvailVirtual;
            public ulong ullAvailExtendedVirtual;
            public MEMORYSTATUSEX()
            {
                dwLength = (uint)Marshal.SizeOf(typeof(MEMORYSTATUSEX));
            }
        }

        [DllImport("kernel32.dll", CharSet = CharSet.Auto, SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool GlobalMemoryStatusEx([In, Out] MEMORYSTATUSEX lpBuffer);

        [StructLayout(LayoutKind.Sequential)]
        private struct NvmlUtilization
        {
            public uint gpu;
            public uint memory;
        }

        [DllImport("nvml.dll", CallingConvention = CallingConvention.Cdecl)]
        private static extern int nvmlInit_v2();

        [DllImport("nvml.dll", CallingConvention = CallingConvention.Cdecl)]
        private static extern int nvmlShutdown();

        [DllImport("nvml.dll", CallingConvention = CallingConvention.Cdecl)]
        private static extern int nvmlDeviceGetHandleByIndex_v2(uint index, out IntPtr device);

        [DllImport("nvml.dll", CallingConvention = CallingConvention.Cdecl)]
        private static extern int nvmlDeviceGetUtilizationRates(IntPtr device, out NvmlUtilization utilization);

        public HardwareReader()
        {
            try
            {
                cpuCounter = new PerformanceCounter("Processor", "% Processor Time", "_Total");
                cpuCounter.NextValue();
            }
            catch { cpuCounter = null; }

            try
            {
                if (nvmlInit_v2() == 0)
                {
                    IntPtr dev;
                    if (nvmlDeviceGetHandleByIndex_v2(0, out dev) == 0)
                    {
                        nvmlDevice = dev;
                        nvmlReady = true;
                    }
                }
            }
            catch { nvmlReady = false; }
        }

        public int ReadCpuPercent()
        {
            if (cpuCounter == null) return 0;
            try
            {
                int v = (int)Math.Round(cpuCounter.NextValue());
                if (v < 0) v = 0;
                if (v > 100) v = 100;
                return v;
            }
            catch { return 0; }
        }

        public void ReadRam(out int percent, out double usedGb, out double totalGb)
        {
            percent = 0;
            usedGb = 0;
            totalGb = 0;
            try
            {
                var mem = new MEMORYSTATUSEX();
                if (GlobalMemoryStatusEx(mem))
                {
                    percent = (int)mem.dwMemoryLoad;
                    totalGb = mem.ullTotalPhys / (1024.0 * 1024.0 * 1024.0);
                    double availGb = mem.ullAvailPhys / (1024.0 * 1024.0 * 1024.0);
                    usedGb = totalGb - availGb;
                }
            }
            catch { }
        }

        public int ReadGpuPercent()
        {
            if (!nvmlReady || nvmlDevice == IntPtr.Zero) return -1;
            try
            {
                NvmlUtilization util;
                if (nvmlDeviceGetUtilizationRates(nvmlDevice, out util) == 0)
                {
                    int v = (int)util.gpu;
                    if (v < 0) v = 0;
                    if (v > 100) v = 100;
                    return v;
                }
            }
            catch { }
            return -1;
        }

        public void Dispose()
        {
            if (cpuCounter != null)
            {
                cpuCounter.Dispose();
                cpuCounter = null;
            }
            if (nvmlReady)
            {
                try { nvmlShutdown(); } catch { }
                nvmlReady = false;
            }
        }
    }

    public class MonitorBarForm : Form
    {
        private const int WS_EX_TOOLWINDOW = 0x00000080;
        private const int WS_EX_NOACTIVATE = 0x08000000;
        private const int HWND_TOPMOST = -1;
        private const uint SWP_NOACTIVATE = 0x0010;
        private const uint SWP_NOMOVE = 0x0002;
        private const uint SWP_NOSIZE = 0x0001;
        private const string StartupRegKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
        private const string AppName = "TraySysMonitor";

        [StructLayout(LayoutKind.Sequential)]
        private struct RECT
        {
            public int Left;
            public int Top;
            public int Right;
            public int Bottom;
        }

        [DllImport("user32.dll", SetLastError = true)]
        private static extern IntPtr FindWindow(string lpClassName, string lpWindowName);

        [DllImport("user32.dll", SetLastError = true)]
        private static extern IntPtr FindWindowEx(IntPtr hwndParent, IntPtr hwndChildAfter, string lpszClass, string lpszWindow);

        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);

        [DllImport("user32.dll")]
        private static extern IntPtr GetForegroundWindow();

        [DllImport("user32.dll")]
        private static extern IntPtr GetShellWindow();

        [DllImport("user32.dll")]
        private static extern IntPtr GetDesktopWindow();

        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool DestroyIcon(IntPtr hIcon);

        private Settings settings;
        private HardwareReader reader;
        private Timer updateTimer;
        private NotifyIcon trayIcon;
        private ContextMenuStrip menu;

        private ToolStripMenuItem miCpu;
        private ToolStripMenuItem miRam;
        private ToolStripMenuItem miGpu;
        private ToolStripMenuItem miShowBar;
        private ToolStripMenuItem miStartup;

        private int cpuPct = 0;
        private int ramPct = 0;
        private double ramUsedGb = 0;
        private double ramTotalGb = 0;
        private int gpuPct = 0;

        private bool dragging = false;
        private Point dragStartScreen;
        private Point dragStartForm;

        private Font labelFont = new Font("Segoe UI", 8.5f, FontStyle.Regular);
        private Font valueFont = new Font("Segoe UI", 9.0f, FontStyle.Bold);

        public MonitorBarForm()
        {
            settings = Settings.Load();
            reader = new HardwareReader();

            FormBorderStyle = FormBorderStyle.None;
            ShowInTaskbar = false;
            TopMost = true;
            DoubleBuffered = true;
            BackColor = Color.FromArgb(24, 24, 27);
            TransparencyKey = Color.Magenta;
            StartPosition = FormStartPosition.Manual;
            Size = new Size(210, 30);
            Cursor = Cursors.SizeAll;

            BuildMenu();
            ContextMenuStrip = menu;

            trayIcon = new NotifyIcon();
            trayIcon.Visible = true;
            trayIcon.ContextMenuStrip = menu;
            trayIcon.Text = "TraySysMonitor";
            trayIcon.DoubleClick += delegate { ToggleBar(); };

            MouseDown += OnBarMouseDown;
            MouseMove += OnBarMouseMove;
            MouseUp += OnBarMouseUp;

            updateTimer = new Timer();
            updateTimer.Interval = 1500;
            updateTimer.Tick += delegate { RefreshStats(); };
            updateTimer.Start();

            RefreshStats();
            UpdateBarLayout();
        }

        protected override CreateParams CreateParams
        {
            get
            {
                CreateParams cp = base.CreateParams;
                cp.ExStyle |= WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
                return cp;
            }
        }

        protected override bool ShowWithoutActivation
        {
            get { return true; }
        }

        private void BuildMenu()
        {
            menu = new ContextMenuStrip();

            miCpu = new ToolStripMenuItem("CPU Usage");
            miCpu.Checked = settings.ShowCpu;
            miCpu.Click += delegate { ToggleMetric("cpu"); };

            miRam = new ToolStripMenuItem("RAM Usage");
            miRam.Checked = settings.ShowRam;
            miRam.Click += delegate { ToggleMetric("ram"); };

            miGpu = new ToolStripMenuItem("GPU Usage");
            miGpu.Checked = settings.ShowGpu;
            miGpu.Click += delegate { ToggleMetric("gpu"); };

            miShowBar = new ToolStripMenuItem("Show Taskbar Bar");
            miShowBar.Checked = settings.ShowBar;
            miShowBar.Click += delegate { ToggleBar(); };

            var miResetPos = new ToolStripMenuItem("Dock Left of Clock");
            miResetPos.Click += delegate
            {
                settings.AutoDock = true;
                settings.Save();
                PositionLeftOfClock();
            };

            miStartup = new ToolStripMenuItem("Run at Windows Startup");
            miStartup.Checked = IsStartupEnabled();
            miStartup.Click += delegate { ToggleStartup(); };

            var miExit = new ToolStripMenuItem("Exit");
            miExit.Click += delegate
            {
                trayIcon.Visible = false;
                Application.Exit();
            };

            menu.Items.Add(miCpu);
            menu.Items.Add(miRam);
            menu.Items.Add(miGpu);
            menu.Items.Add(new ToolStripSeparator());
            menu.Items.Add(miShowBar);
            menu.Items.Add(miResetPos);
            menu.Items.Add(miStartup);
            menu.Items.Add(new ToolStripSeparator());
            menu.Items.Add(miExit);
        }

        private void ToggleMetric(string which)
        {
            if (which == "cpu") settings.ShowCpu = !settings.ShowCpu;
            else if (which == "ram") settings.ShowRam = !settings.ShowRam;
            else if (which == "gpu") settings.ShowGpu = !settings.ShowGpu;

            if (!settings.ShowCpu && !settings.ShowRam && !settings.ShowGpu)
            {
                if (which == "cpu") settings.ShowCpu = true;
                else if (which == "ram") settings.ShowRam = true;
                else if (which == "gpu") settings.ShowGpu = true;
            }

            miCpu.Checked = settings.ShowCpu;
            miRam.Checked = settings.ShowRam;
            miGpu.Checked = settings.ShowGpu;
            settings.Save();
            UpdateBarLayout();
            Invalidate();
        }

        private void ToggleBar()
        {
            settings.ShowBar = !settings.ShowBar;
            miShowBar.Checked = settings.ShowBar;
            settings.Save();
            Visible = settings.ShowBar && !IsForegroundFullscreen();
            if (Visible) UpdateBarLayout();
        }

        private bool IsStartupEnabled()
        {
            try
            {
                using (RegistryKey key = Registry.CurrentUser.OpenSubKey(StartupRegKey, false))
                {
                    if (key == null) return false;
                    object val = key.GetValue(AppName);
                    return val != null;
                }
            }
            catch { return false; }
        }

        private void ToggleStartup()
        {
            try
            {
                using (RegistryKey key = Registry.CurrentUser.OpenSubKey(StartupRegKey, true))
                {
                    if (key == null) return;
                    if (IsStartupEnabled())
                    {
                        key.DeleteValue(AppName, false);
                    }
                    else
                    {
                        key.SetValue(AppName, "\"" + Application.ExecutablePath + "\"");
                    }
                }
                miStartup.Checked = IsStartupEnabled();
            }
            catch { }
        }

        private void RefreshStats()
        {
            cpuPct = reader.ReadCpuPercent();
            reader.ReadRam(out ramPct, out ramUsedGb, out ramTotalGb);
            gpuPct = reader.ReadGpuPercent();

            UpdateTrayTooltip();
            UpdateTrayIconBitmap();

            if (settings.ShowBar)
            {
                bool fs = IsForegroundFullscreen();
                if (fs)
                {
                    if (Visible) Visible = false;
                }
                else
                {
                    if (!Visible) Visible = true;
                    if (settings.AutoDock && !dragging)
                    {
                        PositionLeftOfClock();
                    }
                    SetWindowPos(Handle, (IntPtr)HWND_TOPMOST, 0, 0, 0, 0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                    Invalidate();
                }
            }
            else if (Visible)
            {
                Visible = false;
            }
        }

        private void UpdateBarLayout()
        {
            int count = 0;
            if (settings.ShowCpu) count++;
            if (settings.ShowRam) count++;
            if (settings.ShowGpu) count++;
            if (count < 1) count = 1;

            int newWidth = 14 + (count * 74);
            if (Width != newWidth)
            {
                Width = newWidth;
            }
            Height = 30;

            if (settings.AutoDock)
            {
                PositionLeftOfClock();
            }
            else if (settings.CustomLeft >= 0 && settings.CustomTop >= 0)
            {
                Location = new Point(settings.CustomLeft, settings.CustomTop);
            }
            else
            {
                PositionLeftOfClock();
            }
        }

        private void PositionLeftOfClock()
        {
            try
            {
                IntPtr taskbar = FindWindow("Shell_TrayWnd", null);
                if (taskbar != IntPtr.Zero)
                {
                    RECT tbRect;
                    GetWindowRect(taskbar, out tbRect);

                    IntPtr trayNotify = FindWindowEx(taskbar, IntPtr.Zero, "TrayNotifyWnd", null);
                    int targetRight = tbRect.Right - 140;
                    if (trayNotify != IntPtr.Zero)
                    {
                        RECT notifyRect;
                        if (GetWindowRect(trayNotify, out notifyRect) && notifyRect.Left > tbRect.Left)
                        {
                            targetRight = notifyRect.Left;
                        }
                    }

                    int x = targetRight - Width - 8;
                    int y = tbRect.Top + ((tbRect.Bottom - tbRect.Top) - Height) / 2;
                    if (x < tbRect.Left + 8) x = tbRect.Left + 8;
                    if (Location.X != x || Location.Y != y)
                    {
                        Location = new Point(x, y);
                    }
                    return;
                }
            }
            catch { }

            Rectangle wa = Screen.PrimaryScreen.WorkingArea;
            Location = new Point(wa.Right - Width - 180, wa.Bottom + 6);
        }

        private bool IsForegroundFullscreen()
        {
            try
            {
                IntPtr fg = GetForegroundWindow();
                if (fg == IntPtr.Zero || fg == Handle) return false;
                if (fg == GetDesktopWindow() || fg == GetShellWindow()) return false;

                IntPtr taskbar = FindWindow("Shell_TrayWnd", null);
                if (fg == taskbar) return false;

                RECT r;
                if (!GetWindowRect(fg, out r)) return false;
                Screen scr = Screen.FromHandle(fg);
                return r.Left <= scr.Bounds.Left && r.Top <= scr.Bounds.Top &&
                       r.Right >= scr.Bounds.Right && r.Bottom >= scr.Bounds.Bottom;
            }
            catch { return false; }
        }

        private void UpdateTrayTooltip()
        {
            var parts = new List<string>();
            if (settings.ShowCpu) parts.Add(string.Format("CPU: {0}%", cpuPct));
            if (settings.ShowRam) parts.Add(string.Format("RAM: {0}% ({1:0.0}/{2:0.0} GB)", ramPct, ramUsedGb, ramTotalGb));
            if (settings.ShowGpu)
            {
                parts.Add(gpuPct >= 0 ? string.Format("GPU: {0}%", gpuPct) : "GPU: N/A");
            }
            string tip = string.Join(" | ", parts.ToArray());
            if (tip.Length > 63) tip = tip.Substring(0, 63);
            trayIcon.Text = tip;
        }

        private void UpdateTrayIconBitmap()
        {
            try
            {
                using (var bmp = new Bitmap(16, 16))
                using (var g = Graphics.FromImage(bmp))
                {
                    g.Clear(Color.FromArgb(24, 24, 27));
                    int primary = settings.ShowCpu ? cpuPct : (settings.ShowRam ? ramPct : Math.Max(0, gpuPct));
                    Color c = GetLevelColor(primary);
                    int barH = Math.Max(2, (int)Math.Round(12.0 * primary / 100.0));
                    using (var b = new SolidBrush(c))
                    {
                        g.FillRectangle(b, 2, 14 - barH, 3, barH);
                    }
                    int sec = settings.ShowRam ? ramPct : (settings.ShowGpu ? Math.Max(0, gpuPct) : cpuPct);
                    int barH2 = Math.Max(2, (int)Math.Round(12.0 * sec / 100.0));
                    using (var b2 = new SolidBrush(GetLevelColor(sec)))
                    {
                        g.FillRectangle(b2, 7, 14 - barH2, 3, barH2);
                    }
                    int trd = settings.ShowGpu ? Math.Max(0, gpuPct) : cpuPct;
                    int barH3 = Math.Max(2, (int)Math.Round(12.0 * trd / 100.0));
                    using (var b3 = new SolidBrush(GetLevelColor(trd)))
                    {
                        g.FillRectangle(b3, 12, 14 - barH3, 3, barH3);
                    }

                    IntPtr hIcon = bmp.GetHicon();
                    Icon old = trayIcon.Icon;
                    trayIcon.Icon = Icon.FromHandle(hIcon);
                    if (old != null) old.Dispose();
                    DestroyIcon(hIcon);
                }
            }
            catch { }
        }

        private Color GetLevelColor(int pct)
        {
            if (pct >= 85) return Color.FromArgb(248, 113, 113);
            if (pct >= 60) return Color.FromArgb(250, 204, 21);
            return Color.FromArgb(74, 222, 128);
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            Graphics g = e.Graphics;
            g.SmoothingMode = SmoothingMode.AntiAlias;
            g.TextRenderingHint = TextRenderingHint.ClearTypeGridFit;
            g.Clear(Color.Magenta);

            Rectangle rect = new Rectangle(0, 0, Width - 1, Height - 1);
            using (GraphicsPath path = RoundedRect(rect, 7))
            using (SolidBrush bg = new SolidBrush(Color.FromArgb(30, 30, 34)))
            using (Pen border = new Pen(Color.FromArgb(63, 63, 70), 1f))
            {
                g.FillPath(bg, path);
                g.DrawPath(border, path);
            }

            int x = 10;
            if (settings.ShowCpu)
            {
                DrawMetric(g, x, "CPU", cpuPct + "%", GetLevelColor(cpuPct));
                x += 74;
            }
            if (settings.ShowRam)
            {
                DrawMetric(g, x, "RAM", ramPct + "%", GetLevelColor(ramPct));
                x += 74;
            }
            if (settings.ShowGpu)
            {
                string gpuText = gpuPct >= 0 ? (gpuPct + "%") : "N/A";
                DrawMetric(g, x, "GPU", gpuText, gpuPct >= 0 ? GetLevelColor(gpuPct) : Color.Gray);
            }
        }

        private void DrawMetric(Graphics g, int x, string label, string val, Color valColor)
        {
            using (SolidBrush lblBrush = new SolidBrush(Color.FromArgb(161, 161, 170)))
            using (SolidBrush valBrush = new SolidBrush(valColor))
            {
                g.DrawString(label, labelFont, lblBrush, x, 7);
                g.DrawString(val, valueFont, valBrush, x + 29, 6);
            }
        }

        private GraphicsPath RoundedRect(Rectangle bounds, int radius)
        {
            int d = radius * 2;
            var path = new GraphicsPath();
            path.AddArc(bounds.X, bounds.Y, d, d, 180, 90);
            path.AddArc(bounds.Right - d, bounds.Y, d, d, 270, 90);
            path.AddArc(bounds.Right - d, bounds.Bottom - d, d, d, 0, 90);
            path.AddArc(bounds.X, bounds.Bottom - d, d, d, 90, 90);
            path.CloseFigure();
            return path;
        }

        private void OnBarMouseDown(object sender, MouseEventArgs e)
        {
            if (e.Button == MouseButtons.Left)
            {
                dragging = true;
                dragStartScreen = Cursor.Position;
                dragStartForm = Location;
            }
        }

        private void OnBarMouseMove(object sender, MouseEventArgs e)
        {
            if (dragging)
            {
                Point cur = Cursor.Position;
                Location = new Point(
                    dragStartForm.X + (cur.X - dragStartScreen.X),
                    dragStartForm.Y + (cur.Y - dragStartScreen.Y));
            }
        }

        private void OnBarMouseUp(object sender, MouseEventArgs e)
        {
            if (dragging)
            {
                dragging = false;
                settings.AutoDock = false;
                settings.CustomLeft = Location.X;
                settings.CustomTop = Location.Y;
                settings.Save();
            }
        }

        protected override void Dispose(bool disposing)
        {
            if (disposing)
            {
                if (updateTimer != null) updateTimer.Dispose();
                if (trayIcon != null) trayIcon.Dispose();
                if (reader != null) reader.Dispose();
                if (labelFont != null) labelFont.Dispose();
                if (valueFont != null) valueFont.Dispose();
            }
            base.Dispose(disposing);
        }
    }
}
