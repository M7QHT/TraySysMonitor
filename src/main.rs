#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::ffi::c_void;
use std::fs;
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, FILETIME, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT,
    WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateRoundRectRgn,
    CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject, DrawTextW, EndPaint,
    EnumDisplayMonitors, FF_DONTCARE, FW_BOLD, FW_NORMAL, FillRect, GetMonitorInfoW, HBRUSH, HDC,
    HFONT, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, MonitorFromWindow, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, PS_SOLID, RoundRect, SelectObject, SetBkMode, SetTextColor, SetWindowRgn,
    TRANSPARENT, DT_LEFT, DT_SINGLELINE, DT_VCENTER,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetSystemTimes};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, FindWindowExW, FindWindowW, GetCursorPos, GetDesktopWindow,
    GetForegroundWindow, GetMessageW, GetShellWindow, GetWindowRect, HWND_TOPMOST, IDC_SIZEALL,
    LoadCursorW, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, PostQuitMessage,
    RegisterClassW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE,
    SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_COMMAND, WM_DESTROY, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const BAR_HEIGHT: i32 = 30;
const METRIC_WIDTH: i32 = 74;
const TIMER_ID: usize = 1;
const TIMER_MS: u32 = 1500;

const CMD_CPU: usize = 1001;
const CMD_RAM: usize = 1002;
const CMD_GPU: usize = 1003;
const CMD_MOVE_LEFT: usize = 1010;
const CMD_MOVE_RIGHT: usize = 1011;
const CMD_RESET_POS: usize = 1012;
const CMD_MON_ALL: usize = 1020;
const CMD_MON_BASE: usize = 1021; // 1021..1036 for individual monitors
const CMD_STARTUP: usize = 1050;
const CMD_EXIT: usize = 1099;

const MONITORINFOF_PRIMARY: u32 = 1;

#[repr(C)]
struct NvmlUtilization {
    gpu: u32,
    memory: u32,
}

type NvmlInitFn = unsafe extern "C" fn() -> i32;
type NvmlShutdownFn = unsafe extern "C" fn() -> i32;
type NvmlGetHandleFn = unsafe extern "C" fn(u32, *mut *mut c_void) -> i32;
type NvmlGetUtilFn = unsafe extern "C" fn(*mut c_void, *mut NvmlUtilization) -> i32;

struct HardwareReader {
    prev_idle: u64,
    prev_kernel: u64,
    prev_user: u64,
    has_prev_cpu: bool,
    nvml_device: *mut c_void,
    nvml_util_fn: Option<NvmlGetUtilFn>,
    nvml_shutdown_fn: Option<NvmlShutdownFn>,
}

impl HardwareReader {
    fn new() -> Self {
        let mut r = Self {
            prev_idle: 0,
            prev_kernel: 0,
            prev_user: 0,
            has_prev_cpu: false,
            nvml_device: null_mut(),
            nvml_util_fn: None,
            nvml_shutdown_fn: None,
        };
        let _ = r.read_cpu_percent();
        r.init_nvml();
        r
    }

    fn init_nvml(&mut self) {
        unsafe {
            let dll = to_wide("nvml.dll");
            let hmod = LoadLibraryW(dll.as_ptr());
            if hmod.is_null() {
                return;
            }
            let p_init = GetProcAddress(hmod, b"nvmlInit_v2\0".as_ptr());
            let p_handle = GetProcAddress(hmod, b"nvmlDeviceGetHandleByIndex_v2\0".as_ptr());
            let p_util = GetProcAddress(hmod, b"nvmlDeviceGetUtilizationRates\0".as_ptr());
            let p_shut = GetProcAddress(hmod, b"nvmlShutdown\0".as_ptr());

            if let (Some(f_init), Some(f_handle), Some(f_util)) = (p_init, p_handle, p_util) {
                let init_fn: NvmlInitFn = std::mem::transmute(f_init);
                let handle_fn: NvmlGetHandleFn = std::mem::transmute(f_handle);
                let util_fn: NvmlGetUtilFn = std::mem::transmute(f_util);
                if init_fn() == 0 {
                    let mut dev: *mut c_void = null_mut();
                    if handle_fn(0, &mut dev) == 0 && !dev.is_null() {
                        self.nvml_device = dev;
                        self.nvml_util_fn = Some(util_fn);
                        if let Some(f_shut) = p_shut {
                            self.nvml_shutdown_fn = Some(std::mem::transmute(f_shut));
                        }
                    }
                }
            }
        }
    }

