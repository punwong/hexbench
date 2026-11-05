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
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VK_CONTROL, VK_D, VK_LBUTTON, VK_LEFT, VK_LWIN, VK_MBUTTON, VK_MENU,
    VK_RBUTTON, VK_RETURN, VK_RIGHT, VK_TAB,
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
    SHGetKnownFolderPath, SHParseDisplayName,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetClassNameW,
    GetCursorPos, GetForegroundWindow, GetWindowLongW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    SetCursorPos, SetWindowLongW, SetWindowPos, WindowFromPoint,
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuStringW, InsertMenuW,
    TrackPopupMenuEx, HMENU, MF_BYPOSITION, MF_GRAYED, MF_SEPARATOR, MF_STRING,
    GA_ROOT, GWL_EXSTYLE, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

use crate::geometry::EdgeProbe;
use crate::model::{Monitor, Rect};
use crate::trigger::Direction;

/// `MONITORINFOF_PRIMARY` is not re-exported by `windows-sys` 0.61.
const MONITORINFOF_PRIMARY: u32 = 0x1;
/// `SWP_NOOWNERZORDER` is not re-exported by `windows-sys` 0.61 either.
const SWP_NOOWNERZORDER: u32 = 0x0200;


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
/// The clipboard is never touched, so sending a note cannot destroy whatever
/// the user had copied. Classic edit controls and Chromium inputs both accept
/// the resulting `WM_CHAR`.
pub fn type_text(text: &str) -> bool {
    let mut buf = [0u16; 2];
    for ch in text.chars() {
        // 换行和制表符不能当普通字符发：U+000A / U+0009 走 Unicode 输入时多数
        // 控件直接丢掉（记事本会把整段笔记挤成一行），得真的敲一下回车 / Tab。
        let ok = match ch {
            '\n' => send_keys(VK_RETURN),
            '\t' => send_keys(VK_TAB),
            _ => ch.encode_utf16(&mut buf).iter().all(|unit| {
                let inputs = [unicode_input(*unit, false), unicode_input(*unit, true)];
                // SAFETY: two valid INPUT structures.
                unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) == 2 }
            }),
        };
        if !ok {
            return false;
        }
        thread::sleep(Duration::from_millis(3));
    }
    true
}

/// 按一下某个虚拟键（不进输入法，也不算字符）。
fn send_keys(vk: u16) -> bool {
    let inputs = [key_input(vk, false), key_input(vk, true)];
    // SAFETY: two valid INPUT structures.
    unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) == 2 }
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

/// 窗口句柄现在还有效吗：收藏的「发送」不能在目标窗口已经关掉之后乱输入。
pub fn window_valid(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
    if hwnd == 0 {
        return false;
    }
    // SAFETY: 标准窗口查询；无效句柄返回 0。
    unsafe { IsWindow(hwnd as HWND) != 0 }
}

/// 这个窗口属于本进程吗（呼出面板、内置网页都是自己的窗口，不能当发送目标）。
pub fn hwnd_is_current_process(hwnd: isize) -> bool {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    if hwnd == 0 {
        return false;
    }
    let mut process = 0u32;
    // SAFETY: 标准查询；pid 指针有效。
    unsafe {
        GetWindowThreadProcessId(hwnd as HWND, &mut process);
        process == GetCurrentProcessId()
    }
}

/// 前台窗口的句柄：呼出面板撑走焦点前记下来，发送收藏时再还回去。
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


