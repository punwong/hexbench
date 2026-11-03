//! Direct2D + DirectComposition 的 Windows 实现。
//!
//! 一条长期存活的渲染线程：创建 GPU 设备、注册窗口类、跑自己的消息循环。
//! `NativeFences` 从任意线程投递命令；窗口过程、绘制、图标解码都在这条线程上，
//! 所以 D2D 的单线程工厂和 DComp 的 surface 不需要加锁。
//!
//! 三个原型踩过的坑照抄（见 proto/fence-native/README.md）：
//! 1. `BeginDraw` 返回图层在图集里的像素偏移，绘制前要按 `offset / dpi` 平移；
//! 2. 每帧都要从返回的 DXGI surface 新建 `TARGET | CANNOT_DRAW` 的 D2D 位图；
//! 3. 窗口首次 Show 之后要补一帧（这里用 80 ms 的 `SetTimer`）。

#![allow(non_snake_case)]

use std::cell::Cell;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStrExt;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows::core::{w, Interface, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    COLORREF, GENERIC_READ, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
    D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_CAP_STYLE_ROUND,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_LINE_JOIN_ROUND,
    D2D1_QUADRATIC_BEZIER_SEGMENT, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES1,
    ID2D1Bitmap1, ID2D1Device, ID2D1DeviceContext, ID2D1Factory1, ID2D1Image, ID2D1PathGeometry,
    ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, ID3D11Device,
    ID3D11DeviceContext,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionSurface, IDCompositionTarget,
    IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TEXT_METRICS, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_EMERGENCY_BREAK, DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteFactory,
    IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, FillRect, SetBkColor, SetTextColor, HBRUSH, HDC,
    HFONT, HGDIOBJ, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    FF_DONTCARE, FW_NORMAL, OUT_DEFAULT_PRECIS, ScreenToClient, ValidateRect,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE,
    VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};

use hexbench_core::config::GestureSettings;
use hexbench_core::drop_target::{self, DropEvent};

use crate::{Bench, Crumb, Event, EventSink, Frame, Item};

// ---------- 和 fence.html / fence.tsx 同一套排版尺寸（逻辑像素，100% 缩放） ----------

const HEADER_H: f32 = 40.0;
/// 进了子目录后，标题条左边的「‹ 返回」热区宽度。
const BACK_W: f32 = 28.0;
/// 面包屑从标题的位置开始。
const NAV_START: f32 = 30.0;
const RADIUS: f32 = 8.0;
const TILE_RADIUS: f32 = 9.0;
const CELL_W: f32 = 86.0;
const CELL_H: f32 = 88.0;
const CELL_GAP: f32 = 12.0;
const CELL_PAD: f32 = 10.0;
const ICON_BASE: f32 = 32.0;
const NAME_TOP_PAD: f32 = 7.0;

/// 渲染线程收到新命令。
const WM_FENCE_WAKE: u32 = WM_APP + 1;
/// 左键松手后弹标题菜单（不在按钮按着的时候弹）。
const WM_FENCE_MENU: u32 = WM_APP + 2;
/// 拖放回调（OLE 线程）通知渲染线程更新落点高亮。
const WM_FENCE_DRAG: u32 = WM_APP + 3;

/// OLE 的落点效果：按住 Ctrl 是复制，否则移动（和资源管理器一致）。
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;
const MK_CONTROL: u32 = 0x0008;
/// 右键还按着（WM_MOUSEMOVE 的 wParam）。
const MK_RBUTTON: usize = 0x0002;
/// 左键还按着。
const MK_LBUTTON: usize = 0x0001;
/// Show 之后补一帧的定时器（坑 3）。
const TIMER_FIRST_FRAME: usize = 1;
/// 右键手势：移动超过这个距离才判方向（和网页版一致）。
const GESTURE_START: f32 = 8.0;
/// `windows` 把它生成在 `Win32_UI_Controls` 里，为一个常量再开一个 feature 不值得。
const WM_MOUSELEAVE: u32 = 0x02A3;
/// 条目上按住左键移动超过这个距离（物理像素）才发起文件拖拽，免得单击变拖拽。
const FILE_DRAG_START: i32 = 4;

thread_local! {
    static ENGINE: Cell<*mut Engine> = const { Cell::new(std::ptr::null_mut()) };
}

// ---------- 对外的宿主 ----------

enum Cmd {
    Sync(Vec<Frame>),
    Upsert(Frame),
    Icons {
        id: String,
        generation: u64,
        icons: Vec<(String, Option<std::path::PathBuf>)>,
    },
    Bench {
        id: String,
        frames: usize,
        reply: Sender<Option<Bench>>,
    },
    /// 自动化验证用：按 OLE 拖放回调的口径算一次落点，不碰任何文件。
    DropTarget {
        id: String,
        x: f32,
        y: f32,
        reply: Sender<Option<String>>,
    },
    /// 设置页改了右键手势：格子一直开着，不重开就要立刻生效。
    Gestures(GestureSettings),
    /// 在这个格子里弹一条提示（拖入失败、整理结果这类）。
    Toast { id: String, text: String },
    Shutdown,
}

pub struct NativeFences {
    tx: Sender<Cmd>,
    wake_tid: u32,
    hwnds: Arc<Mutex<HashMap<String, isize>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for NativeFences {
    fn drop(&mut self) {
        // 显式 shutdown 之外（比如线程 panic 后主机被丢弃），也别把渲染线程
        // 留在后台。重复调用是安全的：通道已关、join 已空。
        self.shutdown();
    }
}

impl NativeFences {
    pub fn start(sink: EventSink) -> std::result::Result<Arc<Self>, String> {
        let (tx, rx) = channel::<Cmd>();
        let (ready_tx, ready_rx) = channel::<std::result::Result<u32, String>>();
        let hwnds = Arc::new(Mutex::new(HashMap::new()));
        let thread_hwnds = Arc::clone(&hwnds);
        let join = thread::Builder::new()
            .name("hexbench-native-fence".to_string())
            .spawn(move || render_thread(rx, sink, thread_hwnds, ready_tx))
            .map_err(|error| error.to_string())?;
        let wake_tid = match ready_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(tid)) => tid,
            Ok(Err(error)) => {
                let _ = join.join();
                return Err(error);
            }
            Err(_) => {
                let _ = join.join();
                return Err("原生渲染线程启动超时".to_string());
            }
        };
        Ok(Arc::new(Self {
            tx,
            wake_tid,
            hwnds,
            join: Mutex::new(Some(join)),
        }))
    }

    fn post(&self, cmd: Cmd) {
        if self.tx.send(cmd).is_ok() {
            unsafe {
                let _ = PostThreadMessageW(self.wake_tid, WM_FENCE_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// 和配置对一遍：建 / 更新 / 删。一次调用里完成，避免中间状态。
    pub fn sync(&self, frames: Vec<Frame>) {
        self.post(Cmd::Sync(frames));
    }

    pub fn upsert(&self, frame: Frame) {
        self.post(Cmd::Upsert(frame));
    }

    pub fn set_icons(
        &self,
        id: &str,
        generation: u64,
        icons: Vec<(String, Option<std::path::PathBuf>)>,
    ) {
        self.post(Cmd::Icons {
            id: id.to_string(),
            generation,
            icons,
        });
    }

    pub fn bench(&self, id: &str, frames: usize) -> Option<Bench> {
        let (reply, answer) = channel();
        self.post(Cmd::Bench {
            id: id.to_string(),
            frames,
            reply,
        });
        answer
            .recv_timeout(std::time::Duration::from_secs(30))
            .ok()
            .flatten()
    }

    /// 调试用：和 OLE 回调一样算一次落点（只读）。`x` / `y` 是客户区物理像素。
    /// 滚动之后这里返回的应该是屏幕上真正那一格的文件夹——回归测试就靠它。
    pub fn drop_target(&self, id: &str, x: f32, y: f32) -> Option<String> {
        let (reply, answer) = channel();
        self.post(Cmd::DropTarget {
            id: id.to_string(),
            x,
            y,
            reply,
        });
        answer
            .recv_timeout(std::time::Duration::from_secs(2))
            .ok()
            .flatten()
    }

    /// 面板和格子共用的手势设置；设置页改完直接推过来。
    pub fn set_gestures(&self, gestures: GestureSettings) {
        self.post(Cmd::Gestures(gestures));
    }

    /// 在格子底部弹一条短提示，几秒后自己消失（网页版的 toast）。
    pub fn toast(&self, id: &str, text: &str) {
        self.post(Cmd::Toast {
            id: id.to_string(),
            text: text.to_string(),
        });
    }

    pub fn hwnds(&self) -> Vec<isize> {
        self.hwnds
            .lock()
            .map(|map| map.values().copied().collect())
            .unwrap_or_default()
    }

    /// 格子 id → 原生窗口句柄：吸附要拿其他格子的实际位置。
    pub fn hwnd_map(&self) -> Vec<(String, isize)> {
        self.hwnds
            .lock()
            .map(|map| {
                map.iter()
                    .map(|(id, hwnd)| (id.clone(), *hwnd))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Cmd::Shutdown);
        unsafe {
            let _ = PostThreadMessageW(self.wake_tid, WM_FENCE_WAKE, WPARAM(0), LPARAM(0));
        }
        if let Ok(mut slot) = self.join.lock() {
            if let Some(join) = slot.take() {
                let _ = join.join();
            }
        }
        if let Ok(mut map) = self.hwnds.lock() {
            map.clear();
        }
    }
}

// ---------- 渲染线程 ----------

struct Engine {
    gpu: Gpu,
    windows: Vec<WindowEntry>,
    sink: EventSink,
    /// 全局滚轮钩子：格子是 NOACTIVATE 窗口，永远拿不到焦点。
    /// 系统「悬停时滚动非活动窗口」关掉时（MouseWheelRouting=2），
    /// 滚轮会被前台窗口吃掉，只能在这里按光标位置转发。
    hook: HHOOK,
    /// 右键手势设置（和快捷面板共用一份）。
    gestures: GestureSettings,
}

struct WindowEntry {
    id: String,
    hwnd: HWND,
    ptr: *mut FenceWindow,
}

fn render_thread(
    rx: std::sync::mpsc::Receiver<Cmd>,
    sink: EventSink,
    hwnds: Arc<Mutex<HashMap<String, isize>>>,
    ready: Sender<std::result::Result<u32, String>>,
) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // Tauri 自己已经设过；重复设置会返回错误，忽略即可。
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // 窗口的拖放目标登记在这条线程上，OLE 必须在这条线程初始化。
    hexbench_core::sys::init_ole();
    let gpu = match Gpu::new() {
        Ok(gpu) => gpu,
        Err(error) => {
            let _ = ready.send(Err(format!("图形设备初始化失败：{error}")));
            return;
        }
    };
    if let Err(error) = register_classes() {
        let _ = ready.send(Err(format!("窗口类注册失败：{error}")));
        return;
    }
    let instance = gpu.instance;
    let engine = Box::into_raw(Box::new(Engine {
        gpu,
        windows: Vec::new(),
        sink,
        hook: HHOOK(std::ptr::null_mut()),
        gestures: GestureSettings::default(),
    }));
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance), 0) }
        .unwrap_or_default();
    if hook.0.is_null() {
        log::warn!("native fence: 滚轮钩子安装失败，非活动窗口的滚轮可能不可用");
    }
    unsafe {
        (*engine).hook = hook;
    }
    ENGINE.with(|cell| cell.set(engine));
    let tid = unsafe { GetCurrentThreadId() };
    let _ = ready.send(Ok(tid));
    log::info!("native fence: 渲染线程就绪（D3D11 + D2D + DComp + DWrite + WIC）");

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_FENCE_WAKE && msg.hwnd.is_invalid() {
                while let Ok(cmd) = rx.try_recv() {
                    let quit = handle_cmd(&mut *engine, cmd, &hwnds);
                    if quit {
                        PostQuitMessage(0);
                        break;
                    }
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // 万一 WM_QUIT 是别处来的，也要把窗口收干净。
        destroy_all(&mut *engine);
        if !(*engine).hook.0.is_null() {
            let _ = UnhookWindowsHookEx((*engine).hook);
        }
        ENGINE.with(|cell| cell.set(std::ptr::null_mut()));
        drop(Box::from_raw(engine));
        CoUninitialize();
    }
    log::info!("native fence: 渲染线程退出");
}

fn handle_cmd(engine: &mut Engine, cmd: Cmd, hwnds: &Mutex<HashMap<String, isize>>) -> bool {
    match cmd {
        Cmd::Sync(frames) => {
            let wanted: std::collections::HashSet<String> =
                frames.iter().map(|frame| frame.id.clone()).collect();
            let stale: Vec<HWND> = engine
                .windows
                .iter()
                .filter(|entry| !wanted.contains(&entry.id))
                .map(|entry| entry.hwnd)
                .collect();
            for hwnd in stale {
                drop_target::detach(hwnd.0 as isize);
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            for frame in frames {
                if let Err(error) = upsert(engine, frame) {
                    log::warn!("native fence: 建窗失败: {error}");
                }
            }
            publish_hwnds(engine, hwnds);
        }
        Cmd::Upsert(frame) => {
            if let Err(error) = upsert(engine, frame) {
                log::warn!("native fence: 更新失败: {error}");
            }
            publish_hwnds(engine, hwnds);
        }
        Cmd::Icons {
            id,
            generation,
            icons,
        } => {
            let Some(entry) = engine.windows.iter().find(|entry| entry.id == id) else {
                return false;
            };
            let fence = unsafe { &mut *entry.ptr };
            if fence.generation != generation {
                return false;
            }
            for (path, file) in icons {
                let Some(item) = fence.items.iter_mut().find(|item| item.path == path) else {
                    continue;
                };
                item.tried = true;
                if let Some(file) = file {
                    if let Some(bitmap) = load_icon(&engine.gpu, &fence.dc, &file) {
                        item.icon = Some(bitmap);
                    }
                }
            }
            if let Err(error) = fence.render(&engine.gpu) {
                log::warn!("native fence: 图标后重绘失败: {error}");
            }
        }
        Cmd::Bench {
            id,
            frames,
            reply,
        } => {
            let result = engine
                .windows
                .iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| {
                    let fence = unsafe { &mut *entry.ptr };
                    bench(engine, fence, frames)
                });
            let _ = reply.send(result);
        }
        Cmd::DropTarget {
            id,
            x,
            y,
            reply,
        } => {
            let result = engine
                .windows
                .iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| {
                    let fence = unsafe { &*entry.ptr };
                    let snapshot = fence.drop.lock().ok()?;
                    snapshot.destination(x, y)
                });
            let _ = reply.send(result);
        }
        Cmd::Toast { id, text } => {
            let Some(entry) = engine.windows.iter().find(|entry| entry.id == id) else {
                return false;
            };
            let fence = unsafe { &mut *entry.ptr };
            fence.show_toast(&engine.gpu, text);
        }
        Cmd::Gestures(gestures) => {
            engine.gestures = gestures;
            for entry in &engine.windows {
                let fence = unsafe { &mut *entry.ptr };
                fence.apply_gestures(&engine.gpu, engine.gestures.clone());
            }
        }
        Cmd::Shutdown => {
            destroy_all(engine);
            if let Ok(mut map) = hwnds.lock() {
                map.clear();
            }
            return true;
        }
    }
    false
}