    fn read_cpu_percent(&mut self) -> i32 {
        unsafe {
            let mut idle: FILETIME = zeroed();
            let mut kernel: FILETIME = zeroed();
            let mut user: FILETIME = zeroed();
            if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
                return 0;
            }
            let idle_u = ft_to_u64(idle);
            let kernel_u = ft_to_u64(kernel);
            let user_u = ft_to_u64(user);

            let pct = if self.has_prev_cpu {
                let d_idle = idle_u.saturating_sub(self.prev_idle);
                let d_kernel = kernel_u.saturating_sub(self.prev_kernel);
                let d_user = user_u.saturating_sub(self.prev_user);
                let total = d_kernel.saturating_add(d_user);
                if total > 0 {
                    let busy = total.saturating_sub(d_idle);
                    ((busy as f64 / total as f64) * 100.0).round() as i32
                } else {
                    0
                }
            } else {
                0
            };

            self.prev_idle = idle_u;
            self.prev_kernel = kernel_u;
            self.prev_user = user_u;
            self.has_prev_cpu = true;
            pct.clamp(0, 100)
        }
    }

    fn read_ram_percent(&self) -> i32 {
        unsafe {
            let mut mem: MEMORYSTATUSEX = zeroed();
            mem.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut mem) != 0 {
                (mem.dwMemoryLoad as i32).clamp(0, 100)
            } else {
                0
            }
        }
    }

    fn read_gpu_percent(&self) -> i32 {
        if let Some(util_fn) = self.nvml_util_fn {
            if !self.nvml_device.is_null() {
                unsafe {
                    let mut util = NvmlUtilization { gpu: 0, memory: 0 };
                    if util_fn(self.nvml_device, &mut util) == 0 {
                        return (util.gpu as i32).clamp(0, 100);
                    }
                }
            }
        }
        -1
    }
}

impl Drop for HardwareReader {
    fn drop(&mut self) {
        if let Some(shut) = self.nvml_shutdown_fn {
            unsafe {
                let _ = shut();
            }
        }
    }
}

#[derive(Clone)]
struct Settings {
    show_cpu: bool,
    show_ram: bool,
    show_gpu: bool,
    all_monitors: bool,
    selected_monitors: Vec<usize>, // 0-based monitor indices when all_monitors == false
    offset_from_right: i32,        // distance from right edge of taskbar (or clock)
    auto_dock: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_cpu: true,
            show_ram: true,
            show_gpu: true,
            all_monitors: false,
            selected_monitors: vec![0],
            offset_from_right: 160,
            auto_dock: true,
        }
    }
}

