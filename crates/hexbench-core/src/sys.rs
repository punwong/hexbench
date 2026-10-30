//! Thin Win32 layer, backed by the official `windows-sys` raw bindings.
//!
//! The core crate deliberately stays close to the metal: it only needs
//! `EnumDisplayMonitors`, `GetMonitorInfoW`, `MonitorFromPoint` and
//! `GetCursorPos`, and `windows-sys` gives us those with no runtime cost and no
//! hand-maintained struct layouts.

use std::ffi::c_void;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, ScreenToClient, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL,
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
use windows_sys::core::GUID;
use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Ole::{DoDragDrop, OleInitialize};
use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
use windows_sys::Win32::UI::Shell::{
    CMINVOKECOMMANDINFO, SHBindToParent, SHCreateDataObject, SHGetIDListFromObject,
    SHParseDisplayName,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, EVENT_SYSTEM_DESKTOPSWITCH, GetAncestor, GetClassNameW,
    GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowLongW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, MSG,
    SetCursorPos, SetWindowLongW, SetWindowPos, TranslateMessage, WindowFromPoint,
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuStringW, InsertMenuW,
    TrackPopupMenuEx, HMENU, MF_BYPOSITION, MF_GRAYED, MF_SEPARATOR, MF_STRING,
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

// ---------------------------------------------------------------------------
// 虚拟桌面：把窗口搬到当前桌面
// ---------------------------------------------------------------------------

/// `IVirtualDesktopManager` —— Windows SDK 里的公开接口，只对**本进程**的窗口
/// 生效。资源管理器「在所有桌面上显示」用的那个未公开接口在 Windows 11 24H2
/// 上已经被移除（CLSID 不再注册），所以这里改用公开的「搬窗口」。
#[repr(C)]
struct DesktopManagerVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    is_window_on_current_desktop: unsafe extern "system" fn(*mut c_void, HWND, *mut i32) -> i32,
    get_window_desktop_id: unsafe extern "system" fn(*mut c_void, HWND, *mut GUID) -> i32,
    move_window_to_desktop: unsafe extern "system" fn(*mut c_void, HWND, *const GUID) -> i32,
}

const CLSID_VIRTUAL_DESKTOP_MANAGER: GUID =
    GUID::from_u128(0xAA50_9086_5CA9_4C25_8F95_589D_3C07_B48A);
const IID_IVIRTUAL_DESKTOP_MANAGER: GUID =
    GUID::from_u128(0xA5CD_92FF_29BE_454C_8D04_D828_79FB_3F1B);

fn desktop_manager() -> Result<*mut c_void, String> {
    // SAFETY: standard COM activation on the calling thread.
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let mut object: *mut c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_VIRTUAL_DESKTOP_MANAGER,
            std::ptr::null_mut(),
            CLSCTX_ALL,
            &IID_IVIRTUAL_DESKTOP_MANAGER,
            &mut object,
        );
        if hr < 0 || object.is_null() {
            return Err(format!("CoCreateInstance 0x{hr:08x}"));
        }
        Ok(object)
    }
}

/// Registry layout (little-endian fields) → GUID.
fn guid_from_desktop_id(bytes: &[u8; 16]) -> GUID {
    GUID {
        data1: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        data2: u16::from_le_bytes([bytes[4], bytes[5]]),
        data3: u16::from_le_bytes([bytes[6], bytes[7]]),
        data4: [
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ],
    }
}

