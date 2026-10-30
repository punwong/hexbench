//! 给没有拖放目标的窗口挂一个 `IDropTarget`。
//!
//! Windows 上拖放目标由 OLE 按窗口登记：没登记过的窗口，鼠标拖到上面时
//! 系统什么都不发 —— 连「有东西拖进来了」都不知道。Tauri 只在先创建的那个
//! webview 上登记，我们的呼出面板是后建的，于是外部拖进来的文件会被系统
//! 送给下面的主窗口（表现就是「拖不进面板」）。
//!
//! 这里手写一个最小的 `IDropTarget`（只认 `CF_HDROP` 路径列表），把
//! enter / over / leave / drop 连同**客户区**坐标交给上层（OLE 给的是屏幕
//! 坐标，这里换算成窗口客户区坐标，界面直接拿去点到元素）。

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

/// 拖到窗口上的四个时刻。坐标是窗口客户区的物理像素。
pub enum DropEvent {
    Enter { paths: Vec<PathBuf>, x: i32, y: i32 },
    Over { x: i32, y: i32 },
    Leave,
    Drop { paths: Vec<PathBuf>, x: i32, y: i32 },
}

type Handler = Box<dyn Fn(DropEvent) + Send + Sync>;

static HANDLER: OnceLock<Mutex<Handler>> = OnceLock::new();

const CF_HDROP: u16 = 15;
const DVASPECT_CONTENT: u32 = 1;
const TYMED_HGLOBAL: i32 = 1;
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_NONE: u32 = 0;

const S_OK: i32 = 0;
/// 正在从我们自己的窗口往外拖（这时不能把自己当落点，否则会去读自己的数据对象）。
static DRAG_OUT_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_drag_out(active: bool) {
    DRAG_OUT_ACTIVE.store(active, Ordering::SeqCst);
}

fn dragging_out() -> bool {
    DRAG_OUT_ACTIVE.load(Ordering::SeqCst)
}

/// 外面问：现在是不是我们自己正在往外拖。
pub fn is_dragging_out() -> bool {
    dragging_out()
}
const DRAGDROP_E_ALREADYREGISTERED: i32 = -2147221247; // 0x80040101
const E_NOINTERFACE: i32 = -2147467262; // 0x80004002

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const IID_IUNKNOWN: Guid = Guid {
    data1: 0,
    data2: 0,
    data3: 0,
    data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46],
};
const IID_IDROP_TARGET: Guid = Guid {
    data1: 0x0000_0122,
    data2: 0,
    data3: 0,
    data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46],
};

/// 注意：`IDropTarget` 的坐标是**按值传递**的 `POINTL`（和 IDataObject 那些
/// 接口不一样），写成指针会在第一次拖拽时把坐标当成地址解引用 —— 直接崩。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PointL {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
struct FormatEtc {
    cf_format: u16,
    pt_d: *mut c_void,
    dw_aspect: u32,
    lindex: i32,
    tymed: i32,
}

#[repr(C)]
struct StgMedium {
    tymed: i32,
    union_member: *mut c_void,
    p_unk_for_release: *mut c_void,
}

#[repr(C)]
struct TargetVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    drag_enter:
        unsafe extern "system" fn(*mut c_void, *mut c_void, u32, PointL, *mut u32) -> i32,
    drag_over: unsafe extern "system" fn(*mut c_void, u32, PointL, *mut u32) -> i32,
    drag_leave: unsafe extern "system" fn(*mut c_void) -> i32,
    drop_item: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, PointL, *mut u32) -> i32,
}