impl Settings {
    fn path() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("TraySysMonitor").join("settings.ini"))
    }

    fn load() -> Self {
        let mut s = Self::default();
        let Some(p) = Self::path() else { return s };
        let Ok(content) = fs::read_to_string(p) else { return s };
        for line in content.lines() {
            let line = line.trim();
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim();
                match k {
                    "ShowCpu" => s.show_cpu = v.eq_ignore_ascii_case("true"),
                    "ShowRam" => s.show_ram = v.eq_ignore_ascii_case("true"),
                    "ShowGpu" => s.show_gpu = v.eq_ignore_ascii_case("true"),
                    "AllMonitors" => s.all_monitors = v.eq_ignore_ascii_case("true"),
                    "AutoDock" => s.auto_dock = v.eq_ignore_ascii_case("true"),
                    "OffsetFromRight" => {
                        if let Ok(n) = v.parse::<i32>() {
                            s.offset_from_right = n;
                        }
                    }
                    "SelectedMonitors" => {
                        let list: Vec<usize> = v
                            .split(',')
                            .filter_map(|x| x.trim().parse::<usize>().ok())
                            .collect();
                        if !list.is_empty() {
                            s.selected_monitors = list;
                        }
                    }
                    _ => {}
                }
            }
        }
        if !s.show_cpu && !s.show_ram && !s.show_gpu {
            s.show_cpu = true;
        }
        if s.selected_monitors.is_empty() {
            s.selected_monitors.push(0);
        }
        s
    }

    fn save(&self) {
        let Some(p) = Self::path() else { return };
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mons: Vec<String> = self.selected_monitors.iter().map(|i| i.to_string()).collect();
        let content = format!(
            "ShowCpu={}\r\nShowRam={}\r\nShowGpu={}\r\nAllMonitors={}\r\nAutoDock={}\r\nOffsetFromRight={}\r\nSelectedMonitors={}\r\n",
            self.show_cpu,
            self.show_ram,
            self.show_gpu,
            self.all_monitors,
            self.auto_dock,
            self.offset_from_right,
            mons.join(",")
        );
        let _ = fs::write(p, content);
    }
}

#[derive(Clone)]
struct MonitorTarget {
    index: usize,
    is_primary: bool,
    hmonitor: HMONITOR,
    mon_rect: RECT,
    taskbar_hwnd: HWND,
    tb_rect: RECT,
}

struct BarWindow {
    hwnd: HWND,
    monitor_index: usize,
}

struct AppState {
    settings: Settings,
    reader: HardwareReader,
    cpu_pct: i32,
    ram_pct: i32,
    gpu_pct: i32,
    bars: Vec<BarWindow>,
    monitors: Vec<MonitorTarget>,
    label_font: HFONT,
    value_font: HFONT,
    dragging: bool,
    drag_hwnd: HWND,
    drag_start_x: i32,
    drag_start_offset: i32,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn ft_to_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

fn bar_width(s: &Settings) -> i32 {
    let mut count = 0;
    if s.show_cpu {
        count += 1;
    }
    if s.show_ram {
        count += 1;
    }
    if s.show_gpu {
        count += 1;
    }
    if count < 1 {
        count = 1;
    }
    14 + count * METRIC_WIDTH
}

unsafe extern "system" fn enum_mon_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _lprect: *mut RECT,
    lparam: LPARAM,
) -> i32 {
    let list = &mut *(lparam as *mut Vec<(HMONITOR, RECT, bool)>);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(hmon, &mut mi) != 0 {
        let is_primary = (mi.dwFlags & MONITORINFOF_PRIMARY) != 0;
        list.push((hmon, mi.rcMonitor, is_primary));
    }
    1
}