/// Move one of our windows to the desktop identified by the registry id.
pub fn move_window_to_desktop(hwnd: isize, desktop: &[u8; 16]) -> Result<(), String> {
    if hwnd == 0 {
        return Err("no window handle".to_string());
    }
    let target = guid_from_desktop_id(desktop);
    let object = desktop_manager()?;
    // SAFETY: `object` is the interface just activated; only MoveWindowToDesktop
    // is called, and it is documented to accept our own windows.
    unsafe {
        let vtable = *(object as *mut *const DesktopManagerVtbl);
        let hr = ((*vtable).move_window_to_desktop)(object, hwnd as HWND, &target);
        ((*vtable).release)(object);
        if hr < 0 {
            return Err(format!("MoveWindowToDesktop 0x{hr:08x}"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 从窗口里把文件拖出去（OLE 拖拽源）
// ---------------------------------------------------------------------------
// 从窗口里把文件拖出去（OLE 拖拽源 + Shell 数据对象）
// ---------------------------------------------------------------------------

const DRAGDROP_S_DROP: i32 = 0x0004_0100u32 as i32;
const DRAGDROP_S_CANCEL: i32 = 0x0004_0101u32 as i32;
const DRAGDROP_S_USEDEFAULTCURSORS: i32 = 0x0004_0102u32 as i32;

const S_OK: i32 = 0;

const CF_HDROP: u16 = 15;
const DROPEFFECT_COPY_VAL: u32 = 1;
const DROPEFFECT_MOVE_VAL: u32 = 2;

/// 鼠标左键是否按着（`MK_LBUTTON`）。
const MK_LBUTTON: u32 = 0x0001;

// ---------------------------------------------------------------------------
// IDropSource
// ---------------------------------------------------------------------------

#[repr(C)]
struct DropSourceVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    query_continue_drag: unsafe extern "system" fn(*mut c_void, i32, u32) -> i32,
    give_feedback: unsafe extern "system" fn(*mut c_void, u32) -> i32,
}

#[repr(C)]
struct DropSource {
    vtbl: *const DropSourceVtbl,
    refs: u32,
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "system" fn ds_query_interface(
    this: *mut c_void,
    _iid: *const GUID,
    out: *mut *mut c_void,
) -> i32 {
    if !out.is_null() {
        *out = this;
        ds_add_ref(this);
    }
    S_OK
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "system" fn ds_add_ref(this: *mut c_void) -> u32 {
    let source = &mut *(this as *mut DropSource);
    source.refs += 1;
    source.refs
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "system" fn ds_release(this: *mut c_void) -> u32 {
    let source = &mut *(this as *mut DropSource);
    source.refs = source.refs.saturating_sub(1);
    if source.refs == 0 {
        drop(Box::from_raw(this as *mut DropSource));
        return 0;
    }
    source.refs
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "system" fn ds_query_continue_drag(
    _this: *mut c_void,
    escape_pressed: i32,
    key_state: u32,
) -> i32 {
    if escape_pressed != 0 {
        return DRAGDROP_S_CANCEL;
    }
    if key_state & MK_LBUTTON == 0 {
        return DRAGDROP_S_DROP;
    }
    S_OK
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "system" fn ds_give_feedback(_this: *mut c_void, _effect: u32) -> i32 {
    DRAGDROP_S_USEDEFAULTCURSORS
}

static DROP_SOURCE_VTBL: DropSourceVtbl = DropSourceVtbl {
    query_interface: ds_query_interface,
    add_ref: ds_add_ref,
    release: ds_release,
    query_continue_drag: ds_query_continue_drag,
    give_feedback: ds_give_feedback,
};

/// 把一组宽字符路径包成 `DROPFILES`（fWide = 1，双 `\0` 结尾）放进 HGLOBAL。
fn hdrop_for(files: &[u16]) -> *mut c_void {
    // DROPFILES: pFiles(4) + pt(8) + fNC(4) + fWide(4) = 20 字节
    const HEADER: usize = 20;
    let bytes = HEADER + std::mem::size_of_val(files);
    // SAFETY: 标准的内存分配/拷贝，失败时返回空指针由调用方处理。
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if handle.is_null() {
            return std::ptr::null_mut();
        }
        let base = GlobalLock(handle);
        if base.is_null() {
            GlobalFree(handle);
            return std::ptr::null_mut();
        }
        std::ptr::write_bytes(base as *mut u8, 0, bytes);
        (*(base as *mut u32)) = HEADER as u32; // pFiles
        (*(base.add(16) as *mut i32)) = 1; // fWide
        let names = base.add(HEADER) as *mut u16;
        std::ptr::copy_nonoverlapping(files.as_ptr(), names, files.len());
        GlobalUnlock(handle);
        handle
    }
}
/// 把若干文件放进剪贴板（`CF_HDROP`），资源管理器里 `Ctrl+V` 即可粘贴。
pub fn copy_files_to_clipboard(paths: &[std::path::PathBuf]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    if paths.is_empty() {
        return Err("no files".to_string());
    }
    let mut files: Vec<u16> = Vec::new();
    for path in paths {
        files.extend(path.as_os_str().encode_wide());
        files.push(0);
    }
    files.push(0);
    // SAFETY: standard clipboard exchange; every handle is checked.
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("OpenClipboard failed".to_string());
        }
        let _ = EmptyClipboard();
        let handle = hdrop_for(&files);
        if handle.is_null() {
            let _ = CloseClipboard();
            return Err("out of memory".to_string());
        }
        if SetClipboardData(CF_HDROP as u32, handle).is_null() {
            GlobalFree(handle);
            let _ = CloseClipboard();
            return Err("SetClipboardData failed".to_string());
        }
        let _ = CloseClipboard();
    }
    Ok(())
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

/// 前台窗口的句柄：借走焦点的地方（常用语编辑框）退出时还回去。
pub fn foreground_hwnd() -> Option<isize> {
    // SAFETY: plain query on a system handle.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            None
        } else {
            Some(hwnd as isize)
        }
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

/// 把一个窗口顶到前台并给它键盘焦点。
///
/// Windows 有前台锁：进程不在前台时 `SetForegroundWindow` 往往只是闪一下任务栏。
/// 标准绕法是临时把自己挂到当前前台线程的输入队列上（`AttachThreadInput`），
/// 拿到输入所有权后再设置前台，最后解挂。启动器按快捷键必须立刻能打字，靠的就是它。
/// 注意：这里**不碰**显示状态。tao 自己缓存了一份窗口的 VISIBLE 标志，
/// 在外面直接 `ShowWindow` 会让缓存和真实状态对不上，之后 tao 的
/// `show()`/`hide()` 会因为「标志没变」直接空转 —— 用户看到的就是面板
/// 呼不出来（连续呼出呼入几次就必然复现）。显隐只走 `show_window`/`hide_window`。
/// 把窗口抢到前台并给它键盘焦点。
///
/// **必须在窗口自己所属的线程上调用**（对 Tauri 窗口就是主线程）。从别的线程调
/// `SetFocus` 等于跨线程发 WM_KILLFOCUS / WM_SETFOCUS，是同步的：调用方会一直等
/// 主线程处理完。连续呼出呼入时主线程正走在键盘消息里（tao 在里面持有
/// `KEY_EVENT_BUILDERS` 锁），两边一交叉就死锁——用户看到的是面板卡死、
/// 快捷键也没反应。调用方用 `run_on_main_thread` 把它排进主线程队列。
pub fn force_foreground(hwnd: isize) {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    if hwnd == 0 {
        return;
    }
    // SAFETY: 全是标准窗口/线程 API，句柄由调用方保证有效。
    unsafe {
        let target = hwnd as HWND;
        if SetForegroundWindow(target) != 0 {
            // 已经在窗口自己的线程上：SetFocus 是纯线程内的，不会卡。
            let _ = SetFocus(target);
            return;
        }
        // 前台锁拦住了：临时挂接当前前台线程的输入队列再试一次。
        // 只在前台线程不属于自己时才挂，挂完立刻摘掉。
        let foreground = GetForegroundWindow();
        let foreground_thread = if foreground.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, std::ptr::null_mut())
        };
        let this_thread = GetCurrentThreadId();
        let attached = foreground_thread != 0 && foreground_thread != this_thread;
        if attached {
            let _ = AttachThreadInput(foreground_thread, this_thread, 1);
        }
        let _ = SetForegroundWindow(target);
        let _ = SetFocus(target);
        if attached {
            let _ = AttachThreadInput(foreground_thread, this_thread, 0);
        }
    }
}

/// 窗口现在**真的**可见吗。
///
/// 不用 tao 的 `is_visible()`：那是它自己缓存的状态，一旦和真实窗口不一致
/// （外部改过显示状态、或者被别的窗口系统操作过），判断就会一直是错的。
pub fn window_visible(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    if hwnd == 0 {
        return false;
    }
    // SAFETY: 标准窗口查询；句柄无效时返回 0。
    unsafe { IsWindowVisible(hwnd as HWND) != 0 }
}

/// 直接把窗口显示出来 / 藏起来（兜底用：tao 的 show/hide 空转时纠正真实状态，
/// 让真实状态跟 tao 的缓存重新一致）。
pub fn show_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOW};
    if hwnd == 0 {
        return;
    }
    // SAFETY: 标准窗口操作。
    unsafe {
        ShowWindow(hwnd as HWND, SW_SHOW);
    }
}

pub fn hide_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    if hwnd == 0 {
        return;
    }
    // SAFETY: 标准窗口操作。
    unsafe {
        ShowWindow(hwnd as HWND, SW_HIDE);
    }
}

/// 让 Win11 给窗口剪圆角。
///
/// 面板是工具窗口（不占任务栏），系统不会自动给它圆角；之前靠 CSS 自己画，
/// 圆角外面那圈三角露出的是 WebView 的白底——深色主题下四个角就是四个白点，
/// 浅色主题下描边又被系统剪掉一截，看着和设置窗口不一样。
/// 这里直接让系统按和普通窗口一样的方式剪（8px）。
pub fn round_window_corners(hwnd: isize) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    if hwnd == 0 {
        return false;
    }
    let preference = DWMWCP_ROUND;
    // SAFETY: 只读一个 i32；Win10 上这个属性不存在，返回错误码，调用方忽略。
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd as HWND,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&preference as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        )
    };
    result == 0
}

