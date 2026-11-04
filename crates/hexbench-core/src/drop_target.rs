//! 给没有拖放目标的窗口挂一个 `IDropTarget`。
//!
//! Windows 上拖放目标由 OLE 按窗口登记：没登记过的窗口，鼠标拖到上面时
//! 系统什么都不发 —— 连「有东西拖进来了」都不知道。Tauri 只在先创建的那个
//! webview 上登记，我们的呼出面板是后建的，于是外部拖进来的文件会被系统
//! 送给下面的主窗口（表现就是「拖不进面板」）。
//!
//! 这里手写一个最小的 `IDropTarget`（真实文件走 `CF_HDROP` 路径列表，
//! 浏览器拖网页图片给的「虚拟文件」走 `FileGroupDescriptorW` + `FileContents`），
//! 把 enter / over / leave / drop 连同**客户区**坐标交给上层（OLE 给的是屏幕
//! 坐标，这里换算成窗口客户区坐标，界面直接拿去点到元素）。

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// 拖到窗口上的四个时刻。坐标是窗口客户区的物理像素；`keys` 是 OLE 给的
/// 修饰键状态（`MK_CONTROL` 之类），落点据此决定「复制」还是「移动」。
pub enum DropEvent {
    Enter {
        paths: Vec<PathBuf>,
        x: i32,
        y: i32,
        keys: u32,
    },
    Over {
        x: i32,
        y: i32,
        keys: u32,
    },
    Leave,
    Drop {
        paths: Vec<PathBuf>,
        x: i32,
        y: i32,
        keys: u32,
    },
}

/// 回调处理完事件后可以指定落点的效果（`DROPEFFECT_*`）；`None` 用默认的复制。
pub type Effect = Option<u32>;

type Handler = Box<dyn Fn(DropEvent) -> Effect + Send + Sync>;

/// 每个顶层窗口一个回调：快捷面板和桌面格子可以同时收拖放事件。
static HANDLERS: OnceLock<Mutex<HashMap<isize, Arc<Handler>>>> = OnceLock::new();

fn handlers() -> &'static Mutex<HashMap<isize, Arc<Handler>>> {
    HANDLERS.get_or_init(|| Mutex::new(HashMap::new()))
}

const CF_HDROP: u16 = 15;
const DVASPECT_CONTENT: u32 = 1;
const TYMED_HGLOBAL: i32 = 1;
const TYMED_ISTREAM: i32 = 4;
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_NONE: u32 = 0;

/// Chromium 系浏览器拖网页图片时用的「虚拟文件」格式名。
///
/// 从网页里拖下来的图片不是磁盘上的文件：`CF_HDROP` 根本没有，只有
/// `FileGroupDescriptorW`（文件名）和 `FileContents`（一条 `IStream`）。
/// 不认这两个格式，拖进面板就是「看着能放、松手什么都没有」。
const GROUP_DESCRIPTOR_FORMAT: &str = "FileGroupDescriptorW";
const FILE_CONTENTS_FORMAT: &str = "FileContents";
/// `FILEDESCRIPTORW` 的定长字节数。
const FILEDESCRIPTOR_SIZE: usize = 620;
/// `cFileName` 在 `FILEDESCRIPTORW` 里的偏移。
const FILEDESCRIPTOR_NAME: usize = 72;

const S_OK: i32 = 0;
/// 正在从哪个窗口往外拖：只有它自己不能当落点，其他窗口照常接收。
///
/// 之前是一个全局 bool：桌面格子之间互相拖文件时，所有落点都被当成「自己」，
/// 拖出去再拖进另一个格子会什么都不发生。
static DRAG_SOURCE: AtomicIsize = AtomicIsize::new(0);

pub fn set_drag_source(hwnd: isize) {
    DRAG_SOURCE.store(hwnd, Ordering::SeqCst);
}

pub fn clear_drag_source() {
    DRAG_SOURCE.store(0, Ordering::SeqCst);
}

fn dragging_from(route: isize) -> bool {
    let source = DRAG_SOURCE.load(Ordering::SeqCst);
    source != 0 && source == route
}