fn publish_hwnds(engine: &Engine, hwnds: &Mutex<HashMap<String, isize>>) {
    if let Ok(mut map) = hwnds.lock() {
        map.clear();
        for entry in &engine.windows {
            map.insert(entry.id.clone(), entry.hwnd.0 as isize);
        }
    }
}

fn destroy_all(engine: &mut Engine) {
    let hwnds: Vec<HWND> = engine.windows.iter().map(|entry| entry.hwnd).collect();
    for hwnd in hwnds {
        drop_target::detach(hwnd.0 as isize);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
    engine.windows.clear();
}

fn upsert(engine: &mut Engine, frame: Frame) -> Result<()> {
    if let Some(index) = engine
        .windows
        .iter()
        .position(|entry| entry.id == frame.id)
    {
        let fence = unsafe { &mut *engine.windows[index].ptr };
        fence.update(&engine.gpu, frame)?;
        return Ok(());
    }
    let mut fence = Box::new(FenceWindow::new(&engine.gpu, frame)?);
    fence.gestures = engine.gestures.clone();
    let hwnd = fence.hwnd;
    let ptr = &mut *fence as *mut FenceWindow;
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        hexbench_core::sys::set_no_activate(hwnd.0 as isize, true);
        hexbench_core::sys::round_window_corners(hwnd.0 as isize);
        hexbench_core::sys::anchor_above_desktop(hwnd.0 as isize);
    }
    fence.render(&engine.gpu)?;
    // 外部拖进来的文件：OLE 目标登记在这条渲染线程上——窗口属于它、消息
    // 循环也在它上面，回调里只留快照和事件出口，不碰窗口本身。
    {
        let state = Arc::clone(&fence.drop);
        let sink = Arc::clone(&engine.sink);
        let id = fence.id.clone();
        let attach_id = id.clone();
        let raw = hwnd.0 as isize;
        let callback = Box::new(move |event: DropEvent| match event {
            DropEvent::Enter { x, y, keys, .. } | DropEvent::Over { x, y, keys } => {
                post_drop_point(raw, x, y);
                drop_effect(keys)
            }
            DropEvent::Leave => {
                post_drop_point(raw, -1, -1);
                None
            }
            DropEvent::Drop {
                paths,
                x,
                y,
                keys,
            } => {
                // 悬停在文件夹格子上就落进那个文件夹，否则落进当前目录。
                let dest = state
                    .lock()
                    .ok()
                    .and_then(|snapshot| snapshot.destination(x as f32, y as f32));
                if let Some(dest) = dest {
                    let paths: Vec<String> = paths
                        .iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect();
                    if !paths.is_empty() {
                        sink(Event::Drop {
                            id: id.clone(),
                            dest,
                            paths,
                        });
                    }
                }
                post_drop_point(raw, -1, -1);
                drop_effect(keys)
            }
        });
        match drop_target::attach(raw, callback) {
            Ok(()) => log::info!("native fence {attach_id}: 拖放目标已登记"),
            Err(error) => log::warn!("native fence {attach_id}: 拖放目标登记失败: {error}"),
        }
    }
    // 坑 3：DWM 在窗口第一次可见之后才把合成目标接进来，等它挂上再补一帧。
    unsafe {
        SetTimer(Some(hwnd), TIMER_FIRST_FRAME, 80, None);
    }
    let id = fence.id.clone();
    let _ = Box::into_raw(fence);
    engine.windows.push(WindowEntry { id, hwnd, ptr });
    Ok(())
}

/// 拖拽经过时让渲染线程把光标下的格子点亮（x < 0 = 清掉）。
fn post_drop_point(hwnd: isize, x: i32, y: i32) {
    let packed: isize = if x < 0 {
        -1
    } else {
        ((x as u32 & 0xffff) | ((y as u32 & 0xffff) << 16)) as isize
    };
    let window = HWND(hwnd as *mut core::ffi::c_void);
    let _ = unsafe { PostMessageW(Some(window), WM_FENCE_DRAG, WPARAM(0), LPARAM(packed)) };
}

fn drop_effect(keys: u32) -> Option<u32> {
    Some(if keys & MK_CONTROL != 0 {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT_MOVE
    })
}

/// 配置文件被手改坏也不至于滑半天不动：夹到 20–400px（和网页版一致）。
fn gesture_distance(value: u32) -> f32 {
    value.clamp(20, 400) as f32
}

/// 手势轨迹：和网页版 `gesture.tsx` 一样，以相邻采样点的中点为终点、
/// 采样点本身为控制点画二次曲线。整条轨迹是一条路径，只描一次边——
/// 逐段 `DrawLine` 时每个拐点都会把半透明再叠一层，线看着一格一格的。
fn gesture_path(factory: &ID2D1Factory1, points: &[(f32, f32)]) -> Result<ID2D1PathGeometry> {
    let geometry = unsafe { factory.CreatePathGeometry()? };
    let sink = unsafe { geometry.Open()? };
    unsafe {
        let (start_x, start_y) = points[0];
        sink.BeginFigure(Vector2 { X: start_x, Y: start_y }, D2D1_FIGURE_BEGIN_FILLED);
        if points.len() == 1 {
            // 刚判定还没动：画一段近似为零的线，圆头笔帽补成一个圆点。
            sink.AddLine(Vector2 { X: start_x + 0.01, Y: start_y });
        } else {
            for index in 1..points.len() - 1 {
                let (x, y) = points[index];
                let (next_x, next_y) = points[index + 1];
                let segment = D2D1_QUADRATIC_BEZIER_SEGMENT {
                    point1: Vector2 { X: x, Y: y },
                    point2: Vector2 { X: (x + next_x) * 0.5, Y: (y + next_y) * 0.5 },
                };
                sink.AddQuadraticBezier(&segment);
            }
            let (last_x, last_y) = points[points.len() - 1];
            sink.AddLine(Vector2 { X: last_x, Y: last_y });
        }
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
    }
    Ok(geometry.into())
}

/// 横向位移决定动作；默认向左后退，开了「左右互换」就反过来。
fn gesture_back(dx: f32, invert: bool) -> bool {
    if dx < 0.0 {
        !invert
    } else {
        invert
    }
}

fn bench(engine: &Engine, fence: &mut FenceWindow, frames: usize) -> Option<Bench> {
    if frames == 0 || fence.items.is_empty() {
        return None;
    }
    let saved_scroll = fence.scroll;
    let saved_hover = fence.hover;
    let max = fence.max_scroll();
    let mut samples = Vec::with_capacity(frames);
    for index in 0..frames {
        fence.scroll = if max > 0.0 {
            (index as f32 * 5.0) % max
        } else {
            0.0
        };
        fence.hover = Some(index % fence.items.len());
        let started = Instant::now();
        let _ = fence.render(&engine.gpu);
        samples.push(started.elapsed().as_micros() as u64);
    }
    // 基准测试把滚动位置来回拨了一遍：恢复时要连拖放快照一起还原。
    fence.set_scroll(saved_scroll);
    fence.hover = saved_hover;
    let _ = fence.render(&engine.gpu);
    samples.sort_unstable();
    Some(Bench {
        frames,
        avg_us: samples.iter().sum::<u64>() as f64 / samples.len() as f64,
        p50_us: samples[samples.len() / 2],
        p95_us: samples[samples.len() * 95 / 100],
        max_us: *samples.last().unwrap(),
    })
}

// ---------- GPU ----------

struct Gpu {
    _d3d: ID3D11Device,
    _ctx: ID3D11DeviceContext,
    device: ID2D1Device,
    /// 建 device 的那个工厂；每帧画手势轨迹要从它开路径几何。
    factory: ID2D1Factory1,
    dcomp: IDCompositionDevice,
    dwrite: IDWriteFactory,
    wic: IWICImagingFactory,
    instance: HINSTANCE,
    fmt_title: IDWriteTextFormat,
    fmt_count: IDWriteTextFormat,
    /// 面包屑：12px 常规，单行，长了省略。
    fmt_crumb: IDWriteTextFormat,
    /// 手势方向提示：「← 后退」这类小标签。
    fmt_label: IDWriteTextFormat,
    fmt_empty_title: IDWriteTextFormat,
    fmt_empty: IDWriteTextFormat,
    folder: ID2D1PathGeometry,
    /// 手势轨迹：圆头圆角，点再密也看不到接缝。
    stroke: ID2D1StrokeStyle,
}

impl Gpu {
    fn new() -> Result<Gpu> {
        unsafe {
            let mut d3d: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                Some(&mut ctx),
            )?;
            let d3d = d3d.unwrap();
            let ctx = ctx.unwrap();
            let dxgi: IDXGIDevice = d3d.cast()?;
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let device = factory.CreateDevice(&dxgi)?;
            let dcomp = DCompositionCreateDevice(&dxgi)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let module = GetModuleHandleW(None)?;
            let font = w!("Segoe UI Variable Text");
            let locale = w!("zh-CN");

            let fmt_title = dwrite.CreateTextFormat(
                font,
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.5,
                locale,
            )?;
            fmt_title.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            fmt_title.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            fmt_title.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK)?;
            set_trimming(&dwrite, &fmt_title)?;

            let fmt_count = dwrite.CreateTextFormat(
                font,
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                11.0,
                locale,
            )?;
            fmt_count.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            fmt_count.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;

            let fmt_crumb = dwrite.CreateTextFormat(
                font,
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.0,
                locale,
            )?;
            fmt_crumb.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            fmt_crumb.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            fmt_crumb.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            set_trimming(&dwrite, &fmt_crumb)?;

            let fmt_label = dwrite.CreateTextFormat(
                font,
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.0,
                locale,
            )?;
            fmt_label.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            fmt_label.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            fmt_label.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;

            let stroke = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES1 {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    ..Default::default()
                },
                None,
            )?;

            let fmt_empty_title = dwrite.CreateTextFormat(
                font,
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.5,
                locale,
            )?;
            fmt_empty_title.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            fmt_empty_title.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;

            let fmt_empty = dwrite.CreateTextFormat(
                font,
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.0,
                locale,
            )?;
            fmt_empty.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            fmt_empty.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;

            // 标题条左边的文件夹轮廓：20×20 viewBox 的 SVG 路径，圆角换成直角折线。
            let folder = factory.CreatePathGeometry()?;
            let sink = folder.Open()?;
            sink.BeginFigure(Vector2 { X: 3.8, Y: 5.4 }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(Vector2 { X: 6.9, Y: 5.4 });
            sink.AddLine(Vector2 { X: 8.4, Y: 7.0 });
            sink.AddLine(Vector2 { X: 16.2, Y: 7.0 });
            sink.AddLine(Vector2 { X: 16.2, Y: 15.2 });
            sink.AddLine(Vector2 { X: 3.8, Y: 15.2 });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;

            Ok(Gpu {
                _d3d: d3d,
                _ctx: ctx,
                device,
                factory,
                dcomp,
                dwrite,
                wic,
                instance: HINSTANCE(module.0),
                fmt_title,
                fmt_count,
                fmt_crumb,
                fmt_label,
                fmt_empty_title,
                fmt_empty,
                folder: folder.into(),
                stroke: stroke.into(),
            })
        }
    }
}

fn set_trimming(dwrite: &IDWriteFactory, format: &IDWriteTextFormat) -> Result<()> {
    let ellipsis = unsafe { dwrite.CreateEllipsisTrimmingSign(format)? };
    let trimming = DWRITE_TRIMMING {
        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
        delimiter: 0,
        delimiterCount: 0,
    };
    unsafe { format.SetTrimming(&trimming, Some(&ellipsis)) }
}

// ---------- 一个格子 ----------

struct ItemView {
    name: String,
    path: String,
    is_dir: bool,
    layout: Option<IDWriteTextLayout>,
    icon: Option<ID2D1Bitmap1>,
    tried: bool,
}