/// 在主线程上初始化 OLE。窗口创建时要注册拖放目标，没有这个初始化的话
/// 外部拖进来的文件一个事件都不会有（看着就像「拖不进来」）。
pub fn init_ole() {
    // SAFETY: 重复初始化返回 S_FALSE，已经初始化过也没关系。
    unsafe {
        let result = OleInitialize(std::ptr::null());
        if result < 0 && result != RPC_E_CHANGED_MODE {
            log::warn!("ole: OleInitialize 0x{result:08x}");
        }
    }
}
/// 当前鼠标位置（屏幕坐标）。
pub fn cursor_position() -> (i32, i32) {
    // SAFETY: GetCursorPos 只写一个 POINT，失败时保留零值。
    unsafe {
        let mut point = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut point) != 0 {
            (point.x, point.y)
        } else {
            (0, 0)
        }
    }
}

/// 屏幕坐标 → 窗口客户区坐标（都是物理像素）。
///
/// 界面里判断「光标压在哪个格子 / 哪一行」用的是页面自己的客户区坐标。
/// 面板每次呼出都会挪到鼠标旁边，而页面里的 `window.screenX/screenY`
/// 在窗口被挪动后并不更新（实测停在第一次摆放的位置），前端拿它做减法
/// 会整体偏掉，拖着文件指哪都不亮。所以坐标在交给界面之前就换算好。
pub fn screen_to_client(hwnd: isize, x: i32, y: i32) -> (i32, i32) {
    // SAFETY: ScreenToClient 只写一个 POINT；失败时原样返回。
    unsafe {
        let mut point = POINT { x, y };
        if ScreenToClient(hwnd as *mut c_void, &mut point) != 0 {
            (point.x, point.y)
        } else {
            (x, y)
        }
    }
}