fn discover_monitors() -> Vec<MonitorTarget> {
    unsafe {
        let mut raw_mons: Vec<(HMONITOR, RECT, bool)> = Vec::new();
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(enum_mon_proc),
            &mut raw_mons as *mut _ as LPARAM,
        );
        // Primary first, then left-to-right
        raw_mons.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.left.cmp(&b.1.left)));

        let mut taskbars: Vec<(HWND, HMONITOR, RECT)> = Vec::new();
        let primary_cls = to_wide("Shell_TrayWnd");
        let tb_main = FindWindowW(primary_cls.as_ptr(), null());
        if !tb_main.is_null() {
            let mut r: RECT = zeroed();
            if GetWindowRect(tb_main, &mut r) != 0 {
                let hm = MonitorFromWindow(tb_main, MONITOR_DEFAULTTONEAREST);
                taskbars.push((tb_main, hm, r));
            }
        }

        let sec_cls = to_wide("Shell_SecondaryTrayWnd");
        let mut prev: HWND = null_mut();
        loop {
            let sec = FindWindowExW(null_mut(), prev, sec_cls.as_ptr(), null());
            if sec.is_null() {
                break;
            }
            let mut r: RECT = zeroed();
            if GetWindowRect(sec, &mut r) != 0 {
                let hm = MonitorFromWindow(sec, MONITOR_DEFAULTTONEAREST);
                taskbars.push((sec, hm, r));
            }
            prev = sec;
        }

        let mut targets = Vec::new();
        for (idx, (hmon, mon_rect, is_primary)) in raw_mons.into_iter().enumerate() {
            if let Some((tb_hwnd, _, tb_rect)) = taskbars.iter().find(|t| t.1 == hmon) {
                targets.push(MonitorTarget {
                    index: idx,
                    is_primary,
                    hmonitor: hmon,
                    mon_rect,
                    taskbar_hwnd: *tb_hwnd,
                    tb_rect: *tb_rect,
                });
            }
        }
        targets
    }
}

fn default_offset_for_taskbar(mon: &MonitorTarget) -> i32 {
    unsafe {
        if mon.is_primary {
            let notify_cls = to_wide("TrayNotifyWnd");
            let notify = FindWindowExW(mon.taskbar_hwnd, null_mut(), notify_cls.as_ptr(), null());
            if !notify.is_null() {
                let mut nr: RECT = zeroed();
                if GetWindowRect(notify, &mut nr) != 0 && nr.left > mon.tb_rect.left {
                    return (mon.tb_rect.right - nr.left).max(16) + 8;
                }
            }
        }
        110
    }
}

fn is_monitor_fullscreen(mon: &MonitorTarget, bars: &[BarWindow]) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || fg == GetDesktopWindow() || fg == GetShellWindow() {
            return false;
        }
        if fg == mon.taskbar_hwnd || bars.iter().any(|b| b.hwnd == fg) {
            return false;
        }
        let fg_mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        if fg_mon != mon.hmonitor {
            return false;
        }
        let mut r: RECT = zeroed();
        if GetWindowRect(fg, &mut r) == 0 {
            return false;
        }
        r.left <= mon.mon_rect.left
            && r.top <= mon.mon_rect.top
            && r.right >= mon.mon_rect.right
            && r.bottom >= mon.mon_rect.bottom
    }
}

fn is_startup_enabled() -> bool {
    unsafe {
        let subkey = to_wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
        let name = to_wide("TraySysMonitor");
        let mut hkey: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
            return false;
        }
        let res = RegQueryValueExW(hkey, name.as_ptr(), null(), null_mut(), null_mut(), null_mut());
        RegCloseKey(hkey);
        res == 0
    }
}

fn toggle_startup() {
    unsafe {
        let subkey = to_wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
        let name = to_wide("TraySysMonitor");
        let mut hkey: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_WRITE, &mut hkey) != 0 {
            return;
        }
        if is_startup_enabled() {
            RegDeleteValueW(hkey, name.as_ptr());
        } else {
            let mut buf = vec![0u16; 1024];
            let len = GetModuleFileNameW(null_mut(), buf.as_mut_ptr(), buf.len() as u32) as usize;
            if len > 0 {
                let exe = String::from_utf16_lossy(&buf[..len]);
                let quoted = to_wide(&format!("\"{}\"", exe));
                RegSetValueExW(
                    hkey,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    quoted.as_ptr() as *const u8,
                    (quoted.len() * 2) as u32,
                );
            }
        }
        RegCloseKey(hkey);
    }
}