/// 标题条上的一截面包屑。第 0 段是绑定目录（用格子标题），后面是子目录。
struct NavSeg {
    path: String,
    layout: Option<IDWriteTextLayout>,
    /// 文字宽度；热区还要加上左右各 6px 的 padding。
    width: f32,
    x: f32,
}

struct Brushes {
    bg: ID2D1SolidColorBrush,
    header: ID2D1SolidColorBrush,
    text: ID2D1SolidColorBrush,
    muted: ID2D1SolidColorBrush,
    line: ID2D1SolidColorBrush,
    hover: ID2D1SolidColorBrush,
    /// 单击选中：比 hover 深一档，和网页版的 `--pick` 一致。
    pick: ID2D1SolidColorBrush,
    scroll: ID2D1SolidColorBrush,
    /// 手势轨迹：和 fence.html 的 `--gesture-line` / `--gesture-glow` 一致。
    gesture_line: ID2D1SolidColorBrush,
    gesture_glow: ID2D1SolidColorBrush,
    /// 方向提示条：深色底 + 白字（和网页版一样，不跟主题）。
    label_bg: ID2D1SolidColorBrush,
    label_text: ID2D1SolidColorBrush,
    /// 历史到头了：提示画淡一档。
    label_dim: ID2D1SolidColorBrush,
}

impl Brushes {
    fn new(dc: &ID2D1DeviceContext, dark: bool, opacity: u32) -> Result<Brushes> {
        let alpha = (opacity.clamp(30, 100) as f32 / 100.0).clamp(0.3, 1.0);
        let header_alpha = (alpha - 0.12).max(0.3);
        Ok(Brushes {
            bg: solid(dc, if dark { rgb(24, 24, 26, alpha) } else { rgb(250, 250, 251, alpha) })?,
            header: solid(
                dc,
                if dark { rgb(32, 32, 35, header_alpha) } else { rgb(242, 242, 244, header_alpha) },
            )?,
            text: solid(dc, if dark { rgb(236, 236, 236, 1.0) } else { rgb(13, 13, 13, 1.0) })?,
            muted: solid(dc, if dark { rgb(155, 155, 155, 1.0) } else { rgb(107, 107, 107, 1.0) })?,
            line: solid(dc, if dark { rgb(255, 255, 255, 0.10) } else { rgb(0, 0, 0, 0.09) })?,
            hover: solid(dc, if dark { rgb(255, 255, 255, 0.07) } else { rgb(0, 0, 0, 0.05) })?,
            pick: solid(dc, if dark { rgb(255, 255, 255, 0.14) } else { rgb(0, 0, 0, 0.10) })?,
            scroll: solid(
                dc,
                if dark {
                    rgb(255, 255, 255, 0.03 + alpha * 0.07)
                } else {
                    rgb(0, 0, 0, 0.03 + alpha * 0.07)
                },
            )?,
            gesture_line: solid(
                dc,
                if dark {
                    rgb(255, 255, 255, 0.62)
                } else {
                    rgb(13, 13, 13, 0.45)
                },
            )?,
            gesture_glow: solid(
                dc,
                if dark {
                    rgb(255, 255, 255, 0.14)
                } else {
                    rgb(13, 13, 13, 0.10)
                },
            )?,
            label_bg: solid(dc, rgb(32, 32, 34, 0.92))?,
            label_text: solid(dc, rgb(255, 255, 255, 1.0))?,
            label_dim: solid(dc, rgb(255, 255, 255, 0.45))?,
        })
    }
}

/// 左键按在条目上：拖出一段距离才真的发起 OLE 拖拽。
struct FileDrag {
    index: usize,
    origin: POINT,
}

/// 底部短提示：排版好的文字 + 到点消失的时刻。
struct Toast {
    layout: IDWriteTextLayout,
    text_w: f32,
}

/// 提示停留多久（毫秒）。
const TOAST_MS: u32 = 2600;
const TIMER_TOAST: usize = 2;

/// 边缘 / 四角的可拉区（逻辑像素）。卷起时只认左右边。
const RESIZE_EDGE: f32 = 5.0;
const RESIZE_CORNER: f32 = 10.0;

/// 拖放回调在 OLE 里跑，窗口过程才拿得到 `FenceWindow`。这里留一份够用的
/// 快照：当前目录 + 文件夹格子占的位置（逻辑像素），松手时据此选落点。
#[derive(Default)]
struct DropSnapshot {
    dir: String,
    scale: f32,
    /// `(x, y, w, h, path)`，只放文件夹。
    folders: Vec<(f32, f32, f32, f32, String)>,
}

impl DropSnapshot {
    fn folder_at(&self, x: f32, y: f32) -> Option<&str> {
        self.folders
            .iter()
            .find(|(left, top, width, height, _)| {
                x >= *left && x < left + width && y >= *top && y < top + height
            })
            .map(|(_, _, _, _, path)| path.as_str())
    }

    /// 一次拖放的落点：压在文件夹格子上就进那个文件夹，否则进当前目录。
    /// OLE 回调和自动化验证共用这一份口径（`x` / `y` 是客户区物理像素）。
    fn destination(&self, x: f32, y: f32) -> Option<String> {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        match self.folder_at(x / scale, y / scale) {
            Some(folder) => Some(folder.to_string()),
            None => (!self.dir.is_empty()).then(|| self.dir.clone()),
        }
    }
}

/// 右键手势的运行时状态（窗口逻辑坐标）。
struct Gesture {
    start_x: f32,
    start_y: f32,
    last_x: f32,
    last_y: f32,
    /// 已经判定成横向手势（超过起步距离且横向占优）。
    recognized: bool,
    /// 判定后往哪边走：true = 后退。
    back: bool,
    /// 轨迹采样点，只在显示轨迹时才收集。
    points: Vec<(f32, f32)>,
    /// 判定之后才抢鼠标捕获，普通右键不受影响。
    captured: bool,
}

/// 一次右键松手的结局：没划（照常弹菜单）/ 划了但不执行 / 要退或进。
enum GestureEnd {
    NotAGesture,
    Swallowed,
    Go(bool),
}

impl GestureEnd {
    fn go(self) -> Option<bool> {
        match self {
            GestureEnd::Go(back) => Some(back),
            _ => None,
        }
    }
}

struct FenceWindow {
    id: String,
    generation: u64,
    /// 现在显示的目录；空白处右键菜单用。
    path: String,
    /// 绑定的根目录：面包屑和「返回」不越出这里。
    root: String,
    /// 进到子目录时从根到当前目录的每一层；根目录时为空。
    crumbs: Vec<Crumb>,
    hwnd: HWND,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual,
    surface: IDCompositionSurface,
    dc: ID2D1DeviceContext,
    brushes: Brushes,
    dark: bool,
    opacity: u32,
    title: String,
    title_layout: Option<IDWriteTextLayout>,
    /// 面包屑的每一段（画出来的、可点的）。
    nav: Vec<NavSeg>,
    nav_sep: Option<IDWriteTextLayout>,
    nav_sep_w: f32,
    nav_more: Option<IDWriteTextLayout>,
    /// 路径太长、左边被省略时画一个「…」。
    nav_elided: bool,
    /// 鼠标压在哪一段面包屑上（0 起）。
    nav_hot: Option<usize>,
    count_layout: Option<IDWriteTextLayout>,
    empty_title_layout: Option<IDWriteTextLayout>,
    empty_layout: Option<IDWriteTextLayout>,
    items: Vec<ItemView>,
    name_format: Option<IDWriteTextFormat>,
    zoom: u32,
    rolled: bool,
    scroll: f32,
    hover: Option<usize>,
    /// 单击选中的条目（和网页版的 picked 一样，只是本地状态）。
    pick: Option<usize>,
    /// 左键按在「更多」按钮上：松手时再弹菜单。
    /// 在 `WM_LBUTTONDOWN` 里直接弹菜单会让按钮一直按着，菜单收不到后续鼠标输入。
    dots_down: bool,
    /// 右键手势设置（和快捷面板共用一份）。
    gestures: GestureSettings,
    /// 正在进行的右键手势。
    gesture: Option<Gesture>,
    /// 浏览历史里还能退 / 还能进的层数。
    back_count: usize,
    forward_count: usize,
    /// 手势方向提示的两段文字。
    gesture_back_layout: Option<IDWriteTextLayout>,
    gesture_forward_layout: Option<IDWriteTextLayout>,
    gesture_back_w: f32,
    gesture_forward_w: f32,
    /// 标题菜单：收纳盒、规则数量、当前排序。
    is_bucket: bool,
    rule_count: usize,
    sort_key: String,
    sort_dir: i32,
    /// 左键按在条目上还没松：拖出格子时用。
    file_drag: Option<FileDrag>,
    /// 当前那条短提示。
    toast: Option<Toast>,
    /// 拖放回调看的那份快照（当前目录 / 文件夹位置）。
    drop: Arc<Mutex<DropSnapshot>>,
    tracking_leave: bool,
    scale: f32,
    logical_w: f32,
    logical_h: f32,
    px_w: u32,
    px_h: u32,
    // 排版参数（逻辑像素），layout() 时按 zoom 算好。
    cell_w: f32,
    cell_h: f32,
    gap: f32,
    pad: f32,
    icon_size: f32,
    name_size: f32,
}