/// 左键还按着吗？拖拽结束时用来区分「松手放下」和「按 Esc 取消」。
pub fn left_button_down() -> bool {
    // SAFETY: GetAsyncKeyState 只读键盘状态。
    unsafe { GetAsyncKeyState(VK_LBUTTON as i32) < 0 }
}
/// 用系统默认程序打开一个文件或文件夹（和双击效果一致）。
pub fn open_path(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let target: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        let result = ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
        // ShellExecuteW 返回值 > 32 才算成功（老 API 的约定）。
        if result as isize <= 32 {
            return Err(format!("ShellExecuteW {}", result as isize));
        }
    }
    Ok(())
}

const IID_IDATAOBJECT: GUID = GUID::from_u128(0x0000_010E_0000_0000_C000_0000_0000_0046);
/// `IID_IShellFolder` —— 注意是 `14E6`，写成 `14E2` 会直接 `E_NOINTERFACE`。
const IID_ISHELLFOLDER: GUID = GUID::from_u128(0x0002_14E6_0000_0000_C000_0000_0000_0046);
/// 线程已经是别的单元模型时 `OleInitialize` 会返回这个，不算失败。
const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

fn release_object(object: *mut c_void) {
    // SAFETY: every COM object starts with its IUnknown vtable.
    unsafe {
        let vtbl = *(object as *mut *const IUnknownVtbl);
        ((*vtbl).release)(object);
    }
}

/// 让 Shell 生成数据对象：父文件夹 PIDL + 子项 PIDL。
///
/// 这条路踩过三个坑，记在这里免得再走一遍：
/// 1. `SHCreateDataObject(NULL, 完整 PIDL)` 得到的对象什么格式都不给（`GetData`
///    回 `DV_E_FORMATETC`），拖出去必然是 effect 0；
/// 2. `IShellItemArray::BindToHandler(BHID_DataObject)` 在本机回 `E_NOINTERFACE`；
/// 3. `SHBindToParent` 的 `IID_IShellFolder` 是 `000214E6`。
unsafe fn shell_data_object(paths: &[std::path::PathBuf]) -> Result<*mut c_void, String> {
    use std::os::windows::ffi::OsStrExt;
    if paths.is_empty() {
        return Err("nothing to drag".to_string());
    }
    unsafe {
        // 保持每个绝对 pidl 存活：child pidl 是指向它们的。
        let mut pidls: Vec<*mut ITEMIDLIST> = Vec::with_capacity(paths.len());
        let mut children: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
        let mut folder: *mut c_void = std::ptr::null_mut();

        for (index, path) in paths.iter().enumerate() {
            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            let mut attributes: u32 = 0;
            let hr = SHParseDisplayName(
                wide.as_ptr(),
                std::ptr::null_mut(),
                &mut pidl,
                0,
                &mut attributes,
            );
            if hr < 0 || pidl.is_null() {
                for existing in &pidls {
                    CoTaskMemFree(*existing as *const c_void);
                }
                if !folder.is_null() {
                    release_object(folder);
                }
                return Err(format!("SHParseDisplayName 0x{hr:08x}"));
            }

            let mut this_folder: *mut c_void = std::ptr::null_mut();
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            let hr = SHBindToParent(pidl, &IID_ISHELLFOLDER, &mut this_folder, &mut child);
            if hr < 0 || child.is_null() {
                CoTaskMemFree(pidl as *const c_void);
                for existing in &pidls {
                    CoTaskMemFree(*existing as *const c_void);
                }
                if !folder.is_null() {
                    release_object(folder);
                }
                return Err(format!("SHBindToParent 0x{hr:08x}"));
            }
            if index == 0 {
                folder = this_folder;
            } else {
                // 只有第一个父目录会被用到；其余的接口释放掉。
                release_object(this_folder);
            }
            pidls.push(pidl);
            children.push(child as *const ITEMIDLIST);
        }

        let mut parent_pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        let hr = SHGetIDListFromObject(folder, &mut parent_pidl);
        release_object(folder);
        if hr < 0 || parent_pidl.is_null() {
            for existing in &pidls {
                CoTaskMemFree(*existing as *const c_void);
            }
            return Err(format!("SHGetIDListFromObject 0x{hr:08x}"));
        }

        let mut data: *mut c_void = std::ptr::null_mut();
        let hr = SHCreateDataObject(
            parent_pidl,
            children.len() as u32,
            children.as_ptr(),
            std::ptr::null_mut(),
            &IID_IDATAOBJECT,
            &mut data,
        );
        CoTaskMemFree(parent_pidl as *const c_void);
        for existing in &pidls {
            CoTaskMemFree(*existing as *const c_void);
        }
        if hr < 0 || data.is_null() {
            return Err(format!("SHCreateDataObject 0x{hr:08x}"));
        }
        Ok(data)
    }
}