fn sync_bars_and_layout(state: &mut AppState) {
    state.monitors = discover_monitors();
    let w = bar_width(&state.settings);

    let desired_indices: Vec<usize> = state
        .monitors
        .iter()
        .filter(|m| {
            state.settings.all_monitors || state.settings.selected_monitors.contains(&m.index)
        })
        .map(|m| m.index)
        .collect();

    // Remove bars that are no longer desired
    state.bars.retain(|b| {
        if desired_indices.contains(&b.monitor_index) {
            true
        } else {
            unsafe {
                DestroyWindow(b.hwnd);
            }
            false
        }
    });

    // Create missing bars
    let cls_name = to_wide("TraySysMonitorBarClass");
    for &idx in &desired_indices {
        if !state.bars.iter().any(|b| b.monitor_index == idx) {
            unsafe {
                let hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    cls_name.as_ptr(),
                    null(),
                    WS_POPUP,
                    0,
                    0,
                    w,
                    BAR_HEIGHT,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null(),
                );
                if !hwnd.is_null() {
                    let rgn = CreateRoundRectRgn(0, 0, w + 1, BAR_HEIGHT + 1, 14, 14);
                    SetWindowRgn(hwnd, rgn, 1);
                    state.bars.push(BarWindow {
                        hwnd,
                        monitor_index: idx,
                    });
                }
            }
        }
    }

    // Position & visibility for each bar
    for bar in &state.bars {
        if let Some(mon) = state.monitors.iter().find(|m| m.index == bar.monitor_index) {
            if is_monitor_fullscreen(mon, &state.bars) {
                unsafe {
                    ShowWindow(bar.hwnd, SW_HIDE);
                }
                continue;
            }

            let offset = if state.settings.auto_dock {
                default_offset_for_taskbar(mon)
            } else {
                state.settings.offset_from_right
            };

            let mut x = mon.tb_rect.right - w - offset;
            let min_x = mon.tb_rect.left + 8;
            let max_x = mon.tb_rect.right - w - 8;
            if x < min_x {
                x = min_x;
            }
            if x > max_x {
                x = max_x;
            }
            let y = mon.tb_rect.top + ((mon.tb_rect.bottom - mon.tb_rect.top) - BAR_HEIGHT) / 2;

            unsafe {
                let mut cur: RECT = zeroed();
                GetWindowRect(bar.hwnd, &mut cur);
                let cur_w = cur.right - cur.left;
                if cur_w != w {
                    let rgn = CreateRoundRectRgn(0, 0, w + 1, BAR_HEIGHT + 1, 14, 14);
                    SetWindowRgn(bar.hwnd, rgn, 1);
                }
                SetWindowPos(
                    bar.hwnd,
                    HWND_TOPMOST,
                    x,
                    y,
                    w,
                    BAR_HEIGHT,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                ShowWindow(bar.hwnd, SW_SHOWNOACTIVATE);
                InvalidateRect(bar.hwnd, null(), 0);
            }
        }
    }
}

fn level_color(pct: i32) -> u32 {
    // COLORREF is 0x00BBGGRR
    if pct >= 85 {
        0x007171F8 // red (#F87171)
    } else if pct >= 60 {
        0x0015CCFA // yellow (#FACC15)
    } else {
        0x0080DE4A // green (#4ADE80)
    }
}

fn paint_bar(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rect: RECT = zeroed();
        windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);

        let bg_brush: HBRUSH = CreateSolidBrush(0x00221E1E); // #1E1E22
        FillRect(hdc, &rect, bg_brush);
        let pen = CreatePen(PS_SOLID as i32, 1, 0x00463F3F); // #3F3F46
        let old_pen = SelectObject(hdc, pen as _);
        let old_brush = SelectObject(hdc, bg_brush as _);
        RoundRect(hdc, 0, 0, rect.right, rect.bottom, 14, 14);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
        DeleteObject(pen as _);
        DeleteObject(bg_brush as _);

        SetBkMode(hdc, TRANSPARENT as i32);

        APP.with(|cell| {
            if let Some(state) = cell.borrow().as_ref() {
                let mut x = 10;
                if state.settings.show_cpu {
                    draw_metric(
                        hdc,
                        state.label_font,
                        state.value_font,
                        x,
                        "CPU",
                        &format!("{}%", state.cpu_pct),
                        level_color(state.cpu_pct),
                    );
                    x += METRIC_WIDTH;
                }
                if state.settings.show_ram {
                    draw_metric(
                        hdc,
                        state.label_font,
                        state.value_font,
                        x,
                        "RAM",
                        &format!("{}%", state.ram_pct),
                        level_color(state.ram_pct),
                    );
                    x += METRIC_WIDTH;
                }
                if state.settings.show_gpu {
                    let txt = if state.gpu_pct >= 0 {
                        format!("{}%", state.gpu_pct)
                    } else {
                        "N/A".to_string()
                    };
                    let col = if state.gpu_pct >= 0 {
                        level_color(state.gpu_pct)
                    } else {
                        0x00909090
                    };
                    draw_metric(hdc, state.label_font, state.value_font, x, "GPU", &txt, col);
                }
            }
        });

        EndPaint(hwnd, &ps);
    }
}