pub fn press_enter() -> bool {
    send_keys(VK_RETURN)
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

/// 把键盘焦点还给窗口里的 WebView2 子窗口。
///
/// 回收站 / 撤销这类系统操作会把焦点从 WebView 收回到顶层的「Tauri Window」：
/// 之后窗口虽然还在前台，按键却不再进页面（看起来就是紧接着按 Ctrl+Z 没反应）。
/// 只对顶层 `SetFocus` 修不好，得直接给 WebView2 的 `Chrome_WidgetWin_1` 焦点。
///
/// 和 `force_foreground` 一样**必须在窗口自己的线程上调用**。
pub fn focus_webview(parent: isize) {
    use std::sync::atomic::{AtomicIsize, Ordering};
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, GetAncestor, GetClassNameW, GetWindowThreadProcessId, IsWindow,
        IsWindowVisible, GA_ROOT,
    };
    // 内置网页也是同一个窗口类：记住上次真正用到的那个，别把焦点交给网页那一半。
    static REMEMBERED: AtomicIsize = AtomicIsize::new(0);
    if parent == 0 {
        return;
    }
    // SAFETY: lparam 是调用方传进来的 &mut HWND，活到枚举结束。
    extern "system" fn find(hwnd: HWND, lparam: isize) -> i32 {
        let found = unsafe { &mut *(lparam as *mut HWND) };
        let mut buffer = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        if len <= 0 {
            return 1;
        }
        let name = String::from_utf16_lossy(&buffer[..len as usize]);
        if name == "Chrome_WidgetWin_1" && unsafe { IsWindowVisible(hwnd) } != 0 {
            *found = hwnd;
            return 0;
        }
        1
    }
    // SAFETY: 标准窗口/线程 API，parent 由调用方保证是活着的窗口句柄。
    unsafe {
        let top = parent as HWND;
        let remembered = REMEMBERED.load(Ordering::Relaxed) as HWND;
        let mut found: HWND = std::ptr::null_mut();
        if !remembered.is_null()
            && IsWindow(remembered) != 0
            && GetAncestor(remembered, GA_ROOT) == top
        {
            found = remembered;
        } else {
            EnumChildWindows(top, Some(find), &mut found as *mut HWND as isize);
        }
        if found.is_null() {
            return;
        }
        REMEMBERED.store(found as isize, Ordering::Relaxed);
        let target_thread = GetWindowThreadProcessId(found, std::ptr::null_mut());
        let this_thread = GetCurrentThreadId();
        let attached = target_thread != 0
            && target_thread != this_thread
            && AttachThreadInput(target_thread, this_thread, 1) != 0;
        let _ = SetFocus(found);
        if attached {
            let _ = AttachThreadInput(target_thread, this_thread, 0);
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

/// 显示窗口但**不抢焦点**：便签从面板里钉出来时，用户还在面板上，键盘焦点不该跳走。
pub fn show_window_no_activate(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE};
    if hwnd == 0 {
        return;
    }
    // SAFETY: 标准窗口操作。
    unsafe {
        ShowWindow(hwnd as HWND, SW_SHOWNOACTIVATE);
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

/// 让窗口背后是系统的磨砂材质（亚克力 / 模糊背景）。
///
/// 两条路，按系统版本走：
/// - Win11 22H2+：客户区整块算进边框（`DwmExtendFrameIntoClientArea(-1)`），
///   再设 `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW`（亚克力）。
/// - Win10 1809+：user32 的未公开接口 `SetWindowCompositionAttribute`，
///   `ACCENT_ENABLE_ACRYLICBLURBEHIND`。
///
/// **窗口本身保持不透明**：材质由 DWM 画在客户区后面，页面那边要把 WebView 的
/// 默认背景设成透明（`Webview::set_background_color(None→alpha 0)`）并让卡片色
/// 留出透明度，否则 WebView 自己的白底会把材质盖住。
///
/// 返回 true 表示材质真的上了；调用方据此决定页面用半透明还是纯色（Win10 上
/// 两条路都可能失败，那时退回原来的不透明卡片）。
pub fn acrylic(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    if accent_acrylic(hwnd) {
        return true;
    }
    dwm_acrylic(hwnd)
}

/// Win11 22H2+ 的系统背景材质。
fn dwm_acrylic(hwnd: isize) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_TRANSIENTWINDOW,
        DWMWA_SYSTEMBACKDROP_TYPE,
    };
    use windows_sys::Win32::UI::Controls::MARGINS;
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    let backdrop = DWMSBT_TRANSIENTWINDOW;
    // SAFETY: 两个调用都只读这几字节；老系统没有这个属性，返回错误码而已。
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd as HWND, &margins);
        DwmSetWindowAttribute(
            hwnd as HWND,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&backdrop as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        ) == 0
    }
}