/// 从窗口里把文件拖出去。必须在拥有窗口的线程上调用：OLE 的拖拽循环要靠那个
/// 窗口捕获鼠标。返回值里 `DROPEFFECT_MOVE`（2）表示目标把文件拿走了。
pub fn start_file_drag(paths: &[std::path::PathBuf]) -> Result<u32, String> {
    if paths.is_empty() {
        return Err("nothing to drag".to_string());
    }
    // SAFETY: 数据对象与拖拽源都有各自的引用计数，DoDragDrop 期间不会失效。
    unsafe {
        let init = OleInitialize(std::ptr::null());
        if init < 0 && init != RPC_E_CHANGED_MODE {
            return Err(format!("OleInitialize 0x{init:08x}"));
        }
        // 任意组合（可以跨目录）都做成一个数据对象；不行再退回「同一父目录那一组」。
        let data = match crate::shell_ops::data_object_for(paths) {
            Some(raw) => raw,
            None => {
                let parent = paths[0].parent().map(|p| p.to_path_buf());
                let same_parent: Vec<std::path::PathBuf> = match &parent {
                    Some(parent) => paths
                        .iter()
                        .filter(|path| path.parent() == Some(parent.as_path()))
                        .cloned()
                        .collect(),
                    None => vec![paths[0].clone()],
                };
                log::warn!(
                    "drag: shell item array failed, falling back to {} of {} item(s)",
                    same_parent.len(),
                    paths.len()
                );
                shell_data_object(&same_parent)?
            }
        };
        let source = Box::into_raw(Box::new(DropSource {
            vtbl: &DROP_SOURCE_VTBL,
            refs: 1,
        }));
        let mut effect: u32 = 0;
        let hr = DoDragDrop(
            data,
            source as *mut c_void,
            DROPEFFECT_COPY_VAL | DROPEFFECT_MOVE_VAL,
            &mut effect,
        );
        ds_release(source as *mut c_void);
        release_object(data);
        if hr < 0 {
            return Err(format!("DoDragDrop 0x{hr:08x}"));
        }
        Ok(effect)
    }
}

// ---------------------------------------------------------------------------
// 原生右键菜单：把 Shell 自己那套菜单弹出来
// ---------------------------------------------------------------------------

const CMF_NORMAL: u32 = 0x0000_0000;
const TPM_RETURNCMD: u32 = 0x0100;
const TPM_RIGHTBUTTON: u32 = 0x0002;
const SW_SHOWNORMAL: i32 = 1;

/// `IShellFolder` 只用前 11 个槽，`GetUIObjectOf` 是第 11 个（下标 10）。
#[repr(C)]
struct ShellFolderVtbl {
    query_interface: usize,
    add_ref: usize,
    release: usize,
    parse_display_name: usize,
    enum_objects: usize,
    bind_to_object: usize,
    bind_to_storage: usize,
    compare_ids: usize,
    create_view_object: usize,
    get_attributes_of: usize,
    // 参数顺序跟声明一致：hwnd, cidl, apidl, riid, rgfReserved, ppv
    get_ui_object_of: unsafe extern "system" fn(
        *mut c_void,
        HWND,
        u32,
        *mut *const ITEMIDLIST,
        *const GUID,
        *mut u32,
        *mut *mut c_void,
    ) -> i32,
}

/// `IContextMenu`
#[repr(C)]
struct ContextMenuVtbl {
    query_interface: usize,
    add_ref: usize,
    release: usize,
    query_context_menu: unsafe extern "system" fn(*mut c_void, HMENU, u32, u32, u32, u32) -> i32,
    invoke_command: unsafe extern "system" fn(*mut c_void, *const CMINVOKECOMMANDINFO) -> i32,
    get_command_string: usize,
}

const IID_ICONTEXTMENU: GUID = GUID::from_u128(0x0002_14E4_0000_0000_C000_0000_0000_0046);