unsafe fn draw_metric(
    hdc: HDC,
    lbl_font: HFONT,
    val_font: HFONT,
    x: i32,
    label: &str,
    val: &str,
    val_color: u32,
) {
    let old_font = SelectObject(hdc, lbl_font as _);
    SetTextColor(hdc, 0x00AAA1A1); // #A1A1AA
    let mut r_lbl = RECT {
        left: x,
        top: 0,
        right: x + 30,
        bottom: BAR_HEIGHT,
    };
    let mut w_lbl = to_wide(label);
    DrawTextW(
        hdc,
        w_lbl.as_mut_ptr(),
        (w_lbl.len() - 1) as i32,
        &mut r_lbl,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
    );

    SelectObject(hdc, val_font as _);
    SetTextColor(hdc, val_color);
    let mut r_val = RECT {
        left: x + 29,
        top: 0,
        right: x + METRIC_WIDTH,
        bottom: BAR_HEIGHT,
    };
    let mut w_val = to_wide(val);
    DrawTextW(
        hdc,
        w_val.as_mut_ptr(),
        (w_val.len() - 1) as i32,
        &mut r_val,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
    );
    SelectObject(hdc, old_font);
}

fn show_context_menu(hwnd: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        let mon_menu = CreatePopupMenu();

        APP.with(|cell| {
            let borrow = cell.borrow();
            let Some(state) = borrow.as_ref() else { return };

            let f_cpu = MF_STRING | if state.settings.show_cpu { MF_CHECKED } else { MF_UNCHECKED };
            let f_ram = MF_STRING | if state.settings.show_ram { MF_CHECKED } else { MF_UNCHECKED };
            let f_gpu = MF_STRING | if state.settings.show_gpu { MF_CHECKED } else { MF_UNCHECKED };

            AppendMenuW(menu, f_cpu, CMD_CPU, to_wide("CPU Usage").as_ptr());
            AppendMenuW(menu, f_ram, CMD_RAM, to_wide("RAM Usage").as_ptr());
            AppendMenuW(menu, f_gpu, CMD_GPU, to_wide("GPU Usage").as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, null());

            let f_all = MF_STRING
                | if state.settings.all_monitors {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                };
            AppendMenuW(mon_menu, f_all, CMD_MON_ALL, to_wide("All Monitors").as_ptr());
            AppendMenuW(mon_menu, MF_SEPARATOR, 0, null());

            for mon in &state.monitors {
                let checked = !state.settings.all_monitors
                    && state.settings.selected_monitors.contains(&mon.index);
                let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
                let title = if mon.is_primary {
                    format!("Monitor {} (Primary)", mon.index + 1)
                } else {
                    format!("Monitor {}", mon.index + 1)
                };
                AppendMenuW(mon_menu, flags, CMD_MON_BASE + mon.index, to_wide(&title).as_ptr());
            }

            AppendMenuW(menu, MF_POPUP, mon_menu as usize, to_wide("Monitors").as_ptr());
            AppendMenuW(
                menu,
                MF_STRING,
                CMD_MOVE_LEFT,
                to_wide("Move Left  (<- 20px)").as_ptr(),
            );
            AppendMenuW(
                menu,
                MF_STRING,
                CMD_MOVE_RIGHT,
                to_wide("Move Right (-> 20px)").as_ptr(),
            );
            AppendMenuW(
                menu,
                MF_STRING,
                CMD_RESET_POS,
                to_wide("Reset Position (Left of Clock)").as_ptr(),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());

            let f_start = MF_STRING | if is_startup_enabled() { MF_CHECKED } else { MF_UNCHECKED };
            AppendMenuW(
                menu,
                f_start,
                CMD_STARTUP,
                to_wide("Run at Windows Startup").as_ptr(),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, CMD_EXIT, to_wide("Exit").as_ptr());
        });

        let mut pt: POINT = zeroed();
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            0,
            hwnd,
            null(),
        );
        DestroyMenu(menu);
    }
}