/// 外面问：现在是不是我们自己正在往外拖。
pub fn is_dragging_out() -> bool {
    DRAG_SOURCE.load(Ordering::SeqCst) != 0
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
    /// 实际登记 OLE 的窗口（可能是 WebView2 的子窗口）。
    hwnd: isize,
    /// 顶层窗口：事件按它找回调。
    route: isize,
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

unsafe fn dispatch(route: isize, event: DropEvent) -> Effect {
    // 先克隆再解锁：回调里可能会再进拖放代码，不能在持锁时执行。
    let handler = handlers()
        .lock()
        .ok()
        .and_then(|map| map.get(&route).cloned());
    match handler {
        Some(handler) => handler(event),
        None => None,
    }
}

unsafe extern "system" fn drag_enter(
    this: *mut c_void,
    data: *mut c_void,
    keys: u32,
    point: PointL,
    effect: *mut u32,
) -> i32 {
    unsafe {
        // 自己拖出去的时候，落点判断由外层负责，这里直接拒绝；
        // 别的窗口（另一个格子）照常接收。
        let route = (*(this as *const Target)).route;
        if dragging_from(route) {
            if !effect.is_null() {
                *effect = DROPEFFECT_NONE;
            }
            return S_OK;
        }
        let paths = paths_from(data);
        let (x, y) = client_point(this, point);
        let chosen = dispatch(route, DropEvent::Enter { paths, x, y, keys });
        if !effect.is_null() {
            *effect = chosen.unwrap_or(DROPEFFECT_COPY);
        }
        S_OK
    }
}

unsafe extern "system" fn drag_over(
    this: *mut c_void,
    keys: u32,
    point: PointL,
    effect: *mut u32,
) -> i32 {
    unsafe {
        let route = (*(this as *const Target)).route;
        if dragging_from(route) {
            if !effect.is_null() {
                *effect = DROPEFFECT_NONE;
            }
            return S_OK;
        }
        let (x, y) = client_point(this, point);
        let chosen = dispatch(route, DropEvent::Over { x, y, keys });
        if !effect.is_null() {
            *effect = chosen.unwrap_or(DROPEFFECT_COPY);
        }
        S_OK
    }
}

unsafe extern "system" fn drag_leave(this: *mut c_void) -> i32 {
    unsafe {
        let route = (*(this as *const Target)).route;
        dispatch(route, DropEvent::Leave);
        S_OK
    }
}

unsafe extern "system" fn drop_item(
    this: *mut c_void,
    data: *mut c_void,
    keys: u32,
    point: PointL,
    effect: *mut u32,
) -> i32 {
    unsafe {
        let route = (*(this as *const Target)).route;
        if dragging_from(route) {
            if !effect.is_null() {
                *effect = DROPEFFECT_NONE;
            }
            return S_OK;
        }
        let (x, y) = client_point(this, point);
        let paths = drop_paths(data);
        let chosen = dispatch(route, DropEvent::Drop { paths, x, y, keys });
        if !effect.is_null() {
            *effect = chosen.unwrap_or(DROPEFFECT_COPY);
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
/// `IStream::Read`（`ISequentialStream` 里，IUnknown 之后第一个）。
type StreamReadFn = unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut u32) -> i32;

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
        if medium.tymed & TYMED_HGLOBAL == 0 || medium.union_member.is_null() {
            ReleaseStgMedium(&mut medium);
            return paths;
        }
        let hdrop = medium.union_member;
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
        ReleaseStgMedium(&mut medium);
        paths
    }
}

/// 注册过的剪贴板格式编号（0 表示注册失败）。
static GROUP_FORMAT: OnceLock<u16> = OnceLock::new();
static CONTENTS_FORMAT: OnceLock<u16> = OnceLock::new();

fn clipboard_format(name: &str) -> u16 {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: 字符串以 NUL 结尾，函数只读这一段。
    unsafe { RegisterClipboardFormatW(wide.as_ptr()) as u16 }
}

/// 按 `format` 取一次数据；成功时调用方负责 `ReleaseStgMedium`。
unsafe fn take_medium(data: *mut c_void, format: FormatEtc) -> Option<StgMedium> {
    unsafe {
        if data.is_null() {
            return None;
        }
        let vtbl = *(data as *const *const *const usize);
        let get_data: GetDataFn = std::mem::transmute(*vtbl.add(3));
        let mut medium = StgMedium {
            tymed: 0,
            union_member: std::ptr::null_mut(),
            p_unk_for_release: std::ptr::null_mut(),
        };
        if get_data(data, &format, &mut medium) < 0 {
            return None;
        }
        Some(medium)
    }
}

/// `FileGroupDescriptorW` 里的文件名列表。
///
/// 每一项是定长 620 字节的 `FILEDESCRIPTORW`，名字（`cFileName`）在 +72。
/// 有的来源只给到名字末尾（`GlobalSize` 比整个结构体短），所以按实际
/// 可用字节夹一下再读。
unsafe fn descriptor_names(data: *mut c_void) -> Vec<String> {
    unsafe {
        let format_id = *GROUP_FORMAT.get_or_init(|| clipboard_format(GROUP_DESCRIPTOR_FORMAT));
        if format_id == 0 {
            return Vec::new();
        }
        let format = FormatEtc {
            cf_format: format_id,
            pt_d: std::ptr::null_mut(),
            dw_aspect: DVASPECT_CONTENT,
            lindex: -1,
            tymed: TYMED_HGLOBAL,
        };
        let Some(mut medium) = take_medium(data, format) else {
            return Vec::new();
        };
        if medium.tymed & TYMED_HGLOBAL == 0 || medium.union_member.is_null() {
            ReleaseStgMedium(&mut medium);
            return Vec::new();
        }
        let size = GlobalSize(medium.union_member);
        // HGLOBAL 是句柄，不是内存地址：先锁住才能读。
        let locked = GlobalLock(medium.union_member);
        if locked.is_null() {
            ReleaseStgMedium(&mut medium);
            return Vec::new();
        }
        let mem = locked as *const u8;
        let mut names = Vec::new();
        if size >= 4 {
            // SAFETY: 前 4 字节是 cItems，GlobalSize 已经确认可读。
            let count = std::ptr::read_unaligned(mem as *const u32).min(64) as usize;
            for index in 0..count {
                let base = 4 + index * FILEDESCRIPTOR_SIZE + FILEDESCRIPTOR_NAME;
                if base >= size {
                    break;
                }
                let available = ((size - base) / 2).min(260);
                let mut units = Vec::with_capacity(available);
                for offset in 0..available {
                    // SAFETY: base + offset * 2 落在 GlobalSize 之内。
                    let unit = std::ptr::read_unaligned(mem.add(base + offset * 2) as *const u16);
                    if unit == 0 {
                        break;
                    }
                    units.push(unit);
                }
                names.push(String::from_utf16_lossy(&units));
            }
        }
        GlobalUnlock(medium.union_member);
        ReleaseStgMedium(&mut medium);
        names
    }
}

/// 把一项 `FileContents` 写成文件。`index = -1` 表示整组只有一个流。
unsafe fn write_contents(data: *mut c_void, format_id: u16, index: i32, path: &Path) -> bool {
    unsafe {
        let format = FormatEtc {
            cf_format: format_id,
            pt_d: std::ptr::null_mut(),
            dw_aspect: DVASPECT_CONTENT,
            lindex: index,
            tymed: TYMED_ISTREAM,
        };
        let Some(mut medium) = take_medium(data, format) else {
            return false;
        };
        let mut ok = false;
        if medium.tymed & TYMED_ISTREAM != 0 && !medium.union_member.is_null() {
            // IStream 前三个槽是 IUnknown，Read 是第 4 个。
            let vtbl = *(medium.union_member as *const *const *const usize);
            let read: StreamReadFn = std::mem::transmute(*vtbl.add(3));
            if let Ok(mut file) = File::create(path) {
                let mut buffer = vec![0u8; 64 * 1024];
                ok = true;
                loop {
                    let mut got = 0u32;
                    let result = read(
                        medium.union_member,
                        buffer.as_mut_ptr() as *mut c_void,
                        buffer.len() as u32,
                        &mut got,
                    );
                    if result < 0 {
                        ok = false;
                        break;
                    }
                    if got == 0 {
                        break;
                    }
                    if file.write_all(&buffer[..got as usize]).is_err() {
                        ok = false;
                        break;
                    }
                }
            }
        } else if medium.tymed & TYMED_HGLOBAL != 0 && !medium.union_member.is_null() {
            // 少数来源把内容当内存块给，顺手也认了。
            let size = GlobalSize(medium.union_member);
            let locked = GlobalLock(medium.union_member);
            if !locked.is_null() {
                // SAFETY: size 是这个内存块的大小。
                let bytes = std::slice::from_raw_parts(locked as *const u8, size);
                ok = std::fs::write(path, bytes).is_ok();
                GlobalUnlock(medium.union_member);
            }
        }
        ReleaseStgMedium(&mut medium);
        if !ok {
            let _ = std::fs::remove_file(path);
        }
        ok
    }
}

/// 浏览器虚拟文件的落地目录：每次拖放一个子目录，文件名保持原样。
fn drop_temp_dir() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    std::env::temp_dir()
        .join("hexbench-drop")
        .join(format!("{}-{stamp}", std::process::id()))
}