/// 把背后的材质撤掉（设置里关掉磨砂时用）。
///
/// 上材质的两条路顺序试，撤的时候也不知道当初是哪条生效的，所以两条都撤：
/// DWM 那边的背景材质设回无、客户区边框收回去，重点把 Accent 关掉。
pub fn clear_acrylic(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    accent_policy(hwnd, 0); // ACCENT_DISABLED
    use windows_sys::Win32::Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE,
    };
    use windows_sys::Win32::UI::Controls::MARGINS;
    let margins = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 0, cyBottomHeight: 0 };
    let none: i32 = 0; // DWMSBT_NONE
    // SAFETY: 都是只读这几字节的属性设置，失败返回错误码而已。
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd as HWND, &margins);
        let _ = DwmSetWindowAttribute(
            hwnd as HWND,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&none as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// Win10 1809+ 的亚克力：user32 里没进头文件的接口，按地址取。
fn accent_acrylic(hwnd: isize) -> bool {
    accent_policy(hwnd, 4) // ACCENT_ENABLE_ACRYLICBLURBEHIND
}

/// 摆一份 accent 策略上去。`state` = 0 就是关掉。
fn accent_policy(hwnd: isize, state: i32) -> bool {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

    #[repr(C)]
    struct AccentPolicy {
        state: i32,
        flags: i32,
        /// AABBGGRR。这里给一层很淡的白，真正的颜色由页面里的半透明色决定。
        gradient_color: u32,
        animation_id: i32,
    }
    #[repr(C)]
    struct WindowCompositionAttributeData {
        attribute: i32,
        data: *mut core::ffi::c_void,
        size: usize,
    }
    const WCA_ACCENT_POLICY: i32 = 19;

    // SAFETY: GetProcAddress 拿到的就是 SetWindowCompositionAttribute 的地址；
    // 结构和签名按社区通行的定义摆（这个接口没公开文档）。
    unsafe {
        let module = GetModuleHandleA(c"user32.dll".as_ptr().cast());
        if module.is_null() {
            return false;
        }
        let Some(entry) = GetProcAddress(module, c"SetWindowCompositionAttribute".as_ptr().cast())
        else {
            return false;
        };
        let set: unsafe extern "system" fn(isize, *mut WindowCompositionAttributeData) -> i32 =
            std::mem::transmute(entry);
        let mut policy = AccentPolicy {
            state,
            flags: 0,
            gradient_color: 0x40_FF_FF_FF,
            animation_id: 0,
        };
        let mut data = WindowCompositionAttributeData {
            attribute: WCA_ACCENT_POLICY,
            data: (&mut policy as *mut AccentPolicy).cast(),
            size: std::mem::size_of::<AccentPolicy>(),
        };
        set(hwnd, &mut data) != 0
    }
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

/// Ctrl 按着吗？拖放松手时用来区分「复制」和「移动」。
pub fn ctrl_down() -> bool {
    // SAFETY: GetAsyncKeyState 只读键盘状态。
    unsafe { GetAsyncKeyState(VK_CONTROL as i32) < 0 }
}

/// Alt 按着吗？按住 Alt 时暂停格子移动 / 缩放的吸附（和 PecoFence 一致）。
pub fn alt_down() -> bool {
    // SAFETY: GetAsyncKeyState 只读键盘状态。
    unsafe { GetAsyncKeyState(VK_MENU as i32) < 0 }
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

/// 用系统默认浏览器打开一个网页链接（便签里的 Markdown 链接）。
pub fn open_url(url: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:"))
    {
        return Err("只支持 http / https / mailto 链接".to_string());
    }
    let target: Vec<u16> = trimmed.encode_utf16().chain(std::iter::once(0)).collect();
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
    show_shell_context_menu_for(&[path.to_path_buf()], x, y, owner, before_invoke)
}

/// 多选版：同一目录下的若干文件 / 文件夹共用一个系统菜单（剪切、复制、删除
/// 一次作用到整组）。不在同一目录的路径会被跳过——格子里的条目本来同属一层。
pub fn show_shell_context_menu_for(
    paths: &[std::path::PathBuf],
    x: i32,
    y: i32,
    owner: isize,
    before_invoke: impl FnOnce(),
) -> Result<Option<&'static str>, String> {
    /// 自己加的菜单项用高位 id，和 Shell 的 1..0x7FFF 区分开。
    const CMD_RENAME: u32 = 0x8000;

    use std::os::windows::ffi::OsStrExt;
    let Some(first) = paths.first() else {
        return Ok(None);
    };
    // 只有同一个目录里的条目才能塞进一份菜单。
    let parent = first
        .parent()
        .map(|dir| dir.to_string_lossy().to_lowercase());
    let group: Vec<&std::path::PathBuf> = paths
        .iter()
        .filter(|path| path.parent().map(|dir| dir.to_string_lossy().to_lowercase()) == parent)
        .collect();
    let mut choice = None;
    // SAFETY: PIDL / IShellFolder / IContextMenu 都在本函数内释放；菜单句柄同理。
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let mut folder: *mut c_void = std::ptr::null_mut();
        // 绝对 pidl 要自己释放；`children` 指向它们内部，所以得等菜单对象
        // 建好之后才能一起放掉。
        let mut parsed: Vec<*mut ITEMIDLIST> = Vec::with_capacity(group.len());
        let mut children: Vec<*const ITEMIDLIST> = Vec::with_capacity(group.len());
        for path in &group {
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
                log::warn!(
                    "context menu: 解析路径失败 0x{hr:08x}: {}",
                    path.display()
                );
                continue;
            }
            parsed.push(pidl);
            let mut parent_folder: *mut c_void = std::ptr::null_mut();
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            let hr = SHBindToParent(pidl, &IID_ISHELLFOLDER, &mut parent_folder, &mut child);
            if hr < 0 {
                log::warn!("context menu: SHBindToParent 0x{hr:08x}");
                continue;
            }
            if folder.is_null() {
                folder = parent_folder;
            } else {
                release_object(parent_folder);
            }
            children.push(child as *const ITEMIDLIST);
        }
        if folder.is_null() || children.is_empty() {
            for pidl in &parsed {
                CoTaskMemFree(*pidl as *const c_void);
            }
            return Err("这些条目打不开系统菜单".to_string());
        }

        let folder_vtbl = *(folder as *mut *const ShellFolderVtbl);
        let mut menu_object: *mut c_void = std::ptr::null_mut();
        let mut reserved: u32 = 0;
        let hr = ((*folder_vtbl).get_ui_object_of)(
            folder,
            std::ptr::null_mut(),
            children.len() as u32,
            children.as_ptr() as *mut *const ITEMIDLIST,
            &IID_ICONTEXTMENU,
            &mut reserved,
            &mut menu_object,
        );
        release_object(folder);
        for pidl in &parsed {
            CoTaskMemFree(*pidl as *const c_void);
        }
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
        // 批量选择时没有「重命名」：一次只能改一个名字。
        if children.len() == 1 {
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
// ---------------------------------------------------------------------------
// 桌面层：桌面格子的锚定与几何
// ---------------------------------------------------------------------------

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: 标准窗口查询；句柄无效时返回 0。
    let copied = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if copied <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..copied as usize])
}