impl FenceWindow {
    fn new(gpu: &Gpu, frame: Frame) -> Result<FenceWindow> {
        let [x, y, _, _] = frame.rect;
        let mut consts = LayoutConsts::for_frame(&frame);
        let px_w = (consts.logical_w * 1.0).round().max(1.0) as u32;
        let mut px_h = (consts.logical_h).round().max(1.0) as u32;
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
                w!("HexBenchNativeFence"),
                &HSTRING::from(frame.title.as_str()),
                WS_POPUP,
                x,
                y,
                px_w as i32,
                px_h as i32,
                None,
                None,
                Some(gpu.instance),
                None,
            )?
        };
        let scale = {
            let dpi = unsafe { GetDpiForWindow(hwnd) } as f32;
            if dpi > 0.0 { dpi / 96.0 } else { 1.0 }
        };
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rect);
        }
        let actual_w = (rect.right - rect.left).max(1) as u32;
        let actual_h = (rect.bottom - rect.top).max(1) as u32;
        consts.logical_w = actual_w as f32 / scale;
        consts.logical_h = actual_h as f32 / scale;
        if frame.rolled {
            px_h = header_px(scale);
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    x,
                    y,
                    actual_w as i32,
                    px_h as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            consts.logical_h = px_h as f32 / scale;
        }
        // 建好之后任何一步失败都要把窗口销毁，否则会留下一个没有 userdata、
        // 也没有 surface 的空窗口。
        let content_frame = frame.clone();
        let built: Result<FenceWindow> = (|| {
            let target = unsafe { gpu.dcomp.CreateTargetForHwnd(hwnd, false)? };
            let visual = unsafe { gpu.dcomp.CreateVisual()? };
            let surface = unsafe {
                gpu.dcomp.CreateSurface(
                    actual_w,
                    px_h,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_ALPHA_MODE_PREMULTIPLIED,
                )?
            };
            unsafe {
                visual.SetContent(&surface)?;
                target.SetRoot(&visual)?;
                gpu.dcomp.Commit()?;
            }
            let dc = unsafe { gpu.device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)? };
            let brushes = Brushes::new(&dc, frame.dark, frame.opacity)?;
            Ok(FenceWindow {
                id: frame.id,
                generation: frame.generation,
                path: frame.path,
                root: frame.root,
                crumbs: frame.crumbs,
                hwnd,
                _target: target,
                visual,
                surface,
                dc,
                brushes,
                dark: frame.dark,
                opacity: frame.opacity,
                title: frame.title,
                title_layout: None,
                nav: Vec::new(),
                nav_sep: None,
                nav_sep_w: 8.0,
                nav_more: None,
                nav_elided: false,
                nav_hot: None,
                count_layout: None,
                empty_title_layout: None,
                empty_layout: None,
                items: Vec::new(),
                name_format: None,
                zoom: frame.zoom,
                rolled: frame.rolled,
                scroll: 0.0,
                hover: None,
                pick: None,
                dots_down: false,
                gestures: GestureSettings::default(),
                gesture: None,
                back_count: 0,
                forward_count: 0,
                gesture_back_layout: None,
                gesture_forward_layout: None,
                gesture_back_w: 0.0,
                gesture_forward_w: 0.0,
                is_bucket: frame.is_bucket,
                rule_count: frame.rule_count,
                sort_key: frame.sort_key,
                sort_dir: frame.sort_dir,
                file_drag: None,
                toast: None,
                drop: Arc::new(Mutex::new(DropSnapshot {
                    dir: String::new(),
                    scale: 1.0,
                    folders: Vec::new(),
                })),
                tracking_leave: false,
                scale,
                logical_w: consts.logical_w,
                logical_h: consts.logical_h,
                px_w: actual_w,
                px_h,
                cell_w: safe(consts.cell_w, CELL_W),
                cell_h: safe(consts.cell_h, CELL_H),
                gap: safe(consts.gap, CELL_GAP),
                pad: safe(consts.pad, CELL_PAD),
                icon_size: safe(consts.icon_size, ICON_BASE),
                name_size: safe(consts.name_size, 11.0),
            })
        })();
        if built.is_err() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        let mut window = built?;
        window.set_content(gpu, content_frame);
        Ok(window)
    }

    fn update(&mut self, gpu: &Gpu, frame: Frame) -> Result<()> {
        let mut size_changed = false;
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rect);
        }
        let current_w = (rect.right - rect.left).max(1) as u32;
        let current_h = (rect.bottom - rect.top).max(1) as u32;
        let scale = {
            let dpi = unsafe { GetDpiForWindow(self.hwnd) } as f32;
            if dpi > 0.0 { dpi / 96.0 } else { self.scale }
        };
        let target_h = if frame.rolled { header_px(scale) } else { frame.rect[3].max(1) as u32 };
        let target_w = frame.rect[2].max(1) as u32;
        // 松手时归一化换算回来可能差一两个像素；为这 1px 再挪一次会看到抖动。
        let origin = self.window_origin();
        let far = (origin.0 - frame.rect[0]).abs() > 2 || (origin.1 - frame.rect[1]).abs() > 2;
        if current_w != target_w || current_h != target_h || far {
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    frame.rect[0],
                    frame.rect[1],
                    target_w as i32,
                    target_h as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            size_changed = true;
        }
        if size_changed || (scale - self.scale).abs() > 0.01 {
            self.scale = scale;
            let mut rect = RECT::default();
            unsafe {
                let _ = GetClientRect(self.hwnd, &mut rect);
            }
            self.px_w = (rect.right - rect.left).max(1) as u32;
            self.px_h = (rect.bottom - rect.top).max(1) as u32;
            self.logical_w = self.px_w as f32 / scale;
            self.logical_h = self.px_h as f32 / scale;
            self.recreate_surface(gpu)?;
        }
        self.rolled = frame.rolled;
        let appearance_changed = self.dark != frame.dark || self.opacity != frame.opacity;
        if appearance_changed {
            self.dark = frame.dark;
            self.opacity = frame.opacity;
            self.brushes = Brushes::new(&self.dc, self.dark, self.opacity)?;
        }
        self.set_content(gpu, frame);
        self.render(gpu)
    }

    fn window_origin(&self) -> (i32, i32) {
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rect);
        }
        (rect.left, rect.top)
    }

    fn set_content(&mut self, gpu: &Gpu, frame: Frame) {
        let previous_path = std::mem::replace(&mut self.path, frame.path);
        let navigated = !previous_path.eq_ignore_ascii_case(&self.path);
        self.generation = frame.generation;
        self.title = frame.title;
        self.root = frame.root;
        self.crumbs = frame.crumbs;
        self.zoom = frame.zoom;
        self.back_count = frame.back_count;
        self.forward_count = frame.forward_count;
        self.is_bucket = frame.is_bucket;
        self.rule_count = frame.rule_count;
        self.sort_key = frame.sort_key;
        self.sort_dir = frame.sort_dir;
        let consts = LayoutConsts::for_logical(frame.zoom, self.logical_w, self.logical_h);
        self.cell_w = safe(consts.cell_w, CELL_W);
        self.cell_h = safe(consts.cell_h, CELL_H);
        self.gap = safe(consts.gap, CELL_GAP);
        self.pad = safe(consts.pad, CELL_PAD);
        self.icon_size = safe(consts.icon_size, ICON_BASE);
        self.name_size = safe(consts.name_size, 11.0);
        self.items = frame
            .items
            .into_iter()
            .map(|Item { name, path, is_dir }| ItemView {
                name,
                path,
                is_dir,
                layout: None,
                icon: None,
                tried: false,
            })
            .collect();
        self.rebuild_layout(gpu);
        if navigated {
            // 换了一层：上一层的选中 / 悬停 / 滚动位置都没有意义了。
            self.hover = None;
            self.pick = None;
            self.set_scroll(0.0);
            self.nav_hot = None;
        } else {
            self.hover = self.hover.filter(|index| *index < self.items.len());
            self.pick = self.pick.filter(|index| *index < self.items.len());
            // 条目变少 / 窗口变高之后滚动位置可能被夹小，快照要跟着更新。
            self.set_scroll(self.scroll);
        }
    }

    fn body_h(&self) -> f32 {
        (self.logical_h - HEADER_H).max(0.0)
    }

    fn columns(&self) -> usize {
        (((self.logical_w - 2.0 * self.pad + self.gap) / (self.cell_w + self.gap)).floor() as usize)
            .max(1)
    }

    fn max_scroll(&self) -> f32 {
        if self.rolled || self.items.is_empty() {
            return 0.0;
        }
        let cols = self.columns();
        let rows = self.items.len().div_ceil(cols);
        let content = rows as f32 * self.cell_h + rows.saturating_sub(1) as f32 * self.gap;
        (2.0 * self.pad + content - self.body_h()).max(0.0)
    }

    /// 标题条上的面包屑命中（热区含左右 6px 的 padding）。
    fn nav_at(&self, x: f32) -> Option<usize> {
        self.nav
            .iter()
            .position(|seg| x >= seg.x - 3.0 && x < seg.x + seg.width + 12.0)
    }

    /// 「返回」去哪儿：上一层面包屑；已经是最深一层就回绑定目录。
    fn back_path(&self) -> String {
        if self.crumbs.len() >= 2 {
            self.crumbs[self.crumbs.len() - 2].path.clone()
        } else {
            self.root.clone()
        }
    }

    fn tile_at(&self, x: f32, y: f32) -> Option<usize> {
        if self.rolled || y < HEADER_H {
            return None;
        }
        let cols = self.columns();
        let x0 = self.pad;
        let y0 = HEADER_H + self.pad - self.scroll;
        for (index, _) in self.items.iter().enumerate() {
            let col = (index % cols) as f32;
            let row = (index / cols) as f32;
            let tx = x0 + col * (self.cell_w + self.gap);
            let ty = y0 + row * (self.cell_h + self.gap);
            if x >= tx && x < tx + self.cell_w && y >= ty && y < ty + self.cell_h {
                return Some(index);
            }
        }
        None
    }

    /// 边缘 / 四角命中：返回 `edge` 方向串（`n` `sw` 这类），和网页版同义。
    /// 四角优先；卷起时只有左右边能拉。
    fn edge_at(&self, x: f32, y: f32) -> Option<&'static str> {
        let w = self.logical_w;
        let h = self.logical_h;
        let left = x < RESIZE_EDGE;
        let right = x > w - RESIZE_EDGE;
        let near_left = x < RESIZE_CORNER;
        let near_right = x > w - RESIZE_CORNER;
        if self.rolled {
            return match (left, right) {
                (true, _) => Some("w"),
                (_, true) => Some("e"),
                _ => None,
            };
        }
        let near_top = y < RESIZE_CORNER;
        let near_bottom = y > h - RESIZE_CORNER;
        match (near_left, near_right, near_top, near_bottom) {
            (true, _, true, _) => Some("nw"),
            (_, true, true, _) => Some("ne"),
            (true, _, _, true) => Some("sw"),
            (_, true, _, true) => Some("se"),
            (true, _, _, _) => Some("w"),
            (_, true, _, _) => Some("e"),
            (_, _, true, _) => Some("n"),
            (_, _, _, true) => Some("s"),
            _ => None,
        }
    }

    /// 弹一条底部短提示；同一时间只留最新那条，重新计时。
    fn show_toast(&mut self, gpu: &Gpu, text: String) {
        let Some(layout) = make_layout(&gpu.dwrite, &text, &gpu.fmt_label, 520.0, 22.0) else {
            return;
        };
        let text_w = layout_width(&layout).min(500.0);
        self.toast = Some(Toast { layout, text_w });
        unsafe {
            // 重设同一个 id 的定时器会覆盖旧的，不会叠出两条。
            let _ = SetTimer(Some(self.hwnd), TIMER_TOAST, TOAST_MS, None);
        }
        let _ = self.render(gpu);
    }

    /// 设置页改了手势：开关关掉时把进行中的手势也收掉。
    fn apply_gestures(&mut self, gpu: &Gpu, gestures: GestureSettings) {
        self.gestures = gestures;
        if !self.gestures.enabled {
            self.cancel_gesture();
        }
        let _ = self.render(gpu);
    }

    /// 收掉进行中的手势（设置关掉 / 鼠标捕获被抢走）。
    fn cancel_gesture(&mut self) {
        if let Some(gesture) = self.gesture.take() {
            if gesture.captured {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
        }
    }

    /// 右键按住移动：过了起步距离、横向占优才判成手势；之后按需画轨迹。
    fn move_gesture(&mut self, hwnd: HWND, x: f32, y: f32, gpu: &Gpu) {
        let Some(mut gesture) = self.gesture.take() else {
            return;
        };
        gesture.last_x = x;
        gesture.last_y = y;
        let dx = x - gesture.start_x;
        let dy = y - gesture.start_y;
        if !gesture.recognized {
            if (dx * dx + dy * dy).sqrt() < GESTURE_START {
                self.gesture = Some(gesture);
                return;
            }
            // 竖着划不算：右键上下拖更像是在选一行，不把人导航走。
            if dx.abs() < dy.abs() * 1.2 {
                self.gesture = Some(gesture);
                return;
            }
            gesture.recognized = true;
            if !gesture.captured {
                // 判定之后才抢捕获：划到格子外面也还能收尾，普通右键不受影响。
                unsafe {
                    SetCapture(hwnd);
                }
                gesture.captured = true;
            }
            if self.gestures.show_trail {
                gesture.points.push((gesture.start_x, gesture.start_y));
            }
        }
        gesture.back = gesture_back(dx, self.gestures.invert_direction);
        if self.gestures.show_trail {
            let far = gesture
                .points
                .last()
                .map(|(px, py)| ((x - px).powi(2) + (y - py).powi(2)).sqrt() >= 2.5)
                .unwrap_or(true);
            if far {
                gesture.points.push((x, y));
                // 划得再长也只留一段尾巴，别一直涨内存。
                if gesture.points.len() > 600 {
                    gesture.points.remove(1);
                }
            }
        }
        self.gesture = Some(gesture);
        let _ = self.render(gpu);
    }

    /// 松手：返回要不要退 / 进。判成手势但没划够、或者历史到头，也会吞掉菜单。
    fn finish_gesture(&mut self, x: f32) -> GestureEnd {
        let Some(gesture) = self.gesture.take() else {
            return GestureEnd::NotAGesture;
        };
        if gesture.captured {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        if !gesture.recognized {
            return GestureEnd::NotAGesture;
        }
        let dx = x - gesture.start_x;
        if dx.abs() < gesture_distance(self.gestures.trigger_distance) {
            return GestureEnd::Swallowed;
        }
        let back = gesture_back(dx, self.gestures.invert_direction);
        let available = if back {
            self.back_count
        } else {
            self.forward_count
        };
        if available == 0 {
            GestureEnd::Swallowed
        } else {
            GestureEnd::Go(back)
        }
    }

    fn rebuild_layout(&mut self, gpu: &Gpu) {
        let dwrite = &gpu.dwrite;
        let title_width = (self.logical_w - 34.0 - 90.0).max(20.0);
        self.title_layout = make_layout(dwrite, &self.title, &gpu.fmt_title, title_width, HEADER_H);
        let count = self.items.len().to_string();
        self.count_layout = make_layout(dwrite, &count, &gpu.fmt_count, 60.0, HEADER_H);

        // 进了子目录：标题位置换成面包屑（根标题 › 子目录 › …），每段可点。
        self.nav.clear();
        self.nav_elided = false;
        self.nav_hot = None;
        if self.crumbs.is_empty() {
            self.nav_sep = None;
            self.nav_more = None;
        } else {
            self.nav_sep = make_layout(dwrite, "›", &gpu.fmt_crumb, 16.0, HEADER_H);
            self.nav_sep_w = self.nav_sep.as_ref().map(layout_width).unwrap_or(8.0);
            self.nav_more = make_layout(dwrite, "…", &gpu.fmt_crumb, 16.0, HEADER_H);
            let labels = std::iter::once((self.title.clone(), self.root.clone())).chain(
                self.crumbs
                    .iter()
                    .map(|crumb| (crumb.name.clone(), crumb.path.clone())),
            );
            for (label, path) in labels {
                let layout = make_layout(dwrite, &label, &gpu.fmt_crumb, 160.0, HEADER_H);
                let width = layout.as_ref().map(layout_width).unwrap_or(0.0);
                self.nav.push(NavSeg {
                    path,
                    layout,
                    width,
                    x: 0.0,
                });
            }
            // 太长就从最外层开始省略，只留「…」；最后一段（当前目录）总在。
            let available = (self.logical_w - NAV_START - 104.0).max(60.0);
            loop {
                let mut x = NAV_START + if self.nav_elided { 16.0 } else { 0.0 };
                let count = self.nav.len();
                for (index, seg) in self.nav.iter_mut().enumerate() {
                    seg.x = x;
                    x += seg.width + 12.0;
                    if index + 1 < count {
                        x += self.nav_sep_w + 6.0;
                    }
                }
                if x - NAV_START <= available || self.nav.len() <= 1 {
                    break;
                }
                self.nav.remove(0);
                self.nav_elided = true;
            }
        }
        let empty_width = (self.logical_w - 36.0).max(20.0);
        self.empty_title_layout =
            make_layout(dwrite, "这里是空的", &gpu.fmt_empty_title, empty_width, 24.0);
        self.empty_layout = make_layout(
            dwrite,
            "把文件拖进来，或右键新建",
            &gpu.fmt_empty,
            empty_width,
            22.0,
        );
        self.name_format = None;
        if !self.items.is_empty() {
            let size = self.name_size;
            let format = unsafe {
                dwrite.CreateTextFormat(
                    w!("Segoe UI Variable Text"),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("zh-CN"),
                )
            }
            .ok();
            if let Some(format) = &format {
                unsafe {
                    let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                    let _ = format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
                    let _ = format.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK);
                }
                let _ = set_trimming(dwrite, format);
            }
            let width = (self.cell_w - 6.0).max(10.0);
            let height = (self.cell_h - (NAME_TOP_PAD + self.icon_size + 4.0) - 4.0).max(12.0);
            if let Some(format) = &format {
                for item in &mut self.items {
                    item.layout = make_layout(dwrite, &item.name, format, width, height);
                }
            }
            self.name_format = format;
        }
        // 手势方向提示：两段文字预先排好，划动时只量一下宽度。
        self.gesture_back_layout = make_layout(dwrite, "← 后退", &gpu.fmt_label, 120.0, 22.0);
        self.gesture_forward_layout = make_layout(dwrite, "→ 前进", &gpu.fmt_label, 120.0, 22.0);
        self.gesture_back_w = self
            .gesture_back_layout
            .as_ref()
            .map(layout_width)
            .unwrap_or(48.0);
        self.gesture_forward_w = self
            .gesture_forward_layout
            .as_ref()
            .map(layout_width)
            .unwrap_or(48.0);
        self.sync_drop_snapshot();
    }

    /// 改滚动位置，同时重建拖放快照。
    ///
    /// 快照里存的是文件夹此刻在窗口里的位置，而 OLE 的拖放回调跑在窗口过程
    /// 外面、只能看这份拷贝：滚动后不重建，高亮的是新位置上的文件夹，松手却
    /// 按上一屏的位置挑落点，文件就掉进别的文件夹里了。
    fn set_scroll(&mut self, value: f32) {
        self.scroll = value.clamp(0.0, self.max_scroll());
        self.sync_drop_snapshot();
    }

    /// 把当前目录和文件夹格子的位置写进拖放快照：OLE 回调在窗口过程外面跑，
    /// 只能看这份拷贝。
    fn sync_drop_snapshot(&self) {
        let mut folders = Vec::new();
        if !self.rolled {
            let cols = self.columns();
            let x0 = self.pad;
            let y0 = HEADER_H + self.pad - self.scroll;
            for (index, item) in self.items.iter().enumerate() {
                if !item.is_dir {
                    continue;
                }
                let col = (index % cols) as f32;
                let row = (index / cols) as f32;
                folders.push((
                    x0 + col * (self.cell_w + self.gap),
                    y0 + row * (self.cell_h + self.gap),
                    self.cell_w,
                    self.cell_h,
                    item.path.clone(),
                ));
            }
        }
        if let Ok(mut snapshot) = self.drop.lock() {
            snapshot.dir = self.path.clone();
            snapshot.scale = self.scale;
            snapshot.folders = folders;
        }
    }

    fn recreate_surface(&mut self, gpu: &Gpu) -> Result<()> {
        let surface = unsafe {
            gpu.dcomp.CreateSurface(
                self.px_w,
                self.px_h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
            )?
        };
        unsafe {
            self.visual.SetContent(&surface)?;
            self.surface = surface;
            gpu.dcomp.Commit()?;
        }
        // 尺寸变了文字换行也跟着变。
        self.rebuild_layout(gpu);
        Ok(())
    }

    fn render(&mut self, gpu: &Gpu) -> Result<()> {
        let update = RECT {
            left: 0,
            top: 0,
            right: self.px_w as i32,
            bottom: self.px_h as i32,
        };
        let mut offset = POINT::default();
        let surface: IDXGISurface = unsafe { self.surface.BeginDraw(Some(&update), &mut offset)? };
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: self.scale * 96.0,
            dpiY: self.scale * 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: ManuallyDrop::new(None),
        };
        // 坑 2：每个 BeginDraw 都要重新包一层可以当 target 的位图。
        let bitmap = unsafe { self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))? };
        let base = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: offset.x as f32 / self.scale,
            M32: offset.y as f32 / self.scale,
        };
        unsafe {
            self.dc.SetTarget(&bitmap);
            self.dc.SetDpi(self.scale * 96.0, self.scale * 96.0);
            self.dc.BeginDraw();
            // 坑 1：DComp 返回的是图集里的一块，要按 offset 平移。
            self.dc.SetTransform(&base);
            let transparent = rgb(0, 0, 0, 0.0);
            self.dc.Clear(Some(&transparent));
            self.paint(gpu, &base);
            self.dc.EndDraw(None, None)?;
            self.dc.SetTarget(None::<&ID2D1Image>);
        }
        unsafe {
            self.surface.EndDraw()?;
            gpu.dcomp.Commit()?;
        }
        Ok(())
    }

    fn paint(&self, gpu: &Gpu, base: &Matrix3x2) {
        let w = self.logical_w;
        let h = self.logical_h;
        let dc = &self.dc;
        let card = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: 0.5,
                top: 0.5,
                right: w - 0.5,
                bottom: h - 0.5,
            },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let head_h = HEADER_H.min(h);
        let head = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: 0.5,
                top: 0.5,
                right: w - 0.5,
                bottom: head_h + RADIUS,
            },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        unsafe {
            dc.FillRoundedRectangle(&card, &self.brushes.bg);
            // 表头背景要画到 HEADER_H + RADIUS，把圆角矩形下面两个角压成直边；
            // 但多出来的这一段不能露进正文，否则分割线下面会多出一条 8px 的
            // 表头色带（展开的格子顶部看着就像「多了一点 UI」）。裁到表头高度。
            let head_clip = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: w,
                bottom: head_h,
            };
            dc.PushAxisAlignedClip(&head_clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            dc.FillRoundedRectangle(&head, &self.brushes.header);
            dc.PopAxisAlignedClip();
            if !self.rolled {
                let line = D2D_RECT_F {
                    left: 0.0,
                    top: HEADER_H - 1.0,
                    right: w,
                    bottom: HEADER_H,
                };
                dc.FillRectangle(&line, &self.brushes.line);
            }
            dc.DrawRoundedRectangle(&card, &self.brushes.line, 1.0, None);
        }

        if self.crumbs.is_empty() {
            // 根目录：文件夹轮廓 + 标题。
            let glyph = *base
                * Matrix3x2 {
                    M11: 17.0 / 20.0,
                    M12: 0.0,
                    M21: 0.0,
                    M22: 17.0 / 20.0,
                    M31: 9.5,
                    M32: 11.5,
                };
            unsafe {
                dc.SetTransform(&glyph);
                dc.DrawGeometry(&gpu.folder, &self.brushes.muted, 1.1, None);
                dc.SetTransform(base);
            }
            if let Some(layout) = &self.title_layout {
                unsafe {
                    dc.DrawTextLayout(
                        Vector2 { X: 34.0, Y: 0.0 },
                        layout,
                        &self.brushes.text,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
            }
        } else {
            // 子目录：左边「‹」返回，右边面包屑，最后一段是当前目录。
            unsafe {
                let mid = HEADER_H / 2.0;
                dc.DrawLine(
                    Vector2 { X: 21.0, Y: mid - 5.0 },
                    Vector2 { X: 15.0, Y: mid },
                    &self.brushes.muted,
                    1.3,
                    None,
                );
                dc.DrawLine(
                    Vector2 { X: 15.0, Y: mid },
                    Vector2 { X: 21.0, Y: mid + 5.0 },
                    &self.brushes.muted,
                    1.3,
                    None,
                );
            }
            if let Some(layout) = &self.nav_more {
                if self.nav_elided {
                    unsafe {
                        dc.DrawTextLayout(
                            Vector2 { X: NAV_START, Y: 0.0 },
                            layout,
                            &self.brushes.muted,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                        );
                    }
                }
            }
            for (index, seg) in self.nav.iter().enumerate() {
                if self.nav_hot == Some(index) {
                    let highlight = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: seg.x - 2.0,
                            top: 9.0,
                            right: seg.x + seg.width + 10.0,
                            bottom: HEADER_H - 9.0,
                        },
                        radiusX: 6.0,
                        radiusY: 6.0,
                    };
                    unsafe {
                        dc.FillRoundedRectangle(&highlight, &self.brushes.hover);
                    }
                }
                if let Some(sep) = &self.nav_sep {
                    if index > 0 {
                        unsafe {
                            dc.DrawTextLayout(
                                Vector2 {
                                    X: seg.x - self.nav_sep_w - 5.0,
                                    Y: 0.0,
                                },
                                sep,
                                &self.brushes.muted,
                                D2D1_DRAW_TEXT_OPTIONS_NONE,
                            );
                        }
                    }
                }
                if let Some(layout) = &seg.layout {
                    let last = index + 1 == self.nav.len();
                    unsafe {
                        dc.DrawTextLayout(
                            Vector2 {
                                X: seg.x + 6.0,
                                Y: 0.0,
                            },
                            layout,
                            if last { &self.brushes.text } else { &self.brushes.muted },
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                        );
                    }
                }
            }
        }
        if let Some(layout) = &self.count_layout {
            unsafe {
                dc.DrawTextLayout(
                    Vector2 { X: w - 100.0, Y: 0.0 },
                    layout,
                    &self.brushes.muted,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }
        // 右侧「更多」的三颗点（入口在原生版里还没接，先画出和网页一致的样子）。
        for index in 0..3 {
            let dot = D2D1_ELLIPSE {
                point: Vector2 {
                    X: w - 25.2 + index as f32 * 4.2,
                    Y: HEADER_H / 2.0,
                },
                radiusX: 1.15,
                radiusY: 1.15,
            };
            unsafe {
                dc.FillEllipse(&dot, &self.brushes.text);
            }
        }

        if self.rolled || h <= HEADER_H {
            return;
        }

        let body = D2D_RECT_F {
            left: 0.0,
            top: HEADER_H,
            right: w,
            bottom: h,
        };
        unsafe {
            dc.PushAxisAlignedClip(&body, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        if self.items.is_empty() {
            if let Some(layout) = &self.empty_title_layout {
                unsafe {
                    dc.DrawTextLayout(
                        Vector2 { X: 18.0, Y: HEADER_H + self.body_h() / 2.0 - 24.0 },
                        layout,
                        &self.brushes.text,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
            }
            if let Some(layout) = &self.empty_layout {
                unsafe {
                    dc.DrawTextLayout(
                        Vector2 { X: 18.0, Y: HEADER_H + self.body_h() / 2.0 },
                        layout,
                        &self.brushes.muted,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
            }
        } else {
            let cols = self.columns();
            let y0 = HEADER_H + self.pad - self.scroll;
            for (index, item) in self.items.iter().enumerate() {
                let col = (index % cols) as f32;
                let row = (index / cols) as f32;
                let tx = self.pad + col * (self.cell_w + self.gap);
                let ty = y0 + row * (self.cell_h + self.gap);
                if ty + self.cell_h < HEADER_H || ty > h {
                    continue;
                }
                let clip = D2D_RECT_F {
                    left: tx,
                    top: ty,
                    right: tx + self.cell_w,
                    bottom: ty + self.cell_h,
                };
                unsafe {
                    dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                }
                if self.hover == Some(index) || self.pick == Some(index) {
                    let highlight = D2D1_ROUNDED_RECT {
                        rect: clip,
                        radiusX: TILE_RADIUS,
                        radiusY: TILE_RADIUS,
                    };
                    // 选中比悬停深一档；两者叠加时按选中画。
                    let brush = if self.pick == Some(index) {
                        &self.brushes.pick
                    } else {
                        &self.brushes.hover
                    };
                    unsafe {
                        dc.FillRoundedRectangle(&highlight, brush);
                    }
                }
                let icon_left = tx + (self.cell_w - self.icon_size) / 2.0;
                let icon_top = ty + NAME_TOP_PAD;
                if let Some(icon) = &item.icon {
                    let dest = D2D_RECT_F {
                        left: icon_left,
                        top: icon_top,
                        right: icon_left + self.icon_size,
                        bottom: icon_top + self.icon_size,
                    };
                    unsafe {
                        dc.DrawBitmap(
                            icon,
                            Some(&dest),
                            1.0,
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            None,
                            None,
                        );
                    }
                } else {
                    // 图标还没抽出来：文件夹画轮廓、文件画一个点，和网页版的占位一致。
                    let placeholder = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: icon_left + 2.0,
                            top: icon_top + 1.0,
                            right: icon_left + self.icon_size - 2.0,
                            bottom: icon_top + self.icon_size - 3.0,
                        },
                        radiusX: 3.0,
                        radiusY: 3.0,
                    };
                    unsafe {
                        dc.DrawRoundedRectangle(&placeholder, &self.brushes.muted, 1.1, None);
                    }
                }
                if let Some(layout) = &item.layout {
                    unsafe {
                        dc.DrawTextLayout(
                            Vector2 {
                                X: tx + 3.0,
                                Y: icon_top + self.icon_size + 4.0,
                            },
                            layout,
                            &self.brushes.text,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                        );
                    }
                }
                unsafe {
                    dc.PopAxisAlignedClip();
                }
            }
        }
        unsafe {
            dc.PopAxisAlignedClip();
        }

        // 滚动条：网页版是 6px 宽、半透明的 thumb，这里照画。
        let max = self.max_scroll();
        if max > 0.0 {
            let body_h = self.body_h();
            let content = body_h + max;
            let thumb_h = (body_h * (body_h / content)).clamp(24.0, body_h);
            let travel = body_h - thumb_h;
            let top = HEADER_H + (self.scroll / max) * travel;
            let thumb = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: w - 6.0,
                    top,
                    right: w,
                    bottom: top + thumb_h,
                },
                radiusX: 3.0,
                radiusY: 3.0,
            };
            unsafe {
                dc.FillRoundedRectangle(&thumb, &self.brushes.scroll);
            }
        }

        // 右键手势：轨迹压在内容上面，方向提示跟着光标走。
        if let Some(gesture) = &self.gesture {
            if gesture.recognized && self.gestures.show_trail {
                if !gesture.points.is_empty() {
                    if let Ok(path) = gesture_path(&gpu.factory, &gesture.points) {
                        unsafe {
                            dc.DrawGeometry(&path, &self.brushes.gesture_glow, 9.0, &gpu.stroke);
                            dc.DrawGeometry(
                                &path,
                                &self.brushes.gesture_line,
                                3.0,
                                &gpu.stroke,
                            );
                        }
                    }
                }
                let (layout, text_w) = if gesture.back {
                    (&self.gesture_back_layout, self.gesture_back_w)
                } else {
                    (&self.gesture_forward_layout, self.gesture_forward_w)
                };
                if let Some(layout) = layout {
                    let box_w = text_w + 20.0;
                    let box_h = 24.0;
                    // 贴着光标放，但别跑出卡片。
                    let left = (gesture.last_x + 18.0).min(w - box_w - 8.0).max(8.0);
                    let top = (gesture.last_y + 16.0).min(h - box_h - 8.0).max(8.0);
                    let label = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left,
                            top,
                            right: left + box_w,
                            bottom: top + box_h,
                        },
                        radiusX: 7.0,
                        radiusY: 7.0,
                    };
                    unsafe {
                        dc.FillRoundedRectangle(&label, &self.brushes.label_bg);
                    }
                    let available = if gesture.back {
                        self.back_count
                    } else {
                        self.forward_count
                    };
                    // 到头了就把提示画淡一档，和网页版一样。
                    let brush = if available > 0 {
                        &self.brushes.label_text
                    } else {
                        &self.brushes.label_dim
                    };
                    unsafe {
                        dc.DrawTextLayout(
                            Vector2 {
                                X: left + 10.0,
                                Y: top,
                            },
                            layout,
                            brush,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                        );
                    }
                }
            }
        }
        // 底部短提示（拖入失败 / 整理结果）：压在内容上面，几秒后自己消失。
        if let Some(toast) = &self.toast {
            let box_w = (toast.text_w + 24.0).min(w - 16.0);
            let box_h = 28.0;
            let left = ((w - box_w) / 2.0).max(8.0);
            let top = (h - box_h - 14.0).max(8.0);
            let pill = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left,
                    top,
                    right: left + box_w,
                    bottom: top + box_h,
                },
                radiusX: 8.0,
                radiusY: 8.0,
            };
            unsafe {
                dc.FillRoundedRectangle(&pill, &self.brushes.label_bg);
                dc.DrawTextLayout(
                    Vector2 {
                        X: left + 12.0,
                        Y: top,
                    },
                    &toast.layout,
                    &self.brushes.label_text,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }
    }
}