#[repr(C)]
struct Target {
    vtbl: *const TargetVtbl,
    refs: AtomicU32,
    /// 挂在这个窗口上：OLE 给的屏幕坐标要按它换算成客户区坐标。
    hwnd: isize,
}

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> i32 {
    unsafe {
        if out.is_null() {
            return E_NOINTERFACE;
        }
        let iid = *riid;
        if iid == IID_IUNKNOWN || iid == IID_IDROP_TARGET {
            *out = this;
            add_ref(this);
            S_OK
        } else {
            *out = std::ptr::null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let target = &*(this as *const Target);
        target.refs.fetch_add(1, Ordering::SeqCst) + 1
    }
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    unsafe {
        let target = &*(this as *const Target);
        let left = target.refs.fetch_sub(1, Ordering::SeqCst) - 1;
        if left == 0 {
            drop(Box::from_raw(this as *mut Target));
        }
        left
    }
}

/// OLE 的 `POINTL` 是屏幕坐标：换成这个窗口的客户区坐标再交出去。
unsafe fn client_point(this: *mut c_void, point: PointL) -> (i32, i32) {
    let target = unsafe { &*(this as *const Target) };
    crate::sys::screen_to_client(target.hwnd, point.x, point.y)
}

unsafe fn dispatch(event: DropEvent) {
    if let Some(lock) = HANDLER.get() {
        if let Ok(guard) = lock.lock() {
            guard(event);
        }
    }
}

unsafe extern "system" fn drag_enter(
    this: *mut c_void,
    data: *mut c_void,
    _keys: u32,
    point: PointL,
    effect: *mut u32,
) -> i32 {
    unsafe {
        // 自己拖出去的时候，落点判断由外层负责，这里直接拒绝。
        if dragging_out() {
            if !effect.is_null() {
                *effect = DROPEFFECT_NONE;
            }
            return S_OK;
        }
        let paths = paths_from(data);
        let (x, y) = client_point(this, point);
        dispatch(DropEvent::Enter { paths, x, y });
        if !effect.is_null() {
            *effect = DROPEFFECT_COPY;
        }
        S_OK
    }
}

unsafe extern "system" fn drag_over(
    this: *mut c_void,
    _keys: u32,
    point: PointL,
    effect: *mut u32,
) -> i32 {
    unsafe {
        if dragging_out() {
            if !effect.is_null() {
                *effect = DROPEFFECT_NONE;
            }
            return S_OK;
        }
        let (x, y) = client_point(this, point);
        dispatch(DropEvent::Over { x, y });
        if !effect.is_null() {
            *effect = DROPEFFECT_COPY;
        }
        S_OK
    }
}

unsafe extern "system" fn drag_leave(_this: *mut c_void) -> i32 {
    unsafe {
        dispatch(DropEvent::Leave);
        S_OK
    }
}

unsafe extern "system" fn drop_item(
    this: *mut c_void,
    data: *mut c_void,
    _keys: u32,
    point: PointL,
    effect: *mut u32,
) -> i32 {
    unsafe {
        if dragging_out() {
            if !effect.is_null() {
                *effect = DROPEFFECT_NONE;
            }
            return S_OK;
        }
        let (x, y) = client_point(this, point);
        let paths = paths_from(data);
        dispatch(DropEvent::Drop { paths, x, y });
        if !effect.is_null() {
            *effect = DROPEFFECT_COPY;
        }
        S_OK
    }
}

static VTBL: TargetVtbl = TargetVtbl {
    query_interface,
    add_ref,
    release,
    drag_enter,
    drag_over,
    drag_leave,
    drop_item,
};

type GetDataFn = unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut StgMedium) -> i32;

/// 从 `IDataObject` 里读出 `CF_HDROP` 的路径。
unsafe fn paths_from(data: *mut c_void) -> Vec<PathBuf> {
    unsafe {
    let mut paths = Vec::new();
    if data.is_null() {
        return paths;
    }
    // COM 对象的第一格是 vtable 指针；IDataObject 继承 IUnknown，
    // GetData 是第 4 个槽（QueryInterface/AddRef/Release 之后）。
    let vtbl = *(data as *const *const *const usize);
    let get_data: GetDataFn = std::mem::transmute(*vtbl.add(3));
    let format = FormatEtc {
        cf_format: CF_HDROP,
        pt_d: std::ptr::null_mut(),
        dw_aspect: DVASPECT_CONTENT,
        lindex: -1,
        tymed: TYMED_HGLOBAL,
    };
    let mut medium = StgMedium {
        tymed: 0,
        union_member: std::ptr::null_mut(),
        p_unk_for_release: std::ptr::null_mut(),
    };
    if get_data(data, &format, &mut medium) < 0 {
        return paths;
    }
    let hdrop = medium.union_member;
    if !hdrop.is_null() {
        let count = DragQueryFileW(hdrop, u32::MAX, std::ptr::null_mut(), 0);
        for index in 0..count {
            let len = DragQueryFileW(hdrop, index, std::ptr::null_mut(), 0);
            if len == 0 {
                continue;
            }
            let mut buffer = vec![0u16; len as usize + 1];
            let written = DragQueryFileW(hdrop, index, buffer.as_mut_ptr(), len + 1);
            if written > 0 {
                paths.push(PathBuf::from(String::from_utf16_lossy(
                    &buffer[..written as usize],
                )));
            }
        }

    }
    ReleaseStgMedium(&mut medium);
    paths
    }
}