unsafe extern "system" fn host_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` 是调用方传进来的 `&mut (defview, out)`，活到枚举结束。
    unsafe {
        let (defview, out) = &mut *(lparam as *mut (*const u16, HWND));
        if class_name(hwnd) == "WorkerW"
            && !windows_sys::Win32::UI::WindowsAndMessaging::FindWindowExW(
                hwnd,
                std::ptr::null_mut(),
                *defview,
                std::ptr::null(),
            )
            .is_null()
        {
            *out = hwnd;
            return 0;
        }
    }
    1
}

/// 桌面图标宿主：`SHELLDLL_DefView` 所在的那个顶层窗口。
///
/// 24H2 之后 DefView 一直挂在 `Progman` 下；更早的版本它会被挪到某个顶层
/// `WorkerW` 下。这里每次现查、不缓存——Explorer 重启 / 切换桌面时层级会变，
/// 缓存一次就会把格子锚到已经不存在的窗口上。
pub fn shell_icon_host() -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, FindWindowW, GetShellWindow,
    };
    let defview = wide("SHELLDLL_DefView");
    // SAFETY: 全是标准窗口查询。
    unsafe {
        // EnumWindows 只枚举顶层窗口；WorkerW 就是顶层宿主。
        let mut pair: (*const u16, HWND) = (defview.as_ptr(), std::ptr::null_mut());
        EnumWindows(Some(host_proc), &mut pair as *mut _ as LPARAM);
        let worker = pair.1;

        let shell = GetShellWindow();
        if !shell.is_null() && class_name(shell) == "Progman" {
            if !FindWindowExW(
                shell,
                std::ptr::null_mut(),
                defview.as_ptr(),
                std::ptr::null(),
            )
            .is_null()
            {
                return shell as isize;
            }
            if !worker.is_null() {
                return worker as isize;
            }
            // 新版 Windows 在 Explorer 重启的瞬间可能暂时没有 DefView：
            // 仍然锚到 Progman，比掉到 HWND_BOTTOM 更稳。
            return shell as isize;
        }
        if !worker.is_null() {
            return worker as isize;
        }
        let folder_view = wide("FolderView");
        let list = FindWindowW(std::ptr::null(), folder_view.as_ptr());
        if !list.is_null() {
            let root = GetAncestor(list, GA_ROOT);
            if !root.is_null() {
                return root as isize;
            }
        }
        0
    }
}

/// 把窗口放到所有普通窗口的最底下（桌面宿主之上）。
pub fn send_to_bottom(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos;
    const HWND_BOTTOM: isize = -1;
    if hwnd == 0 {
        return;
    }
    // SAFETY: 标准窗口定位；尺寸和位置都不动。
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            HWND_BOTTOM as HWND,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

/// 把窗口插到桌面图标宿主**正上方**、普通窗口之下。
///
/// 这是成熟桌面小组件的做法（PecoFence / DeskBox）：纯 `HWND_BOTTOM` 在
/// Explorer 抬升桌面、Win+D 之后会被桌面盖住；锚在宿主上面则始终浮在壁纸和
/// 图标之上、被普通窗口盖住。失败时退回最底层，至少还看得见。
pub fn anchor_above_desktop(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindow, SetWindowPos};
    const GW_HWNDPREV: u32 = 3;
    if hwnd == 0 {
        return false;
    }
    let host = shell_icon_host();
    if host == 0 {
        send_to_bottom(hwnd);
        return false;
    }
    // SAFETY: 标准 z-order 查询与定位。
    unsafe {
        // 插在宿主**正上方**：取宿主上邻的那一扇窗口当插入点，不管它现在
        // 可见不可见。以前这里会跳过不可见的窗口去找「第一个看得见的」，
        // 结果是：启动时某个程序正好最小化着，格子就插到了它的上面；那个
        // 程序一还原，格子就浮在了它前面——用户看到的就是「像置顶面板」。
        // 挨着宿主插，所有窗口（包括当时最小化的）都留在格子上面。
        let mut above = GetWindow(host as HWND, GW_HWNDPREV);
        // 宿主上面没有别的窗口：插在宿主正上方（传 null 会跑到最顶层，
        // 格子就会压住所有普通窗口）。
        if above.is_null() {
            above = host as HWND;
        } else if ex_topmost(above) && !ex_topmost(host as HWND) {
            // Win+D / 显示桌面会把桌面宿主抬到普通窗口上面，这时宿主正上方
            // 最近的那扇可见窗口是任务栏这类**置顶**窗口。拿它当插入点，
            // 格子自己也会被标成置顶——桌面落回去之后就永远浮在所有普通窗口
            // 上面了。这种时候直接贴在宿主上方，等桌面落回去再重锚。
            above = host as HWND;
        } else if above as isize == hwnd {
            // 已经紧贴在宿主上方，不要拿自己做锚点（SetWindowPos 会失败并
            // 退回最底层）。
            return true;
        }
        let insert_after = above;
        let ok = SetWindowPos(
            hwnd as HWND,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
        if ok == 0 {
            send_to_bottom(hwnd);
            false
        } else {
            true
        }
    }
}

/// 窗口是不是置顶（WS_EX_TOPMOST）。
pub fn is_topmost(hwnd: isize) -> bool {
    hwnd != 0 && ex_topmost(hwnd as HWND)
}

fn ex_topmost(hwnd: HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongW, GWL_EXSTYLE, WS_EX_TOPMOST};
    // SAFETY: 标准窗口查询。
    let style = unsafe { GetWindowLongW(hwnd as _, GWL_EXSTYLE) };
    style & WS_EX_TOPMOST as i32 != 0
}

/// 格子是不是浮到了普通窗口上面（而不是紧贴着桌面宿主）。
///
/// 两种情况：
/// - 被标成了置顶。正常插在宿主上方是普通 z-order；只有「相对置顶窗口定位」
///   的 `SetWindowPos` 会顺手把它也变成置顶（Win+D 抬起桌面时最容易踩到）。
/// - z-order 里和宿主之间夹着一扇带标题的可见窗口——比如锚定时那个程序正好
///   最小化着，格子插到了它上面，它一还原格子就浮在它前面。
///
/// `siblings` 是其他格子窗口，夹在中间不算数。没有标题的辅助窗口（IME、
/// DWM 监听窗）不算，免得每次看门狗都白锚一遍。
pub fn floats_above_windows(hwnd: isize, siblings: &[isize]) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindow;
    const GW_HWNDNEXT: u32 = 2;
    let host = shell_icon_host();
    if hwnd == 0 || host == 0 {
        return false;
    }
    if is_topmost(hwnd) && !is_topmost(host) {
        return true;
    }
    let mut cursor = unsafe { GetWindow(hwnd as HWND, GW_HWNDNEXT) };
    let mut guard = 0;
    while !cursor.is_null() && guard < 4096 {
        if cursor as isize == host {
            return false;
        }
        if siblings.contains(&(cursor as isize)) {
            cursor = unsafe { GetWindow(cursor, GW_HWNDNEXT) };
            guard += 1;
            continue;
        }
        if visible_surface(cursor) && window_title(cursor).is_some() && !ex_topmost(cursor) {
            return true;
        }
        cursor = unsafe { GetWindow(cursor, GW_HWNDNEXT) };
        guard += 1;
    }
    false
}

fn window_title(hwnd: HWND) -> Option<String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
    // SAFETY: 标准窗口查询；先问长度再按长度取，不会越界。
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return None;
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        if copied <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..copied as usize]))
    }
}

fn visible_surface(hwnd: HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
    // SAFETY: 标准窗口查询。
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return false;
        }
        rect.right > rect.left && rect.bottom > rect.top
    }
}

/// 桌面宿主是不是跑到了格子上面（Win+D / 资源管理器抬升桌面之后）。
pub fn behind_desktop(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindow;
    const GW_HWNDPREV: u32 = 3;
    if hwnd == 0 {
        return false;
    }
    let host = shell_icon_host();
    if host == 0 {
        return false;
    }
    // SAFETY: 只沿 z-order 链往上走，带步数上限防止环。
    unsafe {
        let mut cursor = GetWindow(hwnd as HWND, GW_HWNDPREV);
        let mut guard = 0;
        while !cursor.is_null() && guard < 4096 {
            if cursor as isize == host {
                return true;
            }
            cursor = GetWindow(cursor, GW_HWNDPREV);
            guard += 1;
        }
    }
    false
}

/// 前台窗口所属的顶层窗口（经过 `GA_ROOT`：菜单、弹出层都归到它的主窗口）。
pub fn foreground_root() -> isize {
    let Some(hwnd) = foreground_hwnd() else {
        return 0;
    };
    // SAFETY: 标准窗口查询。
    unsafe {
        let root = GetAncestor(hwnd as HWND, GA_ROOT);
        if root.is_null() {
            hwnd
        } else {
            root as isize
        }
    }
}

/// 窗口所属进程 id；句柄无效时是 0。
pub fn window_pid(hwnd: isize) -> u32 {
    if hwnd == 0 {
        return 0;
    }
    let mut pid = 0u32;
    // SAFETY: 标准窗口查询。
    unsafe { GetWindowThreadProcessId(hwnd as HWND, &mut pid) };
    pid
}

/// 窗口类名。
pub fn window_class_name(hwnd: isize) -> String {
    if hwnd == 0 {
        return String::new();
    }
    class_name(hwnd as HWND)
}

/// 前台是不是资源管理器抬起来的桌面窗口。
///
/// Win+D / 「显示桌面」不最小化任何窗口：资源管理器把 `Progman`（24H2 之后）
/// 或图标所在的 `WorkerW`（更早的版本）抬到所有窗口上面并拿走前台。认出这一
/// 刻靠两件事——根窗口类是桌面类，并且和 shell 窗口同进程；只看类名不够，
/// 别的程序也能注册叫 WorkerW 的窗口。
pub fn foreground_is_desktop() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetShellWindow;
    let root = foreground_root();
    if root == 0 {
        return false;
    }
    let class = window_class_name(root);
    if class != "Progman" && class != "WorkerW" {
        return false;
    }
    // SAFETY: 标准窗口查询。
    let shell_pid = window_pid(unsafe { GetShellWindow() } as isize);
    shell_pid != 0 && window_pid(root) == shell_pid
}

/// 前台窗口是不是资源管理器的界面：桌面本身，或任务栏、桌面右键菜单、开始
/// 菜单这些 shell 窗口（同一个进程）。
///
/// 「显示桌面」之后用户往往还会点任务栏或者右键桌面——桌面还抬着，前台却换成
/// 了菜单 / 任务栏，这时格子仍应留在桌面后面。鼠标点到上面时，这些窗口都属于
/// Explorer 的进程。
pub fn foreground_is_shell() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetShellWindow;
    let root = foreground_root();
    if root == 0 {
        return false;
    }
    if foreground_is_desktop() {
        return true;
    }
    // SAFETY: 标准窗口查询。
    let shell_pid = window_pid(unsafe { GetShellWindow() } as isize);
    shell_pid != 0 && window_pid(root) == shell_pid
}

/// 桌面宿主是不是被抬到了所有普通窗口前面。
///
/// Win+D 先抬桌面、后换前台，中间有个很短的窗口：那一两毫秒里格子已经被压到
/// 宿主下面，前台却还是原来的程序。只看前台会错过这一刻，然后（「保持可见」
/// 模式下）把格子拉回桌面上面，此后再也认不出「显示桌面」了。宿主自己是不是
/// 在所有普通窗口前面，正好补上这个空档——任务栏这类置顶窗口不算，最小化的
/// 窗口也不算。
pub fn desktop_raised_over_windows() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindow;
    const GW_HWNDPREV: u32 = 3;
    let host = shell_icon_host();
    if host == 0 {
        return false;
    }
    // SAFETY: 只沿 z-order 链往上走，带步数上限防止环。
    unsafe {
        let mut cursor = GetWindow(host as HWND, GW_HWNDPREV);
        let mut guard = 0;
        while !cursor.is_null() && guard < 4096 {
            if visible_surface(cursor) && window_title(cursor).is_some() && !ex_topmost(cursor) {
                return false;
            }
            cursor = GetWindow(cursor, GW_HWNDPREV);
            guard += 1;
        }
    }
    true
}

pub fn window_rect(hwnd: isize) -> Option<Rect> {
    if hwnd == 0 {
        return None;
    }
    let mut rect = RECT::default();
    // SAFETY: 标准窗口查询。
    let ok = unsafe { GetWindowRect(hwnd as HWND, &mut rect) };
    if ok == 0 {
        return None;
    }
    Some(Rect::new(rect.left, rect.top, rect.right, rect.bottom))
}

/// `IsWindowVisible`：窗口自己（不含最小化状态）是否可见。
pub fn is_window_visible(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    // SAFETY: 标准窗口查询。
    unsafe { IsWindowVisible(hwnd as HWND) != 0 }
}

/// 直接摆窗口（物理像素）：不动 z-order、不激活。
pub fn set_window_rect(hwnd: isize, rect: Rect) {
    if hwnd == 0 {
        return;
    }
    // SAFETY: 标准窗口定位。
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            std::ptr::null_mut(),
            rect.left,
            rect.top,
            rect.width().max(1),
            rect.height().max(1),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub fn minimized(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsIconic;
    if hwnd == 0 {
        return false;
    }
    // SAFETY: 标准窗口查询。
    unsafe { IsIconic(hwnd as HWND) != 0 }
}

/// Win+D 之后把窗口重新显示出来并锚回桌面层。
pub fn restore_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE};
    if hwnd == 0 {
        return;
    }
    // SAFETY: 标准窗口操作；SW_SHOWNOACTIVATE 不会抢前台。
    unsafe {
        ShowWindow(hwnd as HWND, SW_SHOWNOACTIVATE);
    }
    anchor_above_desktop(hwnd);
}

/// A point's monitor.
pub fn monitor_at(x: i32, y: i32) -> Option<Monitor> {
    // SAFETY: 标准显示器查询。
    unsafe {
        let handle = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        if handle.is_null() {
            return None;
        }
        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(handle, &mut info.monitorInfo) == 0 {
            return None;
        }
        let len = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
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
            device: String::from_utf16_lossy(&info.szDevice[..len]),
        })
    }
}

/// Windows 的「已知文件夹」（桌面 / 文档）。文档目录可能被 OneDrive 重定向，
/// 拼 `%USERPROFILE%\Documents` 不一定对，所以走系统 API。
fn known_folder(id: &GUID) -> Option<std::path::PathBuf> {
    unsafe {
        let mut wide: windows_sys::core::PWSTR = std::ptr::null_mut();
        // KF_FLAG_DEFAULT = 0；htoken 为空 = 当前用户。
        let hr = SHGetKnownFolderPath(
            id,
            0,
            std::ptr::null_mut(),
            &mut wide,
        );
        if hr < 0 || wide.is_null() {
            return None;
        }
        let mut len = 0usize;
        while len < 32_768 && *wide.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(wide, len);
        let path = std::path::PathBuf::from(OsString::from_wide(slice));
        CoTaskMemFree(wide as *const c_void);
        (!path.as_os_str().is_empty()).then_some(path)
    }
}

/// 系统桌面目录（Win+D 收起来的那个；不是桌面的某个快捷方式里）。
pub fn desktop_dir() -> Option<std::path::PathBuf> {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Desktop)
}

/// 「文档」目录。收纳盒的幕后目录默认放这儿：不是缓存区，清理软件不会误删。
pub fn documents_dir() -> Option<std::path::PathBuf> {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Documents)
}

/// 「下载」目录：整理规则最常用的来源。
pub fn downloads_dir() -> Option<std::path::PathBuf> {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Downloads)
}

/// 当前线程 id。拖放自测拿它和「登记拖放目标的线程」比：OLE 要求登记线程
/// 自己泵消息，登记到别的线程上，拖拽的一方会一直卡住。
pub fn current_thread_id() -> u32 {
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    // SAFETY: 无参数、无副作用。
    unsafe { GetCurrentThreadId() }
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