struct LayoutConsts {
    logical_w: f32,
    logical_h: f32,
    cell_w: f32,
    cell_h: f32,
    gap: f32,
    pad: f32,
    icon_size: f32,
    name_size: f32,
}

impl LayoutConsts {
    fn for_frame(frame: &Frame) -> Self {
        Self::for_logical(frame.zoom, frame.rect[2] as f32, frame.rect[3] as f32)
    }

    fn for_logical(zoom: u32, logical_w: f32, logical_h: f32) -> Self {
        let z = (zoom.max(100) as f32) / 100.0;
        let round = |value: f32| value.round();
        Self {
            logical_w,
            logical_h,
            cell_w: round(CELL_W * z).max(44.0),
            cell_h: round(CELL_H * z).max(44.0),
            gap: round(CELL_GAP * z).max(4.0),
            pad: round(CELL_PAD * z).max(4.0),
            icon_size: round(ICON_BASE * z).max(16.0),
            name_size: round(11.0 * z).max(10.0),
        }
    }
}

fn safe(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 { value } else { fallback }
}

fn header_px(scale: f32) -> u32 {
    (HEADER_H * scale).round().max(1.0) as u32
}

// ---------- 窗口过程 ----------

// ---------- 交互：打开 / 右键菜单 / 标题菜单 ----------

