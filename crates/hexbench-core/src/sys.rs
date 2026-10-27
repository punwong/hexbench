//! Thin Win32 layer, backed by the official `windows-sys` raw bindings.
//!
//! The core crate deliberately stays close to the metal: it only needs
//! `EnumDisplayMonitors`, `GetMonitorInfoW`, `MonitorFromPoint` and
//! `GetCursorPos`, and `windows-sys` gives us those with no runtime cost and no
//! hand-maintained struct layouts.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL,
};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VK_CONTROL, VK_D, VK_LBUTTON, VK_LEFT, VK_LWIN, VK_MBUTTON, VK_RBUTTON,
    VK_RETURN, VK_RIGHT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, EVENT_SYSTEM_DESKTOPSWITCH, GetAncestor, GetClassNameW,
    GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowLongW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, MSG,
    SetCursorPos, SetWindowLongW, SetWindowPos, TranslateMessage, WindowFromPoint,
    WINEVENT_OUTOFCONTEXT, GA_ROOT, GWL_EXSTYLE, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

use crate::geometry::EdgeProbe;
use crate::model::{Monitor, Rect};
use crate::trigger::Direction;

/// `MONITORINFOF_PRIMARY` is not re-exported by `windows-sys` 0.61.
const MONITORINFOF_PRIMARY: u32 = 0x1;


/// Current cursor position in physical pixels, or `None` if the call failed.
pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: the pointer is valid for the duration of the call.
    let ok = unsafe { GetCursorPos(&mut point) };
    (ok != 0).then_some((point.x, point.y))
}

/// `true` when the given point lies on any monitor.
///
/// This is exactly `MonitorFromPoint(..., MONITOR_DEFAULTTONULL)`, which is what
/// makes the segmentation match the real desktop.
pub fn point_on_monitor(x: i32, y: i32) -> bool {
    // SAFETY: plain value in, handle out.
    let handle = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL) };
    !handle.is_null()
}

/// Work area (screen without the taskbar) of the monitor holding a point.
pub fn work_area_at(x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
    // SAFETY: plain queries on a live monitor handle.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return None;
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        Some((
            info.rcWork.left,
            info.rcWork.top,
            info.rcWork.right,
            info.rcWork.bottom,
        ))
    }
}
/// Pull a window rectangle back inside the work area of its monitor.
pub fn clamp_to_work_area(x: i32, y: i32, width: i32, height: i32) -> (i32, i32) {
    match work_area_at(x + width / 2, y + height / 2) {
        Some((left, top, right, bottom)) => {
            (x.min(right - width).max(left), y.min(bottom - height).max(top))
        }
        None => (x, y),
    }
}
/// All active displays, in enumeration order.
pub fn enumerate_monitors() -> Vec<Monitor> {
    let mut out: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only writes into `out`, which outlives the call.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(enum_proc),
            &mut out as *mut Vec<Monitor> as LPARAM,
        );
    }
    out
}