/// 在某个位置弹出文件的「资源管理器右键菜单」。
///
/// 菜单本身由 Shell 绘制，所以外观、语言、已安装的扩展（7-Zip、Git 之类）都和
/// 资源管理器一致。重命名 / 删除不是文件菜单里的项（那是视图层的命令），所以
/// 这里在系统菜单后面补两项自己实现的，返回给调用方去处理。
///
/// `owner` 用作菜单的宿主窗口；传 0 时用当前前台窗口。
pub fn show_shell_context_menu(
    path: &std::path::Path,
    x: i32,
    y: i32,
    owner: isize,
    before_invoke: impl FnOnce(),
) -> Result<Option<&'static str>, String> {
    /// 自己加的菜单项用高位 id，和 Shell 的 1..0x7FFF 区分开。
    const CMD_RENAME: u32 = 0x8000;

    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut choice = None;
    // SAFETY: PIDL / IShellFolder / IContextMenu 都在本函数内释放；菜单句柄同理。
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        let mut attributes: u32 = 0;
        let hr = SHParseDisplayName(
            wide.as_ptr(),
            std::ptr::null_mut(),
            &mut pidl,
            0,
            &mut attributes,
        );
        if hr < 0 || pidl.is_null() {
            return Err(format!("SHParseDisplayName 0x{hr:08x}"));
        }

        let mut folder: *mut c_void = std::ptr::null_mut();
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let hr = SHBindToParent(pidl, &IID_ISHELLFOLDER, &mut folder, &mut child);
        if hr < 0 {
            CoTaskMemFree(pidl as *const c_void);
            return Err(format!("SHBindToParent 0x{hr:08x}"));
        }

        let folder_vtbl = *(folder as *mut *const ShellFolderVtbl);
        let mut menu_object: *mut c_void = std::ptr::null_mut();
        let mut reserved: u32 = 0;
        let children: [*const ITEMIDLIST; 1] = [child as *const ITEMIDLIST];
        let hr = ((*folder_vtbl).get_ui_object_of)(
            folder,
            std::ptr::null_mut(),
            1,
            children.as_ptr() as *mut *const ITEMIDLIST,
            &IID_ICONTEXTMENU,
            &mut reserved,
            &mut menu_object,
        );
        release_object(folder);
        CoTaskMemFree(pidl as *const c_void);
        if hr < 0 || menu_object.is_null() {
            return Err(format!("GetUIObjectOf 0x{hr:08x}"));
        }

        let menu_vtbl = *(menu_object as *mut *const ContextMenuVtbl);
        let popup = CreatePopupMenu();
        if popup.is_null() {
            release_object(menu_object);
            return Err("CreatePopupMenu failed".to_string());
        }
        let hr = ((*menu_vtbl).query_context_menu)(menu_object, popup, 0, 1, 0x7FFF, CMF_NORMAL);
        if hr < 0 {
            DestroyMenu(popup);
            release_object(menu_object);
            return Err(format!("QueryContextMenu 0x{hr:08x}"));
        }

        // 「重命名」在资源管理器里由视图层提供；这里插到「删除」上方。
        // 先找「删除」，找不到再找「属性」，都没有就追加到末尾——至少功能还在。
        let anchor = {
            let mut found = None;
            'outer: for needle in ["删除", "属性"] {
                let count = GetMenuItemCount(popup);
                for index in 0..count {
                    let mut text = [0u16; 64];
                    let length = GetMenuStringW(
                        popup,
                        index as u32,
                        text.as_mut_ptr(),
                        text.len() as i32,
                        MF_BYPOSITION,
                    );
                    if length <= 0 {
                        continue;
                    }
                    let label = String::from_utf16_lossy(&text[..length as usize]);
                    if label.starts_with(needle) {
                        found = Some(index as u32);
                        break 'outer;
                    }
                }
            }
            found
        };
        match anchor {
            Some(index) => {
                InsertMenuW(
                    popup,
                    index,
                    MF_BYPOSITION | MF_STRING,
                    CMD_RENAME as usize,
                    windows_sys::core::w!("重命名(&M)"),
                );
            }
            None => {
                AppendMenuW(popup, MF_SEPARATOR, 0, std::ptr::null());
                AppendMenuW(
                    popup,
                    MF_STRING,
                    CMD_RENAME as usize,
                    windows_sys::core::w!("重命名(&M)"),
                );
            }
        }

        let owner_window = if owner != 0 {
            owner as HWND
        } else {
            GetForegroundWindow()
        };
        let id = TrackPopupMenuEx(
            popup,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            x,
            y,
            owner_window,
            std::ptr::null(),
        );
        DestroyMenu(popup);
        log::info!("context menu: picked id={id}");

        match id as u32 {
            0 => {}
            CMD_RENAME => choice = Some("rename"),
            other => {
                // InvokeCommand 可能同步跑上一会儿（等自己的进度窗口出来）。
                // 调用方要在这之前就知道「命令开始了」，否则这段时间收到的
                // 失焦会先把面板收掉——压缩过程中面板就没了。
                before_invoke();
                // QueryContextMenu 从 1 开始编号，调用时要用相对偏移。
                let command = CMINVOKECOMMANDINFO {
                    cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                    fMask: 0,
                    hwnd: owner_window,
                    lpVerb: (other - 1) as usize as *const u8,
                    lpParameters: std::ptr::null(),
                    lpDirectory: std::ptr::null(),
                    nShow: SW_SHOWNORMAL,
                    dwHotKey: 0,
                    hIcon: std::ptr::null_mut(),
                };
                let hr = ((*menu_vtbl).invoke_command)(menu_object, &command);
                if hr < 0 {
                    release_object(menu_object);
                    return Err(format!("InvokeCommand 0x{hr:08x}"));
                }
                log::info!("context menu: invoked shell command {other}");
                // 这些命令大多是异步的：InvokeCommand 返回了，真正的活儿
                // （压缩、复制、发送到…）还在 Shell 里跑，过程中会弹自己的
                // 进度窗口抢走前台。调用方据此别把面板收起。
                choice = Some("shell");
            }
        }
        release_object(menu_object);
    }
    Ok(choice)
}