/// 标题条右侧「更多」按钮的命中宽度（和 paint 里的三颗点对齐）。
const DOTS_W: f32 = 34.0;

const MENU_OPEN: u32 = 1;
const MENU_SORT_NAME: u32 = 2;
const MENU_SORT_MODIFIED: u32 = 3;
const MENU_HIDE: u32 = 4;
const MENU_DELETE: u32 = 5;
const MENU_CHANGE_FOLDER: u32 = 6;
const MENU_RUN_RULES: u32 = 7;
const MENU_RENAME: u32 = 8;

/// 单行 `EDIT` 的两条消息：`Win32_UI_Controls` 这个 feature 没开，值写在本地。
const EM_SETSEL_MSG: u32 = 0x00B1;
const EM_SETMARGINS_MSG: u32 = 0x00D3;
/// 左右各留 3px，字不要贴着边框。
const EDIT_MARGINS: isize = 0x0003_0003;

fn cursor_screen() -> POINT {
    let mut point = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut point);
    }
    point
}

fn open_item(path: &str) {
    if let Err(error) = hexbench_core::sys::open_path(std::path::Path::new(path)) {
        log::warn!("native fence: 打开失败 {path}: {error}");
    }
}

/// 标题条「更多」菜单。返回 0 表示取消，否则是上面几个 MENU_* id。
///
/// 用系统菜单而不是自绘：Beta 阶段先保证可用、键盘和失焦关闭都由系统处理；
/// 等格子的自绘控件齐了再换成和网页一致的样子。
fn show_fence_menu(hwnd: HWND, fence: &FenceWindow, x: i32, y: i32) -> u32 {
    with_menu_focus(hwnd, || unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return 0;
        };
        let item = |id: u32, label: &str| {
            let text = HSTRING::from(label);
            let _ = AppendMenuW(menu, MF_STRING, id as usize, &text);
        };
        // 和网页版一样：「重命名格子」在最上面。
        item(MENU_RENAME, "重命名格子");
        // 收纳盒的幕后目录是应用自己的：不给「更换文件夹」，改成按规则收一次。
        if fence.is_bucket {
            item(MENU_OPEN, "打开收纳目录");
            if fence.rule_count > 0 {
                item(MENU_RUN_RULES, "立即整理");
            }
        } else {
            item(MENU_CHANGE_FOLDER, "更换文件夹…");
            item(MENU_OPEN, "在资源管理器中打开");
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        // 和网页版一样：当前排序键后面标方向，再点一次反转。
        let sort_label = |label: &str, key: &str| {
            if fence.sort_key != key {
                return label.to_string();
            }
            let arrow = if fence.sort_dir < 0 { "↓" } else { "↑" };
            format!("{label} {arrow}")
        };
        item(MENU_SORT_NAME, &sort_label("按名称排序", "name"));
        item(MENU_SORT_MODIFIED, &sort_label("按修改时间排序", "modified"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        item(MENU_HIDE, "隐藏格子");
        item(MENU_DELETE, "删除格子");
        let command = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD.0 | TPM_NONOTIFY.0 | TPM_LEFTALIGN.0 | TPM_TOPALIGN.0,
            x,
            y,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        command.0 as u32
    })
}

/// 菜单期间临时让格子能成为前台。
///
/// `TrackPopupMenuEx` 要有输入焦点才能「点外面关掉 / 按 Esc 关掉」；`WS_EX_NOACTIVATE`
/// 的窗口永远拿不到焦点，菜单会变成一个只能等菜单项被点中的「幽灵」。菜单关掉后
/// 立刻恢复不抢焦点，并插回桌面图标上方。
fn with_menu_focus<T>(hwnd: HWND, show: impl FnOnce() -> T) -> T {
    let raw = hwnd.0 as isize;
    // 菜单关掉后把前台还给原来那个窗口；格子本身不该留下键盘焦点。
    let previous = unsafe { GetForegroundWindow() };
    hexbench_core::sys::set_no_activate(raw, false);
    hexbench_core::sys::force_foreground(raw);
    let result = show();
    hexbench_core::sys::set_no_activate(raw, true);
    hexbench_core::sys::anchor_above_desktop(raw);
    // 只在还停在格子上时把前台还回去。菜单项打开了别的窗口（资源管理器 / 照片）
    // 或者用户点别的窗口关掉菜单时，那个窗口才是他想要的，不要抢回来。
    let now = unsafe { GetForegroundWindow() };
    if !previous.is_invalid() && previous.0 != hwnd.0 && (now.0 == hwnd.0 || now.is_invalid()) {
        unsafe {
            let _ = SetForegroundWindow(previous);
        }
    }
    result
}

// ---------- 重命名：格子标题 / 条目 ----------

/// 改的是谁：格子标题，还是某个文件 / 文件夹的名字。
enum RenameTarget {
    Fence {
        id: String,
    },
    Item {
        id: String,
        path: String,
    },
}

/// 改名弹层的状态。弹层、输入框都挂同一个指针，窗口销毁时释放。
struct RenameEdit {
    popup: HWND,
    edit: HWND,
    /// 弹层关掉后把前台还给原来那个窗口。
    previous: HWND,
    target: RenameTarget,
    sink: EventSink,
    /// 打开时的文字：一个字没改就什么都不做。
    original: String,
    font: HFONT,
    brush: HBRUSH,
    bg: COLORREF,
    fg: COLORREF,
    old_proc: WNDPROC,
    /// 已经在收窗口：别在 `WM_KILLFOCUS` 里再来一次。
    closing: bool,
}

fn window_text(hwnd: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
}

/// 结束改名：`commit` 为真就发事件，否则丢弃。
///
/// `restore_foreground` 只在「用户主动回车 / Esc」时为真；点到别处取消时那个窗口
/// 才是用户想要的，不要抢回来。
unsafe fn finish_rename(ptr: *mut RenameEdit, commit: bool, restore_foreground: bool) {
    unsafe {
        let state = &mut *ptr;
        if state.closing {
            return;
        }
        state.closing = true;
        if commit {
            let text = window_text(state.edit);
            let name = text.trim();
            if !name.is_empty() && name != state.original {
                match &state.target {
                    RenameTarget::Fence { id } => (state.sink)(Event::Rename {
                        id: id.clone(),
                        title: name.to_string(),
                    }),
                    RenameTarget::Item { id, path } => (state.sink)(Event::RenameItem {
                        id: id.clone(),
                        path: path.clone(),
                        name: name.to_string(),
                    }),
                }
            }
        }
        let popup = state.popup;
        let previous = state.previous;
        // `DestroyWindow` 会连带销毁子窗口，结构体在弹层的 `WM_NCDESTROY` 里释放。
        let _ = DestroyWindow(popup);
        if restore_foreground && !previous.is_invalid() {
            let _ = SetForegroundWindow(previous);
        }
    }
}

/// 输入框的子类过程：回车提交、Esc 取消、点到别处取消。
unsafe extern "system" fn rename_edit_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RenameEdit;
        if !ptr.is_null() {
            match msg {
                WM_KEYDOWN => {
                    let key = wparam.0 as u16;
                    if key == VK_RETURN.0 {
                        finish_rename(ptr, true, true);
                        return LRESULT(0);
                    }
                    if key == VK_ESCAPE.0 {
                        finish_rename(ptr, false, true);
                        return LRESULT(0);
                    }
                }
                WM_CHAR if wparam.0 == 13 || wparam.0 == 27 => {
                    // 回车 / Esc 上面已经处理过，别让 EDIT 再「叮」一声。
                    return LRESULT(0);
                }
                WM_KILLFOCUS => {
                    // 先让 EDIT 自己收尾（藏光标），再关弹层。
                    let result = if (*ptr).old_proc.is_some() {
                        CallWindowProcW((*ptr).old_proc, hwnd, msg, wparam, lparam)
                    } else {
                        LRESULT(0)
                    };
                    finish_rename(ptr, false, false);
                    return result;
                }
                _ => {}
            }
            if (*ptr).old_proc.is_some() {
                return CallWindowProcW((*ptr).old_proc, hwnd, msg, wparam, lparam);
            }
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// 弹层的窗口过程：背景和输入框配色跟卡片一致，销毁时释放状态。
unsafe extern "system" fn rename_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RenameEdit;
        if !ptr.is_null() {
            match msg {
                WM_ERASEBKGND => {
                    let dc = HDC(wparam.0 as *mut core::ffi::c_void);
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let _ = FillRect(dc, &rect, (*ptr).brush);
                    return LRESULT(1);
                }
                WM_CTLCOLOREDIT => {
                    let dc = HDC(wparam.0 as *mut core::ffi::c_void);
                    let _ = SetTextColor(dc, (*ptr).fg);
                    let _ = SetBkColor(dc, (*ptr).bg);
                    return LRESULT((*ptr).brush.0 as isize);
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let state = Box::from_raw(ptr);
                    if !state.font.0.is_null() {
                        let _ = DeleteObject(HGDIOBJ(state.font.0));
                    }
                    if !state.brush.0.is_null() {
                        let _ = DeleteObject(HGDIOBJ(state.brush.0));
                    }
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// 在指定位置弹一个单行改名框。输入法、选区、剪贴板都交给系统 `EDIT`。
#[allow(clippy::too_many_arguments)]
fn show_rename_box(
    owner: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    text: &str,
    select_to: Option<usize>,
    dark: bool,
    target: RenameTarget,
    sink: &EventSink,
) {
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return;
        };
        let Ok(popup) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("HexBenchNativeFenceRename"),
            w!(""),
            WS_POPUP | WS_BORDER | WS_CLIPSIBLINGS,
            x,
            y,
            width,
            height,
            Some(owner),
            None,
            Some(module.into()),
            None,
        ) else {
            return;
        };
        let mut client = RECT::default();
        let _ = GetClientRect(popup, &mut client);
        let Ok(edit) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            &HSTRING::from(text),
            WINDOW_STYLE(
                WS_CHILD.0 | WS_VISIBLE.0 | ES_LEFT as u32 | ES_AUTOHSCROLL as u32,
            ),
            2,
            2,
            (client.right - 4).max(24),
            (client.bottom - 4).max(16),
            Some(popup),
            None,
            Some(module.into()),
            None,
        ) else {
            let _ = DestroyWindow(popup);
            return;
        };
        let font = CreateFontW(
            -(height * 3 / 5).max(12),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
            w!("Microsoft YaHei UI"),
        );
        let (bg, fg) = if dark {
            (COLORREF(0x0032_3235), COLORREF(0x00F0_F0F0))
        } else {
            (COLORREF(0x00FF_FFFF), COLORREF(0x001E_1E1E))
        };
        let brush = CreateSolidBrush(bg);
        // 弹层出现前的那个窗口：回车 / Esc 之后把前台还回去。
        let previous = GetForegroundWindow();
        let state = Box::new(RenameEdit {
            popup,
            edit,
            previous,
            target,
            sink: Arc::clone(sink),
            original: text.to_string(),
            font,
            brush,
            bg,
            fg,
            old_proc: None,
            closing: false,
        });
        let ptr = Box::into_raw(state);
        SetWindowLongPtrW(popup, GWLP_USERDATA, ptr as isize);
        SetWindowLongPtrW(edit, GWLP_USERDATA, ptr as isize);
        let old: WNDPROC = std::mem::transmute(GetWindowLongPtrW(edit, GWLP_WNDPROC));
        (*ptr).old_proc = old;
        SetWindowLongPtrW(edit, GWLP_WNDPROC, rename_edit_proc as *const () as usize as isize);
        let _ = SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        let _ = SendMessageW(
            edit,
            EM_SETMARGINS_MSG,
            Some(WPARAM(0x0003)),
            Some(LPARAM(EDIT_MARGINS)),
        );
        // 默认全选；文件把扩展名之前的部分选中，和资源管理器的改名一致。
        let end = select_to.map(|value| value as isize).unwrap_or(-1);
        let _ = SendMessageW(edit, EM_SETSEL_MSG, Some(WPARAM(0)), Some(LPARAM(end)));
        let _ = ShowWindow(popup, SW_SHOWNA);
        hexbench_core::sys::force_foreground(popup.0 as isize);
        let _ = SetFocus(Some(edit));
    }
}

/// 标题菜单「重命名格子」：输入框压在标题条上。
fn start_rename_title(sink: &EventSink, fence: &FenceWindow) {
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(fence.hwnd, &mut rect) }.is_err() {
        return;
    }
    let scale = fence.scale.max(0.5);
    let height = (24.0 * scale).round().max(20.0) as i32;
    let left = rect.left + (NAV_START * scale).round() as i32;
    // 右边让开三颗点，别把输入框盖到按钮上。
    let right = rect.left + ((fence.logical_w - DOTS_W - 6.0) * scale).round() as i32;
    let width = (right - left).max((150.0 * scale) as i32);
    let top = rect.top + (((HEADER_H * scale) - height as f32) / 2.0).round() as i32;
    show_rename_box(
        fence.hwnd,
        left,
        top,
        width,
        height,
        &fence.title,
        None,
        fence.dark,
        RenameTarget::Fence {
            id: fence.id.clone(),
        },
        sink,
    );
}