unsafe extern "system" fn enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` is the `&mut Vec<Monitor>` passed to EnumDisplayMonitors.
    let out = unsafe { &mut *(data as *mut Vec<Monitor>) };
    if let Some(monitor) = read_monitor(hmonitor) {
        out.push(monitor);
    }
    TRUE // keep enumerating
}

fn read_monitor(hmonitor: HMONITOR) -> Option<Monitor> {
    // SAFETY: MONITORINFOEXW is a plain-old-data struct; all-zero is valid.
    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: cbSize is set and `info` is correctly sized/aligned.
    let ok = unsafe { GetMonitorInfoW(hmonitor, &mut info.monitorInfo) };
    if ok == 0 {
        return None;
    }
    let len = info
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szDevice.len());
    let device = String::from_utf16_lossy(&info.szDevice[..len]);

    Some(Monitor {
        rect: Rect::new(
            info.monitorInfo.rcMonitor.left,
            info.monitorInfo.rcMonitor.top,
            info.monitorInfo.rcMonitor.right,
            info.monitorInfo.rcMonitor.bottom,
        ),
        work: Rect::new(
            info.monitorInfo.rcWork.left,
            info.monitorInfo.rcWork.top,
            info.monitorInfo.rcWork.right,
            info.monitorInfo.rcWork.bottom,
        ),
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        device,
    })
}

/// Probe backed by the real desktop.
pub struct SystemProbe;

impl EdgeProbe for SystemProbe {
    fn covered(&self, x: i32, y: i32) -> bool {
        point_on_monitor(x, y)
    }
}

// ---------------------------------------------------------------------------
// Input, guards and cursor helpers
// ---------------------------------------------------------------------------

/// Send `Win+Ctrl+Left/Right`.
///
/// Returns `false` when Windows rejected the injection — most commonly because
/// the foreground window belongs to an elevated process (UIPI). The caller is
/// expected to surface that once instead of failing silently.
pub fn send_desktop_switch(direction: Direction) -> bool {
    let arrow = match direction {
        Direction::Prev => VK_LEFT,
        Direction::Next => VK_RIGHT,
    };
    send_chord(arrow)
}

/// One chord, sent in three batches.
///
/// Submitting all events in a single `SendInput` call is not reliable for
/// shell hotkeys: Explorer's hotkey state machine has not seen the modifiers
/// yet when the final key arrives, so `Win+Ctrl+D` is swallowed. A short gap
/// between "modifiers down" and "key press" mirrors what a human types.
fn send_chord(key: u16) -> bool {
    let modifiers_down = [key_input(VK_LWIN, false), key_input(VK_CONTROL, false)];
    let mut ok = send_inputs(&modifiers_down);
    thread::sleep(Duration::from_millis(30));

    let press = [key_input(key, false), key_input(key, true)];
    ok &= send_inputs(&press);
    thread::sleep(Duration::from_millis(20));

    // Always release the modifiers, even if the press failed half-way.
    let modifiers_up = [key_input(VK_CONTROL, true), key_input(VK_LWIN, true)];
    ok &= send_inputs(&modifiers_up);
    ok
}

fn send_inputs(inputs: &[INPUT]) -> bool {
    // SAFETY: the slice is valid for `len` INPUT structures.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    sent == inputs.len() as u32
}

/// Number of virtual desktops Explorer knows about.
///
/// Explorer stores one 16-byte GUID per desktop under
/// `HKCU\...\Explorer\VirtualDesktops\VirtualDesktopIDs`. This is not a formal
/// API, but it is the least invasive way to answer "is there even a second
/// desktop?" without the undocumented `ImmersiveShell` COM interfaces.
/// Returns `None` when the value is missing or malformed.
pub fn virtual_desktop_count() -> Option<usize> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops")
        .ok()?;
    let ids = key.get_raw_value("VirtualDesktopIDs").ok()?.bytes;
    if ids.is_empty() {
        None
    } else {
        Some(ids.len() / 16)
    }
}

/// Desktop count plus the 1-based index of the active one.
///
/// Both come from the same registry key, so they are consistent with each other.
pub fn virtual_desktop_state() -> Option<(usize, usize)> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops")
        .ok()?;
    let ids = key.get_raw_value("VirtualDesktopIDs").ok()?.bytes;
    if ids.is_empty() {
        return None;
    }
    let count = ids.len() / 16;
    let current = key.get_raw_value("CurrentVirtualDesktop").ok()?.bytes;
    let index = ids
        .chunks_exact(16)
        .position(|chunk| chunk == current.as_ref())
        .map(|position| position + 1)?;
    Some((count, index))
}

/// GUID of the desktop that is currently active.
pub fn current_desktop_id() -> Option<Vec<u8>> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops")
        .ok()?;
    let ids = key.get_raw_value("CurrentVirtualDesktop").ok()?.bytes;
    (!ids.is_empty()).then(|| ids.into_owned())
}

/// Create a new virtual desktop (Win+Ctrl+D). Windows switches to it.
pub fn create_virtual_desktop() -> bool {
    send_chord(VK_D)
}

fn key_input(vk: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn unicode_input(unit: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE | if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Type text into the focused window with unicode input.
///
/// The clipboard is never touched, so a phrase click cannot destroy whatever
/// the user had copied. Classic edit controls and Chromium inputs both accept
/// the resulting `WM_CHAR`.
pub fn type_text(text: &str) -> bool {
    for unit in text.encode_utf16() {
        let inputs = [unicode_input(unit, false), unicode_input(unit, true)];
        // SAFETY: two valid INPUT structures.
        let sent = unsafe {
            SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32)
        };
        if sent != 2 {
            return false;
        }
        thread::sleep(Duration::from_millis(3));
    }
    true
}

/// Toggle `WS_EX_NOACTIVATE` on a window.
///
/// Enabled: clicks never steal focus from the chat app. Disabled: the window
/// can be focused, which editing inside it requires (a non-activating window
/// receives no keyboard input at all).
pub fn set_no_activate(hwnd: isize, enabled: bool) -> bool {
    // SAFETY: the caller passes a handle to our own window.
    unsafe {
        let style = GetWindowLongW(hwnd as _, GWL_EXSTYLE);
        let mut next = style | WS_EX_TOOLWINDOW as i32;
        next = if enabled {
            next | WS_EX_NOACTIVATE as i32
        } else {
            next & !(WS_EX_NOACTIVATE as i32)
        };
        SetWindowLongW(hwnd as _, GWL_EXSTYLE, next);
        // Ex-styles are cached per window; ask the frame to recompute so the
        // change applies without waiting for the next activation.
        SetWindowPos(
            hwnd as _,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
        true
    }
}

/// What the picker reads back from the window under the cursor.
pub struct WindowInfo {
    pub process: String,
    pub title: String,
    pub class: String,
}

fn window_info(hwnd: HWND) -> Option<WindowInfo> {
    // SAFETY: the caller passes a valid window handle.
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let process = process_file_name(pid)?;
        let mut buffer = [0u16; 512];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        let title = if copied > 0 {
            String::from_utf16_lossy(&buffer[..copied as usize])
        } else {
            String::new()
        };
        let mut class_buffer = [0u16; 256];
        let class_copied =
            GetClassNameW(hwnd, class_buffer.as_mut_ptr(), class_buffer.len() as i32);
        let class = if class_copied > 0 {
            String::from_utf16_lossy(&class_buffer[..class_copied as usize])
        } else {
            String::new()
        };
        Some(WindowInfo { process, title, class })
    }
}

/// Executable name, title and class of the window currently under the cursor.
///
/// Used by the "pick an app by clicking it" flow: the click lands on the target
/// window, and this reads back what the user aimed at.
pub fn window_under_cursor() -> Option<WindowInfo> {
    // SAFETY: plain queries on a handle returned by the system.
    unsafe {
        let mut point = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut point) == 0 {
            return None;
        }
        let hit = WindowFromPoint(point);
        if hit.is_null() {
            return None;
        }
        let root = GetAncestor(hit, GA_ROOT);
        let hwnd = if root.is_null() { hit } else { root };
        window_info(hwnd)
    }
}

/// Same data for the foreground window, used to match a configured target.
pub fn foreground_window_info() -> Option<WindowInfo> {
    // SAFETY: plain query.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        window_info(hwnd)
    }
}

/// Press Enter in the focused window (used by "fill and send").
/// Ctrl+Enter, for chat apps that send on that chord.
pub fn press_ctrl_enter() -> bool {
    let inputs = [
        key_input(VK_CONTROL, false),
        key_input(VK_RETURN, false),
        key_input(VK_RETURN, true),
        key_input(VK_CONTROL, true),
    ];
    // SAFETY: array lives for the call; INPUT is plain-old-data.
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        ) == inputs.len() as u32
    }
}

pub fn press_enter() -> bool {
    let inputs = [key_input(VK_RETURN, false), key_input(VK_RETURN, true)];
    // SAFETY: two valid INPUT structures.
    unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) == 2 }
}

/// Bounds of the foreground window, in physical pixels.
pub fn foreground_window_rect() -> Option<Rect> {
    // SAFETY: plain out-parameter call.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return None;
        }
        Some(Rect::new(rect.left, rect.top, rect.right, rect.bottom))
    }
}

/// `true` while any mouse button is held down (dragging must never switch).
pub fn mouse_button_down() -> bool {
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON]
        .into_iter()
        .any(|vk| {
            // SAFETY: plain query, no pointers.
            (unsafe { GetAsyncKeyState(vk as i32) } as u16) & 0x8000 != 0
        })
}

/// `true` when the cursor is over the taskbar (start button, tray, show-desktop).
pub fn cursor_over_taskbar(x: i32, y: i32) -> bool {
    // SAFETY: plain value in, window handle out.
    let hwnd = unsafe { WindowFromPoint(POINT { x, y }) };
    if hwnd.is_null() {
        return false;
    }
    matches!(
        window_class(hwnd).as_str(),
        "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "TrayNotifyWnd"
            | "MSTaskSwWClass"
            | "MSTaskListWClass"
            | "TaskListThumbnailWnd"
    )
}

/// `true` when the secure desktop is active (lock screen, UAC prompt).
///
/// `OpenInputDesktop` fails in that state, which is exactly the signal we want.
pub fn is_locked() -> bool {
    // SAFETY: handle is closed immediately below.
    let desktop = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
    if desktop.is_null() {
        true
    } else {
        // SAFETY: the handle came from OpenInputDesktop.
        unsafe { CloseDesktop(desktop) };
        false
    }
}

/// Drag the window from its custom title bar.
///
/// A window without a system frame does not answer the caption drag, so the
/// title bar moves it from here: follow the cursor while the left button is
/// held, then stop.
pub fn start_manual_drag(hwnd: isize) {
    thread::spawn(move || {
        let hwnd = hwnd as HWND;
        let mut origin = POINT::default();
        let mut rect = RECT::default();
        // SAFETY: plain queries on a live window owned by this process.
        unsafe {
            if GetCursorPos(&mut origin) == 0 || GetWindowRect(hwnd, &mut rect) == 0 {
                return;
            }
        }
        loop {
            // SAFETY: same.
            if unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } >= 0 {
                break;
            }
            let mut now = POINT::default();
            // SAFETY: same.
            unsafe {
                if GetCursorPos(&mut now) == 0 {
                    break;
                }
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    rect.left + (now.x - origin.x),
                    rect.top + (now.y - origin.y),
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            thread::sleep(Duration::from_millis(8));
        }
    });
}
/// Number of virtual desktop switches seen since the app started.
static DESKTOP_SWITCHES: AtomicU64 = AtomicU64::new(0);

pub fn desktop_switch_count() -> u64 {
    DESKTOP_SWITCHES.load(Ordering::Relaxed)
}

unsafe extern "system" fn desktop_switch_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    _hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if event == EVENT_SYSTEM_DESKTOPSWITCH {
        DESKTOP_SWITCHES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Count desktop switches as they happen.
///
/// Polling the current desktop can take a few hundred milliseconds, and during
/// that window Windows has already carried the phrase panel to the new desktop
/// (which shows up as a black rectangle flashing). The shell event fires as the
/// switch starts, so the panel can be hidden in time.
pub fn watch_desktop_switches() {
    thread::spawn(|| {
        // SAFETY: installing an out-of-context hook and pumping its messages.
        unsafe {
            let hook = SetWinEventHook(
                EVENT_SYSTEM_DESKTOPSWITCH,
                EVENT_SYSTEM_DESKTOPSWITCH,
                std::ptr::null_mut(),
                Some(desktop_switch_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if hook.is_null() {
                return;
            }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}
/// Move a window immediately, without activation or z-order changes.
///
/// `WebviewWindow::set_position` goes through the Tauri event loop, which
/// adds a frame or two of latency; a follow loop needs the raw call.
pub fn move_window_now(hwnd: isize, x: i32, y: i32) {
    if hwnd == 0 {
        return;
    }
    // SAFETY: the handle is owned by this process and only repositioned.
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        );
    }
}
/// Move the cursor (used to nudge it back inside after a switch).
pub fn set_cursor_pos(x: i32, y: i32) -> bool {
    // SAFETY: plain values.
    unsafe { SetCursorPos(x, y) != 0 }
}

/// Windows accent color as `#rrggbb`.
///
/// DWM stores it as `0xAABBGGRR`. Achromatic accents (dark gray / black, which
/// many users pick) are rejected: tinting the UI with them looks muddy, so the
/// app keeps its own accent instead.
pub fn accent_color() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\DWM").ok()?;
    let value: u32 = key.get_value("AccentColor").ok()?;
    format_accent(value)
}