/// 把文件或文件夹删到回收站（可撤销）。
pub fn delete_to_recycle_bin(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, SHFILEOPSTRUCTW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT, FO_DELETE,
    };
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    // SHFileOperation 要的是「双 NUL 结尾」的路径列表。
    from.push(0);
    from.push(0);
    // SAFETY: 结构体按文档初始化，字符串在调用期间有效。
    unsafe {
        let mut operation: SHFILEOPSTRUCTW = std::mem::zeroed();
        operation.wFunc = FO_DELETE;
        operation.pFrom = from.as_ptr();
        operation.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT) as u16;
        let result = SHFileOperationW(&mut operation);
        if result != 0 {
            return Err(format!("SHFileOperationW 0x{result:04x}"));
        }
    }
    log::info!("context menu: deleted {} to the recycle bin", path.display());
    Ok(())
}

/// 在 `parent` 下新建一个文件夹，重名时自动加序号（新建文件夹、新建文件夹 (2)…）。
pub fn create_folder(parent: &std::path::Path, base: &str) -> Result<std::path::PathBuf, String> {
    if !parent.is_dir() {
        return Err("这个位置不是一个文件夹".to_string());
    }
    for index in 1..1000 {
        let name = if index == 1 {
            base.to_string()
        } else {
            format!("{base} ({index})")
        };
        let target = parent.join(&name);
        if target.exists() {
            continue;
        }
        std::fs::create_dir(&target).map_err(|error| error.to_string())?;
        log::info!("created folder {}", target.display());
        return Ok(target);
    }
    Err("试了很多个名字都被占用了".to_string())
}
/// Windows 的文件名规则。
///
/// 之前只挡了非法字符，漏掉的几类会让系统「悄悄改名」或者直接失败：
/// `CON`/`NUL` 这类设备名、结尾的点和空格（Windows 会吃掉）、超长名字。
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("名字不能为空".to_string());
    }
    if name
        .chars()
        .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
    {
        return Err("名字里有不允许的字符".to_string());
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Err("名字不能以空格或点结尾".to_string());
    }
    // 设备名不分大小写，带扩展名也算（`CON.txt` 同样保留）。
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let upper = stem.to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&upper.as_str()) {
        return Err(format!("「{stem}」是系统保留名，换一个"));
    }
    if name.encode_utf16().count() > 255 {
        return Err("名字太长（超过 255 个字符）".to_string());
    }
    Ok(())
}
/// 在原目录里把文件改名。重名会被拒绝，不会覆盖。
pub fn rename_path(
    path: &std::path::Path,
    new_name: &str,
) -> Result<std::path::PathBuf, String> {
    let name = new_name.trim();
    validate_name(name)?;
    let parent = path
        .parent()
        .ok_or_else(|| "找不到所在文件夹".to_string())?;
    let target = parent.join(name);
    if target == path {
        return Ok(target);
    }
    // Windows 不区分大小写：只改大小写时 `target.exists()` 也为 true，
    // 但那其实是同一个文件。资源管理器允许这样改名，这里也允许。
    if target.exists() && !target.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy()) {
        return Err("已经有同名文件".to_string());
    }
    crate::shell_ops::rename(path, name)?;
    log::info!("renamed {} -> {}", path.display(), target.display());
    Ok(target)
}



/// 前台窗口是不是属于我们自己的进程。
///
/// 面板收起靠「失焦」判断，但内嵌网页是同一进程的子窗口：
/// 它抢到焦点时前台窗口仍然是我们的，这种情况不应该收起。
pub fn foreground_is_current_process() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return false;
        }
        let mut pid = 0u32;
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, &mut pid);
        pid == std::process::id()
    }
}