/// 清掉一小时前留下的落地目录（文件被搬走后它们通常是空的）。
fn prune_drop_temp() {
    let root = std::env::temp_dir().join("hexbench-drop");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age.as_secs() > 3600);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// 拖放源给的名字只留最后一段，并去掉 Windows 不允许的字符。
fn sanitized_name(raw: &str) -> String {
    let base = raw.rsplit(['\\', '/']).next().unwrap_or(raw).trim();
    let mut cleaned: String = base
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    cleaned = cleaned.trim_end_matches([' ', '.']).to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        cleaned = "拖入文件".to_string();
    }
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        cleaned.insert(0, '_');
    }
    // 名字太长就截断，保留扩展名；按字符截，别切坏 UTF-8。
    if cleaned.chars().count() > 150 {
        let extension = cleaned
            .rsplit_once('.')
            .map(|(_, ext)| format!(".{ext}"))
            .unwrap_or_default();
        let keep = 150usize.saturating_sub(extension.chars().count());
        let stem: String = cleaned.chars().take(keep).collect();
        cleaned = format!("{stem}{extension}");
    }
    cleaned
}

/// 同一个落地目录里重名时加序号，别互相覆盖。
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    for number in 2..1000 {
        let next = dir.join(format!("{stem} ({number}){extension}"));
        if !next.exists() {
            return next;
        }
    }
    dir.join(format!("{stem}-{}{extension}", std::process::id()))
}