fn format_accent(value: u32) -> Option<String> {
    let r = (value & 0xFF) as u8;
    let g = ((value >> 8) & 0xFF) as u8;
    let b = ((value >> 16) & 0xFF) as u8;
    let high = r.max(g).max(b);
    let low = r.min(g).min(b);
    if high - low < 24 {
        return None;
    }
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn window_class(hwnd: HWND) -> String {
    let mut buffer = [0u16; 128];
    // SAFETY: buffer is valid for `len` UTF-16 units.
    let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..len as usize])
}

/// Executable file name (lowercase) of a process.
fn process_file_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    // SAFETY: the handle is closed on every path below.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let path = OsString::from_wide(&buffer[..length as usize]);
        Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
    }
}

/// Executable name of the process owning the foreground window.
pub fn foreground_process_name() -> Option<String> {
    // SAFETY: plain handle/pid queries.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        process_file_name(pid)
    }
}

/// One visible application window, for the "add app to blacklist" picker.
pub struct AppEntry {
    pub name: String,
    pub title: String,
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `&mut Vec<AppEntry>` passed to EnumWindows below.
    let entries = unsafe { &mut *(lparam as *mut Vec<AppEntry>) };

    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return TRUE;
    }
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return TRUE;
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if copied <= 0 {
        return TRUE;
    }
    let title = OsString::from_wide(&buffer[..copied as usize])
        .to_string_lossy()
        .to_string();

    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if let Some(name) = process_file_name(pid) {
        if !entries.iter().any(|entry| entry.name == name) {
            entries.push(AppEntry { name, title });
        }
    }
    TRUE
}

/// Visible top-level windows, one entry per executable, sorted by name.
pub fn window_apps() -> Vec<AppEntry> {
    let mut entries: Vec<AppEntry> = Vec::new();
    // SAFETY: the callback only writes through the pointer we pass in.
    unsafe {
        EnumWindows(Some(collect_window), &mut entries as *mut Vec<AppEntry> as LPARAM);
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

#[cfg(test)]
mod tests {
    use super::format_accent;

    #[test]
    fn accent_is_abgr() {
        // 0xAABBGGRR -> #rrggbb
        assert_eq!(format_accent(0xFFD77800).as_deref(), Some("#0078d7"));
    }

    #[test]
    fn achromatic_accent_is_ignored() {
        assert_eq!(format_accent(0xFF484A4C), None);
        assert_eq!(format_accent(0xFFFFFFFF), None);
    }
}