/// 在文件夹的**空白处**弹出资源管理器原生右键菜单（新建 / 粘贴 / 属性 / 刷新 …）。
///
/// 和文件项菜单的区别只在「怎么拿 IContextMenu」：
/// * 文件项：`IShellFolder::GetUIObjectOf(child)`
/// * 空白处：先 `BindToObject` 拿到该文件夹自己的 `IShellFolder`，
///   再 `CreateViewObject(NULL, IID_IContextMenu)`——这就是「文件夹背景菜单」。
pub fn show_folder_background_menu(
    dir: &std::path::Path,
    x: i32,
    y: i32,
    owner: isize,
    can_paste: bool,
) -> Result<Option<&'static str>, String> {
    /// 自己加的「粘贴」用高位 id，避开 Shell 的 1..0x7FFF。
    const CMD_PASTE: u32 = 0x8001;
    let choice: Option<&'static str> = None;
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: PIDL / IShellFolder / IContextMenu 都在本函数内释放；菜单句柄同理。
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        let mut attributes: u32 = 0;
        let hr = SHParseDisplayName(
            wide.as_ptr(),
            std::ptr::null_mut(),
            &mut pidl,
            0,
            &mut attributes,
        );
        if hr < 0 || pidl.is_null() {
            return Err(format!("SHParseDisplayName 0x{hr:08x}"));
        }

        let mut parent: *mut c_void = std::ptr::null_mut();
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let hr = SHBindToParent(pidl, &IID_ISHELLFOLDER, &mut parent, &mut child);
        if hr < 0 || parent.is_null() {
            CoTaskMemFree(pidl as *const c_void);
            return Err(format!("SHBindToParent 0x{hr:08x}"));
        }

        // 1) 拿到这个目录自己的 IShellFolder
        let parent_vtbl = *(parent as *mut *const ShellFolderVtbl);
        // 注意参数个数：BindToObject(pidl, pbc, riid, ppv) —— 多一个 IBindCtx，
        // 少写一个会让调用栈错位，直接崩进程（之前就是这么崩的）。
        let bind_to_object: unsafe extern "system" fn(
            *mut c_void,
            *const ITEMIDLIST,
            *mut c_void,
            *const GUID,
            *mut *mut c_void,
        ) -> i32 = std::mem::transmute((*parent_vtbl).bind_to_object);
        let mut dir_folder: *mut c_void = std::ptr::null_mut();
        let hr = bind_to_object(
            parent,
            child as *const ITEMIDLIST,
            std::ptr::null_mut(),
            &IID_ISHELLFOLDER,
            &mut dir_folder,
        );
        release_object(parent);
        CoTaskMemFree(pidl as *const c_void);
        if hr < 0 || dir_folder.is_null() {
            return Err(format!("BindToObject 0x{hr:08x}"));
        }

        // 2) 背景菜单 = CreateViewObject(NULL, IID_IContextMenu)
        let dir_vtbl = *(dir_folder as *mut *const ShellFolderVtbl);
        let create_view_object: unsafe extern "system" fn(
            *mut c_void,
            HWND,
            *const GUID,
            *mut *mut c_void,
        ) -> i32 = std::mem::transmute((*dir_vtbl).create_view_object);
        let mut menu_object: *mut c_void = std::ptr::null_mut();
        let hr = create_view_object(
            dir_folder,
            std::ptr::null_mut(),
            &IID_ICONTEXTMENU,
            &mut menu_object,
        );
        release_object(dir_folder);
        if hr < 0 || menu_object.is_null() {
            return Err(format!("CreateViewObject 0x{hr:08x}"));
        }

        // 3) 弹菜单（和文件项菜单同一套流程）
        let menu_vtbl = *(menu_object as *mut *const ContextMenuVtbl);
        let popup = CreatePopupMenu();
        if popup.is_null() {
            release_object(menu_object);
            return Err("CreatePopupMenu failed".to_string());
        }
        let hr = ((*menu_vtbl).query_context_menu)(menu_object, popup, 0, 1, 0x7FFF, CMF_NORMAL);
        if hr < 0 {
            DestroyMenu(popup);
            release_object(menu_object);
            return Err(format!("QueryContextMenu 0x{hr:08x}"));
        }

        // 文件夹自身的 IContextMenu 只有扩展项（Git / VS / 新建…）；
        // 标准项（粘贴 / 查看 / 刷新）在 Shell 视图对象里，这里自己补一个「粘贴」。
        InsertMenuW(
            popup,
            0,
            MF_BYPOSITION | MF_STRING | if can_paste { 0 } else { MF_GRAYED },
            CMD_PASTE as usize,
            windows_sys::core::w!("粘贴(&P)"),
        );
        InsertMenuW(popup, 1, MF_BYPOSITION | MF_SEPARATOR, 0, std::ptr::null());

        let owner_window = if owner != 0 {
            owner as HWND
        } else {
            GetForegroundWindow()
        };
        let id = TrackPopupMenuEx(
            popup,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            x,
            y,
            owner_window,
            std::ptr::null(),
        );
        DestroyMenu(popup);
        log::info!("background menu: picked id={id}");

        if id as u32 == CMD_PASTE {
            release_object(menu_object);
            return Ok(Some("paste"));
        }

        if id != 0 {
            // QueryContextMenu 从 1 开始编号，InvokeCommand 用相对偏移。
            let command = CMINVOKECOMMANDINFO {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                fMask: 0,
                hwnd: owner_window,
                lpVerb: (id as u32 - 1) as usize as *const u8,
                lpParameters: std::ptr::null(),
                lpDirectory: std::ptr::null(),
                nShow: SW_SHOWNORMAL,
                dwHotKey: 0,
                hIcon: std::ptr::null_mut(),
            };
            let _ = ((*menu_vtbl).invoke_command)(menu_object, &command);
        }
        release_object(menu_object);
        Ok(choice)
    }
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