/// 条目的「重命名」：输入框出现在菜单原来的位置。
fn start_rename_item(
    sink: &EventSink,
    fence: &FenceWindow,
    path: &str,
    is_dir: bool,
    x: i32,
    y: i32,
) {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let lower = name.to_lowercase();
    // 和网页版 `displayName` 一致：快捷方式不显示后缀。
    let prefill = if lower.ends_with(".lnk") || lower.ends_with(".url") {
        &name[..name.len() - 4]
    } else {
        name
    };
    let scale = fence.scale.max(0.5);
    let height = (24.0 * scale).round().max(20.0) as i32;
    let width = (220.0 * scale).round().max(180.0) as i32;
    let select_to = if is_dir {
        None
    } else {
        match prefill.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => Some(stem.encode_utf16().count()),
            _ => None,
        }
    };
    show_rename_box(
        fence.hwnd,
        x,
        y + 4,
        width,
        height,
        prefill,
        select_to,
        fence.dark,
        RenameTarget::Item {
            id: fence.id.clone(),
            path: path.to_string(),
        },
        sink,
    );
}

/// 条目的系统右键菜单（外观、语言、扩展项都和资源管理器一致）。
///
/// 菜单里的「重命名」是 `hexbench_core` 自己插的一项（资源管理器的视图层才有改名），
/// 选中时返回路径，由调用方弹原生输入框。
fn show_item_menu(hwnd: HWND, path: &str, x: i32, y: i32) -> Option<String> {
    let result = with_menu_focus(hwnd, || {
        hexbench_core::sys::show_shell_context_menu(
            std::path::Path::new(path),
            x,
            y,
            hwnd.0 as isize,
            || {},
        )
    });
    match result {
        Ok(Some("rename")) => Some(path.to_string()),
        Ok(_) => None,
        Err(error) => {
            log::warn!("native fence: 右键菜单失败: {error}");
            None
        }
    }
}

/// 空白处的文件夹背景菜单（新建 / 粘贴 / 属性）。
fn show_background_menu(hwnd: HWND, path: &str, x: i32, y: i32) {
    let dir = std::path::Path::new(path);
    if !dir.is_dir() {
        return;
    }
    let can_paste = hexbench_core::shell_ops::clipboard_get_files().is_some();
    let result = with_menu_focus(hwnd, || {
        hexbench_core::sys::show_folder_background_menu(dir, x, y, hwnd.0 as isize, can_paste)
    });
    match result {
        Ok(Some("paste")) => {
            if let Err(error) = hexbench_core::shell_ops::paste_into(dir) {
                log::warn!("native fence: 粘贴失败: {error}");
            }
        }
        Ok(_) => {}
        Err(error) => log::warn!("native fence: 背景菜单失败: {error}"),
    }
}