use windows_sys::Win32::UI::WindowsAndMessaging::EnumChildWindows;

#[link(name = "ole32")]
unsafe extern "system" {
    fn RegisterDragDrop(hwnd: *mut c_void, target: *mut c_void) -> i32;
    fn RevokeDragDrop(hwnd: *mut c_void) -> i32;
    fn ReleaseStgMedium(medium: *mut StgMedium);
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn DragQueryFileW(hdrop: *mut c_void, index: u32, buffer: *mut u16, size: u32) -> u32;

}


/// 已经挂过的窗口，重复调用时跳过。
static ATTACHED: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// 挂一个窗口。已经挂过就直接返回。
fn register_one(hwnd: isize) -> Result<(), String> {
    if let Ok(list) = ATTACHED.lock() {
        if list.contains(&hwnd) {
            return Ok(());
        }
    }
    let target = Box::new(Target {
        vtbl: &VTBL,
        refs: AtomicU32::new(1),
        hwnd,
    });
    let raw = Box::into_raw(target);
    let mut result = unsafe { RegisterDragDrop(hwnd as *mut c_void, raw as *mut c_void) };
    // 窗口上已经有一个（框架自己登记的）目标：它不给我们事件，直接换掉。
    if result == DRAGDROP_E_ALREADYREGISTERED {
        unsafe { RevokeDragDrop(hwnd as *mut c_void) };
        result = unsafe { RegisterDragDrop(hwnd as *mut c_void, raw as *mut c_void) };
    }
    if result < 0 {
        unsafe { drop(Box::from_raw(raw)) };
        return Err(format!("RegisterDragDrop 0x{result:08x}"));
    }
    if let Ok(mut list) = ATTACHED.lock() {
        list.push(hwnd);
    }
    Ok(())
}

unsafe extern "system" fn collect_child(hwnd: *mut c_void, param: isize) -> i32 {
    let list = unsafe { &mut *(param as *mut Vec<isize>) };
    list.push(hwnd as isize);
    1
}

/// 给窗口和它的所有子窗口挂上拖放目标。
///
/// 这一步很关键：WebView2 的拖放目标是挂在**子窗口**上的（wry 的
/// `drag_drop.rs` 就是这么做的 —— 遍历子窗口逐个 `RevokeDragDrop` 再
/// `RegisterDragDrop`）。只挂顶层窗口的话，鼠标落在网页区域时事件被
/// 子窗口吃掉，我们这边什么也收不到。
pub fn attach(hwnd: isize, callback: Handler) -> Result<(), String> {
    let _ = HANDLER.set(Mutex::new(callback));
    attach_more(hwnd)
}

/// 重新扫一遍子窗口（WebView2 的子窗口是异步建出来的，得补挂一次）。
pub fn attach_more(hwnd: isize) -> Result<(), String> {
    let mut handles = vec![hwnd];
    unsafe {
        EnumChildWindows(
            hwnd as *mut c_void,
            Some(collect_child),
            &mut handles as *mut Vec<isize> as isize,
        );
    }
    let mut first_error = None;
    let mut attached = 0;
    for handle in handles {
        match register_one(handle) {
            Ok(()) => attached += 1,
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    match first_error {
        Some(error) if attached == 0 => Err(error),
        _ => Ok(()),
    }
}