/// 浏览器（Chromium 系）从网页里拖图片时给的不是 `CF_HDROP`，而是
/// 「虚拟文件」：`FileGroupDescriptorW` 说明名字，`FileContents` 是一条流。
/// 松手时把内容落成临时文件，后面就和真实文件走同一条搬运流程。
unsafe fn virtual_paths_from(data: *mut c_void) -> Vec<PathBuf> {
    unsafe {
        let contents = *CONTENTS_FORMAT.get_or_init(|| clipboard_format(FILE_CONTENTS_FORMAT));
        if contents == 0 {
            return Vec::new();
        }
        let names = descriptor_names(data);
        if names.is_empty() {
            return Vec::new();
        }
        prune_drop_temp();
        let dir = drop_temp_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return Vec::new();
        }
        let mut paths = Vec::new();
        for (index, raw) in names.iter().enumerate() {
            let path = unique_path(&dir, &sanitized_name(raw));
            // 单文件的来源有的只认 lindex = -1（整组一个流），再试一次。
            let saved = write_contents(data, contents, index as i32, &path)
                || (names.len() == 1 && write_contents(data, contents, -1, &path));
            if saved {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            let _ = std::fs::remove_dir(&dir);
        }
        paths
    }
}

/// 松手时的路径：真实文件优先；没有 `CF_HDROP` 就看是不是浏览器的虚拟文件。
unsafe fn drop_paths(data: *mut c_void) -> Vec<PathBuf> {
    unsafe {
        let mut paths = paths_from(data);
        if paths.is_empty() {
            paths = virtual_paths_from(data);
        }
        paths
    }
}

use windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetWindowThreadProcessId, IsWindow,
};

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


/// 已经挂过的窗口：(实际登记的窗口, 顶层窗口, 登记线程)。重复调用时跳过。
///
/// 记下登记线程是给自测用的：OLE 只把拖放回调送到这条线程，而这条线程必须
/// 自己泵消息（msdn 原话：否则任何拖着东西经过这个窗口的程序都会卡住）。
static ATTACHED: Mutex<Vec<(isize, isize, u32)>> = Mutex::new(Vec::new());