unsafe extern "system" fn fence_wndproc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut FenceWindow;
    if raw.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, w, l) };
    }
    let engine = ENGINE.with(|cell| cell.get());
    if engine.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, w, l) };
    }
    match msg {
        WM_MOUSEMOVE => {
            unsafe {
                let fence = &mut *raw;
                let x = ((l.0 & 0xffff) as u16 as i16) as f32 / fence.scale;
                let y = (((l.0 >> 16) & 0xffff) as u16 as i16) as f32 / fence.scale;
                if fence.gesture.is_some() {
                    if w.0 & MK_RBUTTON != 0 {
                        fence.move_gesture(hwnd, x, y, &(*engine).gpu);
                    } else {
                        // 松开事件丢了（比如在格子外面松手）：按已经划到的位置收尾。
                        if let Some(back) = fence.finish_gesture(x).go() {
                            ((*engine).sink)(if back {
                                Event::Back { id: fence.id.clone() }
                            } else {
                                Event::Forward { id: fence.id.clone() }
                            });
                        }
                        let _ = fence.render(&(*engine).gpu);
                    }
                    return LRESULT(0);
                }
                // 在窗口外松手拿不到 WM_LBUTTONUP：按键状态自己清。
                if w.0 & MK_LBUTTON == 0 {
                    fence.file_drag = None;
                }
                if let Some(pending) = fence.file_drag.take() {
                    let mut now = POINT::default();
                    let moved = GetCursorPos(&mut now).is_ok()
                        && (now.x - pending.origin.x).abs() + (now.y - pending.origin.y).abs()
                            >= FILE_DRAG_START;
                    if !moved {
                        // 还没拖起来：继续按着，等下一次移动。
                        fence.file_drag = Some(pending);
                    } else if let Some(item) = fence.items.get(pending.index) {
                        let path = std::path::PathBuf::from(&item.path);
                        // DoDragDrop 要和自己窗口同一条线程的鼠标捕获：这里是渲染线程，
                        // 落点（自己的格子 / 别的程序）都由 OLE 处理。
                        SetCapture(hwnd);
                        match hexbench_core::sys::start_file_drag(&[path]) {
                            Ok(effect) => log::info!("native fence: 拖出 effect={effect}"),
                            Err(error) => log::warn!("native fence: 拖出失败: {error}"),
                        }
                        let fence = &mut *raw;
                        fence.hover = None;
                        let _ = fence.render(&(*engine).gpu);
                    }
                } else {
                    if !fence.tracking_leave {
                        let mut track = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut track);
                        fence.tracking_leave = true;
                    }
                    let hot = if y < HEADER_H { fence.nav_at(x) } else { None };
                    let next = if y < HEADER_H {
                        None
                    } else {
                        fence.tile_at(x, y)
                    };
                    if hot != fence.nav_hot || next != fence.hover {
                        fence.nav_hot = hot;
                        fence.hover = next;
                        let _ = fence.render(&(*engine).gpu);
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            unsafe {
                let fence = &mut *raw;
                fence.tracking_leave = false;
                let hot = fence.nav_hot.take().is_some();
                if fence.hover.take().is_some() || hot {
                    let _ = fence.render(&(*engine).gpu);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            unsafe {
                let fence = &mut *raw;
                let x = ((l.0 & 0xffff) as u16 as i16) as f32 / fence.scale;
                let y = ((((l.0 >> 16) & 0xffff) as u16 as i16) as f32) / fence.scale;
                // 先认边角：拉边缩放不弹菜单、不动标题。
                if let Some(edge) = fence.edge_at(x, y) {
                    ((*engine).sink)(Event::BeginResize {
                        id: fence.id.clone(),
                        hwnd: hwnd.0 as isize,
                        edge: edge.to_string(),
                        scale: fence.scale,
                    });
                    SetCapture(hwnd);
                    return LRESULT(0);
                }
                if y < HEADER_H {
                    // 右侧三颗点是「更多」菜单，其余标题区起拖。
                    if x >= fence.logical_w - DOTS_W {
                        // 松手再弹：在 WM_LBUTTONDOWN 里弹菜单时左键还按着，
                        // 菜单的模态循环会把之后的鼠标输入当成同一次拖拽，点不动。
                        fence.dots_down = true;
                        fence.hover = None;
                        let _ = fence.render(&(*engine).gpu);
                        return LRESULT(0);
                    }
                    // 进了子目录：左边的「‹」回上一层，面包屑的每一段都能跳。
                    if !fence.crumbs.is_empty() {
                        let target = if x < BACK_W {
                            Some(fence.back_path())
                        } else {
                            fence.nav_at(x).and_then(|index| {
                                fence.nav.get(index).map(|seg| seg.path.clone())
                            })
                        };
                        if let Some(path) = target {
                            ((*engine).sink)(Event::Navigate {
                                id: fence.id.clone(),
                                path,
                            });
                            return LRESULT(0);
                        }
                    }
                    // 吸附、参考线、落盘都在后端那套循环里（和网页版共用）。
                    ((*engine).sink)(Event::BeginMove {
                        id: fence.id.clone(),
                        hwnd: hwnd.0 as isize,
                        scale: fence.scale,
                    });
                    fence.hover = None;
                    let _ = fence.render(&(*engine).gpu);
                } else {
                    // 单击选中；点空白处清掉选中，和网页版一致。
                    let next = fence.tile_at(x, y);
                    if next != fence.pick {
                        fence.pick = next;
                        let _ = fence.render(&(*engine).gpu);
                    }
                    // 按在条目上：拖出一段距离就发起文件拖拽（拖出格子 / 拖进子文件夹）。
                    let mut origin = POINT::default();
                    if GetCursorPos(&mut origin).is_ok() {
                        fence.file_drag = next.map(|index| FileDrag { index, origin });
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            unsafe {
                let fence = &mut *raw;
                let x = ((l.0 & 0xffff) as u16 as i16) as f32 / fence.scale;
                let y = ((((l.0 >> 16) & 0xffff) as u16 as i16) as f32) / fence.scale;
                if y < HEADER_H {
                    // 双击标题条卷起 / 展开；「更多」按钮和面包屑除外。
                    let on_crumb = !fence.crumbs.is_empty()
                        && (x < BACK_W || fence.nav_at(x).is_some());
                    if x < fence.logical_w - DOTS_W && !on_crumb {
                        ((*engine).sink)(Event::Rolled {
                            id: fence.id.clone(),
                            rolled: !fence.rolled,
                        });
                    }
                } else if let Some(index) = fence.tile_at(x, y) {
                    if let Some(item) = fence.items.get(index) {
                        if item.is_dir {
                            // 和网页版一致：文件夹在格子里进去，文件才交给系统打开。
                            ((*engine).sink)(Event::Navigate {
                                id: fence.id.clone(),
                                path: item.path.clone(),
                            });
                        } else {
                            open_item(&item.path);
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_FENCE_DRAG => {
            unsafe {
                let fence = &mut *raw;
                let packed = l.0;
                let next = if packed < 0 {
                    None
                } else {
                    let x = ((packed & 0xffff) as u16 as i16) as f32 / fence.scale;
                    let y = ((((packed >> 16) & 0xffff) as u16 as i16) as f32) / fence.scale;
                    fence.tile_at(x, y)
                };
                if next != fence.hover {
                    fence.hover = next;
                    let _ = fence.render(&(*engine).gpu);
                }
            }
            LRESULT(0)
        }
        WM_FENCE_MENU => {
            unsafe {
                let fence = &mut *raw;
                let cursor = cursor_screen();
                let command = show_fence_menu(hwnd, fence, cursor.x, cursor.y);
                match command {
                    MENU_RENAME => start_rename_title(&(*engine).sink, fence),
                    MENU_OPEN => open_item(&fence.path),
                    MENU_CHANGE_FOLDER => ((*engine).sink)(Event::ChangeFolder {
                        id: fence.id.clone(),
                    }),
                    MENU_RUN_RULES => {
                        ((*engine).sink)(Event::RunRules { id: fence.id.clone() })
                    }
                    MENU_SORT_NAME => ((*engine).sink)(Event::Sort {
                        id: fence.id.clone(),
                        key: "name".to_string(),
                    }),
                    MENU_SORT_MODIFIED => ((*engine).sink)(Event::Sort {
                        id: fence.id.clone(),
                        key: "modified".to_string(),
                    }),
                    MENU_HIDE => ((*engine).sink)(Event::Hide { id: fence.id.clone() }),
                    MENU_DELETE => ((*engine).sink)(Event::Delete { id: fence.id.clone() }),
                    _ => {}
                }
                // 菜单开着时收不到 WM_MOUSEMOVE，关掉后按当前位置重算悬停。
                let mut now = POINT::default();
                let mut rect = RECT::default();
                if GetCursorPos(&mut now).is_ok() && GetWindowRect(hwnd, &mut rect).is_ok() {
                    let x = (now.x - rect.left) as f32 / fence.scale;
                    let y = (now.y - rect.top) as f32 / fence.scale;
                    fence.hover = fence.tile_at(x, y);
                    fence.nav_hot = if y < HEADER_H {
                        fence.nav_at(x)
                    } else {
                        None
                    };
                }
                let _ = fence.render(&(*engine).gpu);
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            unsafe {
                let fence = &mut *raw;
                let x = ((l.0 & 0xffff) as u16 as i16) as f32 / fence.scale;
                let y = ((((l.0 >> 16) & 0xffff) as u16 as i16) as f32) / fence.scale;
                // 只记起点，不拦菜单：没划动时右键照常弹。
                fence.gesture = if fence.gestures.enabled && !fence.rolled && y >= HEADER_H {
                    Some(Gesture {
                        start_x: x,
                        start_y: y,
                        last_x: x,
                        last_y: y,
                        recognized: false,
                        back: false,
                        points: Vec::new(),
                        captured: false,
                    })
                } else {
                    None
                };
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            unsafe {
                let fence = &mut *raw;
                if let Some(gesture) = fence.gesture.take() {
                    if gesture.recognized {
                        let _ = fence.render(&(*engine).gpu);
                    }
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            unsafe {
                let fence = &mut *raw;
                let x = ((l.0 & 0xffff) as u16 as i16) as f32 / fence.scale;
                let y = ((((l.0 >> 16) & 0xffff) as u16 as i16) as f32) / fence.scale;
                // 划过的右键是手势：这次不弹菜单。
                match fence.finish_gesture(x) {
                    GestureEnd::NotAGesture => {}
                    GestureEnd::Swallowed => {
                        let _ = fence.render(&(*engine).gpu);
                        return LRESULT(0);
                    }
                    GestureEnd::Go(back) => {
                        let _ = fence.render(&(*engine).gpu);
                        ((*engine).sink)(if back {
                            Event::Back {
                                id: fence.id.clone(),
                            }
                        } else {
                            Event::Forward {
                                id: fence.id.clone(),
                            }
                        });
                        return LRESULT(0);
                    }
                }
                // 标题条上不弹菜单（和网页版一致）；空白处弹文件夹背景菜单。
                if y >= HEADER_H && !fence.rolled {
                    let cursor = cursor_screen();
                    let target = fence.tile_at(x, y);
                    if let Some(index) = target {
                        if fence.pick != Some(index) {
                            fence.pick = Some(index);
                            let _ = fence.render(&(*engine).gpu);
                        }
                    }
                    // 菜单必须在窗口自己的线程上弹：TrackPopupMenuEx 要求调用
                    // 线程掌握鼠标捕获，换一条线程会立刻返回 0。菜单自己会跑
                    // 模态消息循环，滚轮钩子照常被回调，只是这一格暂时不重绘。
                    if let Some(index) = target {
                        let picked = fence
                            .items
                            .get(index)
                            .map(|item| (item.path.clone(), item.is_dir));
                        if let Some((path, is_dir)) = picked {
                            if let Some(rename) = show_item_menu(hwnd, &path, cursor.x, cursor.y) {
                                start_rename_item(
                                    &(*engine).sink,
                                    fence,
                                    &rename,
                                    is_dir,
                                    cursor.x,
                                    cursor.y,
                                );
                            }
                        }
                    } else if !fence.path.is_empty() {
                        show_background_menu(hwnd, &fence.path, cursor.x, cursor.y);
                    }
                    // 菜单里可能新建 / 粘贴 / 删除了东西：先要一格新数据，
                    // 再在稍后补几次——新建文件夹是先建后改名的异步操作。
                    ((*engine).sink)(Event::Refresh { id: fence.id.clone() });
                    let sink = Arc::clone(&(*engine).sink);
                    let id = fence.id.clone();
                    thread::spawn(move || {
                        for delay in [300u64, 900] {
                            thread::sleep(std::time::Duration::from_millis(delay));
                            sink(Event::Refresh { id: id.clone() });
                        }
                    });
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            unsafe {
                let fence = &mut *raw;
                if std::mem::take(&mut fence.dots_down) {
                    let x = ((l.0 & 0xffff) as u16 as i16) as f32 / fence.scale;
                    let y = (((l.0 >> 16) & 0xffff) as u16 as i16) as f32 / fence.scale;
                    if y < HEADER_H && x >= fence.logical_w - DOTS_W {
                        let _ = PostMessageW(Some(hwnd), WM_FENCE_MENU, WPARAM(0), LPARAM(0));
                    }
                    return LRESULT(0);
                }
                // 按着没拖起来的单击到此结束；真拖起来的已经交给主线程了。
                fence.file_drag = None;
                let _ = ReleaseCapture();
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            unsafe {
                let fence = &*raw;
                if (l.0 & 0xffff) as u32 == HTCLIENT {
                    let mut pt = POINT::default();
                    if GetCursorPos(&mut pt).is_ok() && ScreenToClient(hwnd, &mut pt).as_bool() {
                        let x = pt.x as f32 / fence.scale;
                        let y = pt.y as f32 / fence.scale;
                        if let Some(edge) = fence.edge_at(x, y) {
                            let name = match edge {
                                "n" | "s" => IDC_SIZENS,
                                "e" | "w" => IDC_SIZEWE,
                                "nw" | "se" => IDC_SIZENWSE,
                                _ => IDC_SIZENESW,
                            };
                            if let Ok(cursor) = LoadCursorW(None, name) {
                                SetCursor(Some(cursor));
                            }
                            return LRESULT(1);
                        }
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, w, l) }
        }
        WM_SIZE => {
            // 后端那套移动 / 缩放循环直接 SetWindowPos；surface 跟着新尺寸重建。
            unsafe {
                let fence = &mut *raw;
                let px_w = (l.0 & 0xffff) as u32;
                let px_h = ((l.0 >> 16) & 0xffff) as u32;
                if px_w == 0 || px_h == 0 {
                    return LRESULT(0);
                }
                if px_w == fence.px_w && px_h == fence.px_h {
                    return LRESULT(0);
                }
                fence.px_w = px_w;
                fence.px_h = px_h;
                fence.logical_w = px_w as f32 / fence.scale;
                fence.logical_h = px_h as f32 / fence.scale;
                if fence.recreate_surface(&(*engine).gpu).is_ok() {
                    let _ = fence.render(&(*engine).gpu);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            unsafe {
                let fence = &mut *raw;
                let notch = (((w.0 >> 16) & 0xffff) as u16 as i16) as f32;
                let max = fence.max_scroll();
                if max > 0.0 {
                    fence.set_scroll(fence.scroll - notch / 120.0 * 100.0);
                    fence.hover = None;
                    let _ = fence.render(&(*engine).gpu);
                }
            }
            LRESULT(0)
        }
        WM_TIMER if w.0 == TIMER_TOAST => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_TOAST);
                let fence = &mut *raw;
                if fence.toast.take().is_some() {
                    let _ = fence.render(&(*engine).gpu);
                }
            }
            LRESULT(0)
        }
        WM_TIMER if w.0 == TIMER_FIRST_FRAME => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_FIRST_FRAME);
                let _ = (*raw).render(&(*engine).gpu);
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            unsafe {
                let suggested = l.0 as *const RECT;
                if !suggested.is_null() {
                    let rect = *suggested;
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                let fence = &mut *raw;
                let dpi = ((w.0 >> 16) & 0xffff) as f32;
                fence.scale = if dpi > 0.0 { dpi / 96.0 } else { fence.scale };
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                fence.px_w = (client.right - client.left).max(1) as u32;
                fence.px_h = (client.bottom - client.top).max(1) as u32;
                fence.logical_w = fence.px_w as f32 / fence.scale;
                fence.logical_h = fence.px_h as f32 / fence.scale;
                let _ = fence.recreate_surface(&(*engine).gpu);
                let _ = fence.render(&(*engine).gpu);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCDESTROY => {
            unsafe {
                let was = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if was != 0 {
                    let fence = Box::from_raw(was as *mut FenceWindow);
                    let id = fence.id.clone();
                    drop(fence);
                    let engine = &mut *engine;
                    engine.windows.retain(|entry| entry.id != id);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, w, l) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

/// 只认「光标正好压在我们的格子上」的滚轮，吃掉这一条消息避免前台窗口跟着滚；
/// 其余一概放行。钩子回调要快，这里只改滚动位置 + 画一帧。
unsafe extern "system" fn mouse_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 && w.0 as u32 == WM_MOUSEWHEEL {
        let engine = ENGINE.with(|cell| cell.get());
        if !engine.is_null() {
            let info = unsafe { &*(l.0 as *const MSLLHOOKSTRUCT) };
            let hwnd = unsafe { WindowFromPoint(info.pt) };
            if !hwnd.is_invalid() {
                let engine = unsafe { &mut *engine };
                if let Some(entry) = engine.windows.iter().find(|entry| entry.hwnd == hwnd) {
                    let fence = unsafe { &mut *entry.ptr };
                    let notch = (((info.mouseData >> 16) & 0xffff) as u16 as i16) as f32;
                    let max = fence.max_scroll();
                    if max > 0.0 {
                        fence.set_scroll(fence.scroll - notch / 120.0 * 100.0);
                        fence.hover = None;
                        let _ = fence.render(&engine.gpu);
                        return LRESULT(1);
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, w, l) }
}

unsafe extern "system" fn notify_wndproc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

fn register_classes() -> Result<()> {
    unsafe {
        let module = GetModuleHandleW(None)?;
        let classes: [(
            PCWSTR,
            unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
            bool,
        ); 3] = [
            (w!("HexBenchNativeFence"), fence_wndproc, true),
            (w!("HexBenchNativeFenceNotify"), notify_wndproc, false),
            // 改名弹层：带边框的小窗口 + 系统 EDIT，输入法由控件自己处理。
            (w!("HexBenchNativeFenceRename"), rename_wndproc, false),
        ];
        for (name, proc_, double_click) in classes {
            let mut class: WNDCLASSW = std::mem::zeroed();
            class.lpfnWndProc = Some(proc_);
            // 只有格子窗口要双击消息（CS_DBLCLKS），通知窗口不需要。
            if double_click {
                class.style = CS_DBLCLKS;
            }
            class.hInstance = HINSTANCE(module.0);
            class.lpszClassName = name;
            class.hCursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
            if RegisterClassW(&class) == 0 {
                let error = windows::core::Error::from_win32();
                // 窗口类在进程里只注册一次。设置里关掉 Beta 再打开时渲染线程
                // 会重启，类还留在进程里，第二次注册必然报「类已存在」——这
                // 不是错误，直接用现成的那个类。
                if error.code().0 as u32 != 0x8007_0582 {
                    return Err(error);
                }
            }
        }
    }
    Ok(())
}

// ---------- 图标 ----------

fn load_icon(gpu: &Gpu, dc: &ID2D1DeviceContext, file: &std::path::Path) -> Option<ID2D1Bitmap1> {
    let wide: Vec<u16> = file
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let decoder = gpu
            .wic
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let converter = gpu.wic.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
        dc.CreateBitmapFromWicBitmap(&converter, None).ok()
    }
}

// ---------- 小工具 ----------

fn make_layout(
    dwrite: &IDWriteFactory,
    text: &str,
    format: &IDWriteTextFormat,
    width: f32,
    height: f32,
) -> Option<IDWriteTextLayout> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe { dwrite.CreateTextLayout(&wide, format, width, height) }.ok()
}

/// 布局实际画出来有多宽（带省略号之后），排版面包屑用。
fn layout_width(layout: &IDWriteTextLayout) -> f32 {
    let mut metrics = DWRITE_TEXT_METRICS::default();
    // SAFETY: 布局和调用方在同一个线程上创建、持有。
    unsafe {
        let _ = layout.GetMetrics(&mut metrics);
    }
    metrics.widthIncludingTrailingWhitespace
}

fn rgb(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

fn solid(dc: &ID2D1DeviceContext, color: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush> {
    unsafe { dc.CreateSolidColorBrush(&color, None) }
}