fn handle_command(cmd: usize) {
    if cmd == CMD_EXIT {
        unsafe {
            PostQuitMessage(0);
        }
        return;
    }
    if cmd == CMD_STARTUP {
        toggle_startup();
        return;
    }

    APP.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let Some(state) = borrow.as_mut() else { return };

        match cmd {
            CMD_CPU => {
                state.settings.show_cpu = !state.settings.show_cpu;
                if !state.settings.show_cpu && !state.settings.show_ram && !state.settings.show_gpu {
                    state.settings.show_cpu = true;
                }
            }
            CMD_RAM => {
                state.settings.show_ram = !state.settings.show_ram;
                if !state.settings.show_cpu && !state.settings.show_ram && !state.settings.show_gpu {
                    state.settings.show_ram = true;
                }
            }
            CMD_GPU => {
                state.settings.show_gpu = !state.settings.show_gpu;
                if !state.settings.show_cpu && !state.settings.show_ram && !state.settings.show_gpu {
                    state.settings.show_gpu = true;
                }
            }
            CMD_MOVE_LEFT => {
                if state.settings.auto_dock {
                    if let Some(m) = state.monitors.first() {
                        state.settings.offset_from_right = default_offset_for_taskbar(m);
                    }
                    state.settings.auto_dock = false;
                }
                state.settings.offset_from_right += 20;
            }
            CMD_MOVE_RIGHT => {
                if state.settings.auto_dock {
                    if let Some(m) = state.monitors.first() {
                        state.settings.offset_from_right = default_offset_for_taskbar(m);
                    }
                    state.settings.auto_dock = false;
                }
                state.settings.offset_from_right = (state.settings.offset_from_right - 20).max(8);
            }
            CMD_RESET_POS => {
                state.settings.auto_dock = true;
            }
            CMD_MON_ALL => {
                state.settings.all_monitors = true;
            }
            x if x >= CMD_MON_BASE && x < CMD_MON_BASE + 16 => {
                let idx = x - CMD_MON_BASE;
                if state.settings.all_monitors {
                    state.settings.all_monitors = false;
                    state.settings.selected_monitors = vec![idx];
                } else if state.settings.selected_monitors.contains(&idx) {
                    if state.settings.selected_monitors.len() > 1 {
                        state.settings.selected_monitors.retain(|&m| m != idx);
                    }
                } else {
                    state.settings.selected_monitors.push(idx);
                }
            }
            _ => return,
        }

        state.settings.save();
        sync_bars_and_layout(state);
    });
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_bar(hwnd);
            0
        }
        WM_TIMER => {
            if wparam == TIMER_ID {
                APP.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.cpu_pct = state.reader.read_cpu_percent();
                        state.ram_pct = state.reader.read_ram_percent();
                        state.gpu_pct = state.reader.read_gpu_percent();
                        sync_bars_and_layout(state);
                    }
                });
            }
            0
        }
        WM_LBUTTONDOWN => {
            let mut pt: POINT = zeroed();
            GetCursorPos(&mut pt);
            APP.with(|cell| {
                if let Some(state) = cell.borrow_mut().as_mut() {
                    if state.settings.auto_dock {
                        if let Some(bar) = state.bars.iter().find(|b| b.hwnd == hwnd) {
                            if let Some(mon) =
                                state.monitors.iter().find(|m| m.index == bar.monitor_index)
                            {
                                state.settings.offset_from_right = default_offset_for_taskbar(mon);
                            }
                        }
                        state.settings.auto_dock = false;
                    }
                    state.dragging = true;
                    state.drag_hwnd = hwnd;
                    state.drag_start_x = pt.x;
                    state.drag_start_offset = state.settings.offset_from_right;
                }
            });
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let mut pt: POINT = zeroed();
            GetCursorPos(&mut pt);
            APP.with(|cell| {
                if let Some(state) = cell.borrow_mut().as_mut() {
                    if state.dragging && state.drag_hwnd == hwnd {
                        let dx = pt.x - state.drag_start_x;
                        state.settings.offset_from_right = (state.drag_start_offset - dx).max(8);
                        sync_bars_and_layout(state);
                    }
                }
            });
            0
        }
        WM_LBUTTONUP => {
            APP.with(|cell| {
                if let Some(state) = cell.borrow_mut().as_mut() {
                    if state.dragging {
                        state.dragging = false;
                        state.drag_hwnd = null_mut();
                        state.settings.save();
                    }
                }
            });
            windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
            0
        }
        WM_RBUTTONUP => {
            show_context_menu(hwnd);
            0
        }
        WM_COMMAND => {
            handle_command(wparam & 0xFFFF);
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    unsafe {
        let mutex_name = to_wide("TraySysMonitor_Rust_SingleInstance");
        let h_mutex = CreateMutexW(null(), 1, mutex_name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if !h_mutex.is_null() {
                CloseHandle(h_mutex);
            }
            return;
        }

        let cls_name = to_wide("TraySysMonitorBarClass");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: null_mut(),
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_SIZEALL),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: cls_name.as_ptr(),
        };
        RegisterClassW(&wc);

        let font_face = to_wide("Segoe UI");
        let label_font = CreateFontW(
            -12,
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            font_face.as_ptr(),
        );
        let value_font = CreateFontW(
            -12,
            0,
            0,
            0,
            FW_BOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            font_face.as_ptr(),
        );

        let settings = Settings::load();
        let mut reader = HardwareReader::new();
        let cpu_pct = reader.read_cpu_percent();
        let ram_pct = reader.read_ram_percent();
        let gpu_pct = reader.read_gpu_percent();

        // Hidden controller window for timer & cleanup
        let ctrl_hwnd = CreateWindowExW(
            0,
            cls_name.as_ptr(),
            null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            null(),
        );
        SetTimer(ctrl_hwnd, TIMER_ID, TIMER_MS, None);

        APP.with(|cell| {
            let mut state = AppState {
                settings,
                reader,
                cpu_pct,
                ram_pct,
                gpu_pct,
                bars: Vec::new(),
                monitors: Vec::new(),
                label_font,
                value_font,
                dragging: false,
                drag_hwnd: null_mut(),
                drag_start_x: 0,
                drag_start_offset: 0,
            };
            sync_bars_and_layout(&mut state);
            *cell.borrow_mut() = Some(state);
        });

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        APP.with(|cell| {
            if let Some(mut state) = cell.borrow_mut().take() {
                for b in state.bars.drain(..) {
                    DestroyWindow(b.hwnd);
                }
                DeleteObject(state.label_font as _);
                DeleteObject(state.value_font as _);
            }
        });
        if !ctrl_hwnd.is_null() {
            DestroyWindow(ctrl_hwnd);
        }
        if !h_mutex.is_null() {
            CloseHandle(h_mutex);
        }
    }
}