/// 挂一个窗口（子窗口也要挂）。已经挂过就直接返回。
fn register_one(hwnd: isize, route: isize) -> Result<(), String> {
    if let Ok(mut list) = ATTACHED.lock() {
        if let Some(index) = list.iter().position(|(child, _, _)| *child == hwnd) {
            // 窗口还活着才算挂过。句柄会被系统回收：旧窗口销毁后同一个值
            // 可能落到新窗口上，这时跳过登记，拖放会悄悄失效。
            if unsafe { IsWindow(hwnd as *mut c_void) } != 0 {
                return Ok(());
            }
            list.remove(index);
        }
    }
    let target = Box::new(Target {
        vtbl: &VTBL,
        refs: AtomicU32::new(1),
        hwnd,
        route,
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
        // SAFETY: 无参数、无副作用。
        list.push((hwnd, route, unsafe { GetCurrentThreadId() }));
    }
    Ok(())
}

/// 已挂过拖放目标的窗口：(窗口, 顶层窗口, 登记线程, 窗口所属线程)。
///
/// 自测用它验证「登记线程就是窗口所属、并且泵消息的主线程」。两者不一致时，
/// 资源管理器拖着文件经过这个窗口会一直卡在 `DragEnter` 上等我们。
pub fn attach_report() -> Vec<(isize, isize, u32, u32)> {
    let list = ATTACHED.lock().map(|list| list.clone()).unwrap_or_default();
    list.into_iter()
        .map(|(hwnd, route, registered)| {
            let mut pid = 0u32;
            // SAFETY: hwnd 是登记过的窗口；只读它的线程 / 进程 id。
            let owner = unsafe { GetWindowThreadProcessId(hwnd as *mut c_void, &mut pid) };
            (hwnd, route, registered, owner)
        })
        .collect()
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
    match handlers().lock() {
        Ok(mut map) => {
            map.insert(hwnd, Arc::new(callback));
        }
        Err(_) => return Err("拖放回调表已损坏".to_string()),
    }
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
        match register_one(handle, hwnd) {
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

/// 摘掉一个窗口的拖放登记（删除格子 / 关闭窗口时用）。
pub fn detach(hwnd: isize) {
    if let Ok(mut map) = handlers().lock() {
        map.remove(&hwnd);
    }
    let mut revoked = Vec::new();
    if let Ok(mut list) = ATTACHED.lock() {
        list.retain(|(child, route, _)| {
            if *route == hwnd {
                revoked.push(*child);
                false
            } else {
                true
            }
        });
    }
    for child in revoked {
        // SAFETY: 这些句柄都是之前登记过的；窗口可能已经销毁，失败也无妨。
        unsafe {
            RevokeDragDrop(child as *mut c_void);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const E_FAIL: i32 = -2147467259;
    const S_OK: i32 = 0;

    /// 假的 `IDataObject`：只实现 `GetData`，两种虚拟文件格式各答一次。
    #[repr(C)]
    struct FakeObject {
        vtbl: *const FakeObjectVtbl,
        group_format: u16,
        contents_format: u16,
        stream: *mut FakeStream,
    }

    #[repr(C)]
    struct FakeObjectVtbl {
        query_interface: unsafe extern "system" fn(*mut c_void, *const c_void, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_data: unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut StgMedium) -> i32,
    }

    #[repr(C)]
    struct FakeStream {
        vtbl: *const FakeStreamVtbl,
        data: Vec<u8>,
        pos: usize,
    }

    #[repr(C)]
    struct FakeStreamVtbl {
        query_interface: unsafe extern "system" fn(*mut c_void, *const c_void, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        read: StreamReadFn,
    }

    unsafe extern "system" fn fake_query_interface(
        _this: *mut c_void,
        _iid: *const c_void,
        out: *mut *mut c_void,
    ) -> i32 {
        unsafe {
            if !out.is_null() {
                *out = std::ptr::null_mut();
            }
        }
        E_FAIL
    }

    unsafe extern "system" fn fake_add_ref(_this: *mut c_void) -> u32 {
        1
    }

    unsafe extern "system" fn fake_release(_this: *mut c_void) -> u32 {
        1
    }

    unsafe extern "system" fn fake_read(
        this: *mut c_void,
        buffer: *mut c_void,
        length: u32,
        read_out: *mut u32,
    ) -> i32 {
        unsafe {
            let stream = &mut *(this as *mut FakeStream);
            let left = stream.data.len() - stream.pos;
            let take = left.min(length as usize);
            if take > 0 {
                std::ptr::copy_nonoverlapping(
                    stream.data.as_ptr().add(stream.pos),
                    buffer as *mut u8,
                    take,
                );
                stream.pos += take;
            }
            if !read_out.is_null() {
                *read_out = take as u32;
            }
        }
        S_OK
    }

    unsafe extern "system" fn fake_get_data(
        this: *mut c_void,
        format: *const FormatEtc,
        medium: *mut StgMedium,
    ) -> i32 {
        unsafe {
            let object = &*(this as *const FakeObject);
            let requested = &*format;
            if requested.cf_format == object.group_format {
                // 一个文件：cItems + FILEDESCRIPTORW（名字在 +72）。
                let mut bytes = vec![0u8; 4 + FILEDESCRIPTOR_SIZE];
                bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
                let name: Vec<u16> = "测试 图.png".encode_utf16().collect();
                for (index, unit) in name.iter().enumerate() {
                    let offset = 4 + FILEDESCRIPTOR_NAME + index * 2;
                    bytes[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
                }
                let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
                if handle.is_null() {
                    return E_FAIL;
                }
                let target = GlobalLock(handle);
                if target.is_null() {
                    return E_FAIL;
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, bytes.len());
                GlobalUnlock(handle);
                (*medium).tymed = TYMED_HGLOBAL;
                (*medium).union_member = handle;
                (*medium).p_unk_for_release = std::ptr::null_mut();
                return S_OK;
            }
            if requested.cf_format == object.contents_format
                && requested.tymed & TYMED_ISTREAM != 0
                && (requested.lindex == 0 || requested.lindex == -1)
            {
                (*medium).tymed = TYMED_ISTREAM;
                (*medium).union_member = object.stream as *mut c_void;
                (*medium).p_unk_for_release = std::ptr::null_mut();
                return S_OK;
            }
            E_FAIL
        }
    }

    static OBJECT_VTBL: FakeObjectVtbl = FakeObjectVtbl {
        query_interface: fake_query_interface,
        add_ref: fake_add_ref,
        release: fake_release,
        get_data: fake_get_data,
    };

    static STREAM_VTBL: FakeStreamVtbl = FakeStreamVtbl {
        query_interface: fake_query_interface,
        add_ref: fake_add_ref,
        release: fake_release,
        read: fake_read,
    };

    /// 浏览器拖网页图片的格式组合（没有 CF_HDROP）也要落成文件。
    #[test]
    fn browser_virtual_file_is_materialized() {
        let payload: Vec<u8> = (0..4096u32).map(|value| value as u8).collect();
        let mut stream = FakeStream {
            vtbl: &STREAM_VTBL,
            data: payload.clone(),
            pos: 0,
        };
        let mut object = FakeObject {
            vtbl: &OBJECT_VTBL,
            group_format: clipboard_format(GROUP_DESCRIPTOR_FORMAT),
            contents_format: clipboard_format(FILE_CONTENTS_FORMAT),
            stream: &mut stream,
        };
        assert_ne!(object.group_format, 0);
        assert_ne!(object.contents_format, 0);
        let paths = unsafe { drop_paths(&mut object as *mut FakeObject as *mut c_void) };
        assert_eq!(paths.len(), 1, "应落成一个临时文件");
        assert_eq!(
            paths[0].file_name().and_then(|name| name.to_str()),
            Some("测试 图.png")
        );
        assert_eq!(std::fs::read(&paths[0]).unwrap(), payload);
        let _ = std::fs::remove_file(&paths[0]);
        if let Some(dir) = paths[0].parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }

    #[test]
    fn names_from_other_apps_are_sanitized() {
        assert_eq!(sanitized_name("../../windows/evil.png"), "evil.png");
        assert_eq!(sanitized_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitized_name("   "), "拖入文件");
        assert_eq!(sanitized_name("a:b?.txt"), "a_b_.txt");
    }
}
