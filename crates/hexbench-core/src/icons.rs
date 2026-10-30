//! File icons for the panel.
//!
//! 图标一律问 Shell 要：它手里有和资源管理器完全一致的图标（含快捷方式的
//! 自定义图标、`.exe` 内嵌图标、文件夹的 `desktop.ini` 图标）。
//!
//! 两个关键点：
//!
//! 1. **缓存按完整路径**，不是按扩展名。按扩展名缓存会让所有 `.exe` 共用
//!    第一个 exe 的图标、所有 `.lnk` 共用一张——看起来就是「图标显示不对」。
//! 2. **取 JUMBO（96px）而不是大图标（32px）**：面板里按 40px 显示，32px
//!    放大后发虚，96px 缩下来是锐的，高 DPI 下也不糊。
//!
//! 路径 + 修改时间一起做键，文件被替换后会自动重新取图标。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use windows_sys::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    DIB_RGB_COLORS, HGDIOBJ,
};
use windows_sys::Win32::UI::Shell::{
    SHGetFileInfoW, SHGetImageList, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SYSICONINDEX,
    SHIL_EXTRALARGE, SHIL_JUMBO,
};
use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

thread_local! {
    /// COM 是按线程初始化的：每个线程第一次要图标前调一次。
    static COM_READY: std::cell::OnceCell<()> = const { std::cell::OnceCell::new() };
    /// 系统图标列表（`IImageList`）不是线程安全的，但按线程各存一份就没事：
    /// 取图可以在多个线程上并行，不必再全局排队。
    static EXTRA_LARGE: std::cell::RefCell<Option<ImageList>> = const { std::cell::RefCell::new(None) };
    static JUMBO: std::cell::RefCell<Option<ImageList>> = const { std::cell::RefCell::new(None) };
}

fn system_icon(which: u32, index: i32) -> Option<*mut std::ffi::c_void> {
    let pick = |slot: &mut Option<ImageList>| -> Option<*mut std::ffi::c_void> {
        if slot.is_none() {
            *slot = ImageList::system(which);
        }
        slot.as_ref()?.icon(index)
    };
    if which == SHIL_JUMBO {
        JUMBO.with(|cell| pick(&mut cell.borrow_mut()))
    } else {
        EXTRA_LARGE.with(|cell| pick(&mut cell.borrow_mut()))
    }
}

fn ensure_com() {
    COM_READY.with(|cell| {
        cell.get_or_init(|| {
            // SAFETY: 只初始化当前线程的 COM 公寓，失败也不影响后面的兜底路径。
            unsafe {
                let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
            }
        });
    });
}

/// 想拿到的图标边长。EXTRALARGE 是 48px：面板按 40px 显示，这个尺寸够锐，
/// 而 JUMBO(256px) 每张要上百毫秒，一屏几十项就会卡住面板。
const PREFERRED_ICON_PX: i32 = 48;

/// 实际编码进 PNG 的边长。源图标更大时缩到这个尺寸，避免 base64 过大。
const OUTPUT_ICON_PX: i32 = 64;

/// 缓存上限：超过就整体清空（简单、够用，面板一次也就几百项）。
const CACHE_LIMIT: usize = 4096;

/// `ILD_TRANSPARENT`：让 `IImageList::GetIcon` 走带 alpha 的绘制路径。
const ILD_TRANSPARENT: u32 = 0x0000_0001;

/// `{46EB5926-582E-4017-9FDF-E8998DAA0950}`
const IID_IIMAGELIST: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x46EB5926,
    data2: 0x582E,
    data3: 0x4017,
    data4: [0x9F, 0xDF, 0xE8, 0x99, 0x8D, 0xAA, 0x09, 0x50],
};

/// `IImageList` 只用到 `GetIcon`，所以前面的槽位按顺序占位即可。
#[repr(C)]
struct ImageListVtbl {
    query_interface: *const std::ffi::c_void,
    add_ref: *const std::ffi::c_void,
    release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
    add: *const std::ffi::c_void,
    replace_icon: *const std::ffi::c_void,
    set_overlay_image: *const std::ffi::c_void,
    replace: *const std::ffi::c_void,
    add_masked: *const std::ffi::c_void,
    draw: *const std::ffi::c_void,
    remove: *const std::ffi::c_void,
    get_icon: unsafe extern "system" fn(
        *mut std::ffi::c_void,
        i32,
        u32,
        *mut *mut std::ffi::c_void,
    ) -> i32,
}

struct ImageList(*mut std::ffi::c_void);

impl ImageList {
    /// 系统大图标列表。拿不到就返回 `None`，调用方退回 32px 的路径。
    fn system(which: u32) -> Option<Self> {
        // SAFETY: SHGetImageList 只写回一个接口指针；IID 与接口匹配。
        unsafe {
            let mut raw: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = SHGetImageList(which as i32, &IID_IIMAGELIST, &mut raw);
            if hr < 0 || raw.is_null() {
                return None;
            }
            Some(Self(raw))
        }
    }

    fn icon(&self, index: i32) -> Option<*mut std::ffi::c_void> {
        // SAFETY: self.0 是有效的 IImageList；GetIcon 写入一个新的 HICON。
        unsafe {
            let vtbl = *(self.0 as *mut *const ImageListVtbl);
            let mut handle: *mut std::ffi::c_void = std::ptr::null_mut();
            let hr = ((*vtbl).get_icon)(self.0, index, ILD_TRANSPARENT, &mut handle);
            if hr < 0 || handle.is_null() {
                None
            } else {
                Some(handle)
            }
        }
    }
}

impl Drop for ImageList {
    fn drop(&mut self) {
        // SAFETY: 对应的 Release 槽位是 IUnknown 的第三个方法。
        unsafe {
            let vtbl = *(self.0 as *mut *const ImageListVtbl);
            ((*vtbl).release)(self.0);
        }
    }
}

/// 取不到图标的结果只记一小会儿：Shell 偶尔会抽风一次，永久记住的话这个
/// 文件就再也不会出图了（之前 `.none` 落盘就是这个毛病）。
const MISS_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// 每项：文件修改时间 + 图标缓存键（`None` 表示这次没取到）+ 失败时间。
struct CacheEntry {
    stamp: u64,
    key: Option<String>,
    failed_at: Option<std::time::Instant>,
}

type IconCache = HashMap<PathBuf, CacheEntry>;

static CACHE: Mutex<Option<IconCache>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// 磁盘缓存：进程重启、切回目录都不用再问 Shell
// ---------------------------------------------------------------------------

/// 缓存总体积上限；超过就按最久没用过的删到低水位。
const DISK_BUDGET: u64 = 200 * 1024 * 1024;
const DISK_TARGET: u64 = 150 * 1024 * 1024;

pub fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    let dir = PathBuf::from(base).join("io.github.hexbench").join("thumbs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// 缓存文件名：路径（不分大小写）+ 修改时间做 FNV-1a，稳定且够短。
fn disk_key(path: &Path, stamp: u64) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |byte: u8| {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for byte in path.to_string_lossy().to_lowercase().bytes() {
        mix(byte);
    }
    for byte in stamp.to_le_bytes() {
        mix(byte);
    }
    format!("{hash:016x}")
}

/// 缓存文件就叫 `<16 位十六进制>.png`。前端拿到的也是这个键，
/// 再用 `hbicon://<键>` 让 WebView 直接从磁盘取，不走 base64。
pub fn cache_file(key: &str) -> Option<PathBuf> {
    if key.len() != 16 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(cache_dir()?.join(format!("{key}.png")))
}

/// 只清理一次：按修改时间从旧到新删到低水位。
fn prune_once(dir: &Path) {
    static PRUNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    PRUNED.get_or_init(|| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                if !meta.is_file() {
                    return None;
                }
                // 旧版本把「没取到图标」写成 `.none` 落盘，等于永久生效。
                // 那些文件没有用了，顺手清掉。
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) == Some("none") {
                    let _ = std::fs::remove_file(&path);
                    return None;
                }
                Some((meta.modified().ok()?, meta.len(), path))
            })
            .collect();
        let total: u64 = files.iter().map(|(_, size, _)| size).sum();
        if total <= DISK_BUDGET {
            return;
        }
        files.sort_by_key(|(time, _, _)| *time);
        let mut left = total;
        for (_, size, path) in files {
            if left <= DISK_TARGET {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                left = left.saturating_sub(size);
            }
        }
        log::info!("icons: 缩略图缓存清理完成，剩余 {} MB", left / 1024 / 1024);
    });
}

/// 把 PNG 落到缓存目录，返回是否成功。先写临时文件再改名，别的线程/协议
/// 读到的永远是完整的图。
fn write_icon(key: &str, png: &[u8]) -> bool {
    let Some(dir) = cache_dir() else {
        return false;
    };
    prune_once(&dir);
    let Some(target) = cache_file(key) else {
        return false;
    };
    let temp = dir.join(format!(
        "{key}.{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.subsec_nanos())
            .unwrap_or(0)
    ));
    if std::fs::write(&temp, png).is_err() {
        let _ = std::fs::remove_file(&temp);
        return false;
    }
    if std::fs::rename(&temp, &target).is_err() {
        let _ = std::fs::remove_file(&temp);
        return false;
    }
    true
}

fn remember(path: &Path, stamp: u64, key: Option<String>) {
    if let Ok(mut guard) = CACHE.lock() {
        let cache = guard.get_or_insert_with(HashMap::new);
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(
            path.to_path_buf(),
            CacheEntry {
                stamp,
                failed_at: key.is_none().then(std::time::Instant::now),
                key,
            },
        );
    }
}

fn cached_entry(path: &Path, stamp: u64) -> Option<Option<String>> {
    let guard = CACHE.lock().ok()?;
    let entry = guard.as_ref()?.get(path)?;
    if entry.stamp != stamp {
        return None;
    }
    match &entry.key {
        Some(key) => Some(Some(key.clone())),
        None => {
            // 失败结果只压一小会儿，过了就重新问 Shell。
            let expired = entry
                .failed_at
                .is_none_or(|at| at.elapsed() >= MISS_TTL);
            if expired {
                None
            } else {
                Some(None)
            }
        }
    }
}

/// 只看缓存（内存 + 磁盘），不问 Shell。
///
/// 面板首屏要的就是这个：命中的立刻出图，没命中的丢给后台慢慢抽。
/// 返回 `Some(None)` 表示「刚试过、没取到」，`None` 表示还没试过。
pub fn cached_icon_key(path: &Path) -> Option<Option<String>> {
    let stamp = modified_stamp(path);
    if let Some(cached) = cached_entry(path, stamp) {
        return Some(cached);
    }

    let key = disk_key(path, stamp);
    let file = cache_file(&key)?;
    if file.metadata().map(|meta| meta.len() > 0).unwrap_or(false) {
        // 更新访问时间，清理时按「最久没用」淘汰。
        let _ = std::fs::File::open(&file)
            .and_then(|handle| handle.set_modified(std::time::SystemTime::now()));
        remember(path, stamp, Some(key.clone()));
        Some(Some(key))
    } else {
        None
    }
}

/// 取图标的缓存键。键就是缓存文件名，前端用 `hbicon://<键>` 直接拿图。
///
/// 失败不落盘，只在内存里记 5 分钟——Shell 偶发失败不该让一个文件永远没图标。
pub fn icon_key(path: &Path, is_dir: bool) -> Option<String> {
    if let Some(cached) = cached_icon_key(path) {
        return cached;
    }

    let stamp = modified_stamp(path);
    let key = disk_key(path, stamp);

    // 每个线程用自己的 COM 公寓和 IImageList；不同线程之间不共享接口，
    // 也就不需要全局串行。
    let fresh = match shell_icon_png(path, is_dir) {
        Some(png) => write_icon(&key, &png).then_some(key),
        None => None,
    };
    remember(path, stamp, fresh.clone());
    fresh
}

/// 把内存里的「上次没取到」抹掉，让下一次 `icon_key` 真的再去问 Shell。
/// 刚失败就重试时用：`icon_key` 自己会把失败记 5 分钟。
pub fn forget_miss(path: &Path) {
    if let Ok(mut guard) = CACHE.lock() {
        let Some(cache) = guard.as_mut() else {
            return;
        };
        if cache.get(path).is_some_and(|entry| entry.key.is_none()) {
            cache.remove(path);
        }
    }
}

/// 正在抽取的路径。可见区、预取、滚动三处的请求会重叠，
/// 同一张图没必要抽好几遍；后来者直接跳过，结果由先来的那一路发出去。
static IN_FLIGHT: Mutex<Option<std::collections::HashSet<PathBuf>>> = Mutex::new(None);

/// 占着一条路径的分发权；`None` 表示别的线程正在抽，别重复劳动。
pub struct ExtractClaim(PathBuf);

impl Drop for ExtractClaim {
    fn drop(&mut self) {
        if let Ok(mut guard) = IN_FLIGHT.lock() {
            if let Some(set) = guard.as_mut() {
                set.remove(&self.0);
            }
        }
    }
}

pub fn claim_extraction(path: &Path) -> Option<ExtractClaim> {
    let mut guard = IN_FLIGHT
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let set = guard.get_or_insert_with(std::collections::HashSet::new);
    if set.insert(path.to_path_buf()) {
        Some(ExtractClaim(path.to_path_buf()))
    } else {
        None
    }
}

/// 兼容旧接口：直接给 base64。面板已经改成 `icon_key` + 协议。
pub fn file_icon_base64(path: &Path, is_dir: bool) -> Option<String> {
    let key = icon_key(path, is_dir)?;
    let bytes = std::fs::read(cache_file(&key)?).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(BASE64.encode(bytes))
}

fn modified_stamp(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|delta| delta.as_secs())
        .unwrap_or(0)
}

fn wide(path: &Path) -> Vec<u16> {
    // Shell 的路径解析只认反斜杠：混进 `/` 会让 SHGetFileInfoW 直接返回 0。
    let text = path.to_string_lossy().replace('/', "\\");
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 这些类型才值得问缩略图。
fn wants_thumbnail(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "bmp"
            | "webp"
            | "tif"
            | "tiff"
            | "ico"
            | "svg"
            | "mp4"
            | "mkv"
            | "mov"
            | "avi"
            | "webm"
            | "wmv"
            | "pdf"
    )
}
fn shell_icon_png(path: &Path, is_dir: bool) -> Option<Vec<u8>> {
    // 先问缩略图：照片、视频首帧、PDF 能给真预览。
    // 只对这类文件尝试 —— 文本、文件夹取缩略图要几十毫秒还拿不到，纯属浪费。
    if !is_dir && wants_thumbnail(path) {
        // 先问系统缩略图缓存：命中就是现成的图，不触发抽取；
        // 没命中再走 GetImage 抽取（抽出来的图也会进系统缓存）。
        let size = (OUTPUT_ICON_PX * 2) as u32;
        if let Some((pixels, width, height)) = crate::shell_ops::thumbnail_cached_rgba(path, size)
            .or_else(|| crate::shell_ops::thumbnail_rgba(path, size))
        {
            if let Some(png) = rgba_to_png(pixels, width as i32, height as i32) {
                return Some(png);
            }
        }
    }
    let _ = is_dir;
    ensure_com();
    let path_wide = wide(path);
    // SAFETY: 每个 HICON 都在返回前释放；接口对象由 ImageList 的 Drop 释放。
    unsafe {
        let mut info: SHFILEINFOW = std::mem::zeroed();
        let ok = SHGetFileInfoW(
            path_wide.as_ptr(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX,
        );
        if ok != 0 {
            // 先用 EXTRALARGE（48px），没有就退到 JUMBO（96px）。
            for which in [SHIL_EXTRALARGE, SHIL_JUMBO] {
                if let Some(icon) = system_icon(which, info.iIcon) {
                    let png = icon_to_png(icon);
                    DestroyIcon(icon);
                    if png.is_some() {
                        return png;
                    }
                }
            }
        }

        // 兜底：让 Shell 直接给一张 32px 的大图标。
        let mut small: SHFILEINFOW = std::mem::zeroed();
        let ok = SHGetFileInfoW(
            path_wide.as_ptr(),
            0,
            &mut small,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if ok == 0 || small.hIcon.is_null() {
            return None;
        }
        let png = icon_to_png(small.hIcon);
        DestroyIcon(small.hIcon);
        png
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn icon_to_png(icon: *mut std::ffi::c_void) -> Option<Vec<u8>> {
    let mut icon_info: ICONINFO = std::mem::zeroed();
    if GetIconInfo(icon, &mut icon_info) == 0 {
        return None;
    }
    let color = icon_info.hbmColor;
    let mask = icon_info.hbmMask;
    let result = if color.is_null() {
        None
    } else {
        bitmap_to_png(color)
    };
    if !color.is_null() {
        DeleteObject(color as HGDIOBJ);
    }
    if !mask.is_null() {
        DeleteObject(mask as HGDIOBJ);
    }
    result
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn bitmap_to_png(bitmap: *mut std::ffi::c_void) -> Option<Vec<u8>> {
    let mut info: BITMAP = std::mem::zeroed();
    if GetObjectW(
        bitmap as HGDIOBJ,
        std::mem::size_of::<BITMAP>() as i32,
        &mut info as *mut _ as *mut std::ffi::c_void,
    ) == 0
    {
        return None;
    }
    let width = info.bmWidth;
    let height = info.bmHeight;
    if width <= 0 || height <= 0 || info.bmBitsPixel != 32 {
        // 32bpp 图标自带 alpha；更早的格式要走 mask，Shell 这些年已经不给这种了。
        return None;
    }

    let mut header: BITMAPINFO = std::mem::zeroed();
    header.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    header.bmiHeader.biWidth = width;
    // 负高度=从上到下，正好是 PNG 要的顺序。
    header.bmiHeader.biHeight = -height;
    header.bmiHeader.biPlanes = 1;
    header.bmiHeader.biBitCount = 32;
    header.bmiHeader.biCompression = 0; // BI_RGB

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let screen = GetDC(std::ptr::null_mut());
    if screen.is_null() {
        return None;
    }
    let copied = GetDIBits(
        screen,
        bitmap as HGDIOBJ,
        0,
        height as u32,
        pixels.as_mut_ptr() as *mut std::ffi::c_void,
        &mut header,
        DIB_RGB_COLORS,
    );
    ReleaseDC(std::ptr::null_mut(), screen);
    if copied == 0 {
        return None;
    }

    // DIB 是 BGRA，PNG 要 RGBA。个别老图标 alpha 全 0，这时按不透明处理，
    // 否则会画出一个透明方块。
    let opaque = pixels.chunks_exact(4).all(|pixel| pixel[3] == 0);
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        if opaque {
            pixel[3] = 255;
        }
    }

    rgba_to_png(pixels, width, height)
}

/// 缩放 + 编码成 PNG（base64 前的一步）。
fn rgba_to_png(pixels: Vec<u8>, width: i32, height: i32) -> Option<Vec<u8>> {
    // 大图缩到 64px：一屏几十项也不会把 base64 撑到几 MB。
    let (pixels, width, height) = downscale(pixels, width, height, OUTPUT_ICON_PX);

    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&pixels).ok()?;
    }
    Some(png)
}

/// 盒式降采样：每 `factor × factor` 个源像素平均成一个目标像素。
///
/// 图标本来就画得干净，盒式平均足够，也不会像最近邻那样丢细节。
/// 按**长边**缩：缩略图不是方的（16:9 的截图、视频首帧），按短边缩的话
/// 长边还停在 128px，白白多占一半缓存。
fn downscale(
    pixels: Vec<u8>,
    width: i32,
    height: i32,
    target: i32,
) -> (Vec<u8>, i32, i32) {
    let long = width.max(height);
    if long <= target || target <= 0 {
        return (pixels, width, height);
    }
    let factor = (long / target).max(1);
    let out_width = (width / factor).max(1);
    let out_height = (height / factor).max(1);
    let mut out = vec![0u8; (out_width * out_height * 4) as usize];
    for y in 0..out_height {
        for x in 0..out_width {
            let mut sums = [0u32; 4];
            let mut count = 0u32;
            for dy in 0..factor {
                let sy = y * factor + dy;
                if sy >= height {
                    break;
                }
                for dx in 0..factor {
                    let sx = x * factor + dx;
                    if sx >= width {
                        break;
                    }
                    let offset = ((sy * width + sx) * 4) as usize;
                    for channel in 0..4 {
                        sums[channel] += pixels[offset + channel] as u32;
                    }
                    count += 1;
                }
            }
            if count == 0 {
                continue;
            }
            let offset = ((y * out_width + x) * 4) as usize;
            for channel in 0..4 {
                out[offset + channel] = (sums[channel] / count) as u8;
            }
        }
    }
    (out, out_width, out_height)
}

/// 面板要用的图标边长（供调用方决定是否提示用户）。
pub const fn preferred_size() -> i32 {
    PREFERRED_ICON_PX
}

/// 缓存占用：`(字节数, 文件数)`。
pub fn cache_usage() -> (u64, usize) {
    let Some(dir) = cache_dir() else {
        return (0, 0);
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut bytes = 0u64;
    let mut files = 0usize;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

/// 清空磁盘缓存，返回释放的字节数。内存缓存也一起清，否则界面上还是旧图标。
pub fn clear_cache() -> u64 {
    let (bytes, _) = cache_usage();
    if let Some(dir) = cache_dir() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    if let Ok(mut guard) = CACHE.lock() {
        if let Some(cache) = guard.as_mut() {
            cache.clear();
        }
    }
    bytes
}
#[cfg(test)]
mod tests {
    use super::*;

    fn reset_cache() {
        if let Ok(mut guard) = CACHE.lock() {
            *guard = Some(HashMap::new());
        }
    }

    /// 同一个进程里的 Shell 取图标（COM 公寓 + 系统图标列表）不是线程安全的：
    /// 几个测试并行跑会互相踩，`folders_have_their_own_icon` 隔几次就挂一次、
    /// 单跑又必过。这里的测试都拿着这把锁排队。
    static ICON_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn icon_test_guard() -> std::sync::MutexGuard<'static, ()> {
        ICON_TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// 转换和缓存放在同一个测试里：缓存是进程级的全局状态，拆开会互相污染。
    #[test]
    fn converts_icons_to_png_and_caches_by_path() {
        let _guard = icon_test_guard();
        reset_cache();
        let exe = std::env::current_exe().expect("test binary path");
        let icon = file_icon_base64(&exe, false).expect("exe should have a shell icon");
        let bytes = BASE64.decode(&icon).expect("base64");
        // PNG magic: 89 50 4E 47
        assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);

        // 同一个路径第二次走缓存，结果必须一致。
        assert_eq!(Some(icon), file_icon_base64(&exe, false));
    }

    #[test]
    fn different_programs_get_different_icons() {
        // 这是之前按扩展名缓存的 bug：所有 exe 都会拿到第一张图标。
        let _guard = icon_test_guard();
        reset_cache();
        let system = Path::new(r"C:\Windows\System32");
        let notepad = system.join("notepad.exe");
        let cmd = system.join("cmd.exe");
        if !notepad.exists() || !cmd.exists() {
            return;
        }
        let a = file_icon_base64(&notepad, false).expect("notepad icon");
        let b = file_icon_base64(&cmd, false).expect("cmd icon");

        assert_ne!(a, b, "不同程序必须拿到各自的图标");
    }

    #[test]
    fn folders_have_their_own_icon() {
        let _guard = icon_test_guard();
        reset_cache();
        let dir = std::env::temp_dir();
        let icon = file_icon_base64(&dir, true).expect("folders have an icon");
        assert!(!icon.is_empty());
    }

    #[test]
    fn icons_are_at_least_48px_so_the_panel_stays_sharp() {
        let _guard = icon_test_guard();
        reset_cache();
        let exe = std::env::current_exe().expect("test binary path");
        let icon = file_icon_base64(&exe, false).expect("icon");
        let bytes = BASE64.decode(&icon).expect("base64");
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let reader = decoder.read_info().expect("png header");
        let side = reader.info().width.max(reader.info().height);
        assert!(
            (48..=128).contains(&side),
            "图标要够清晰又不能太大，实际 {side}px"
        );
    }

    /// 缓存键要稳定：同一路径不同大小写必须同键，文件改过必须换键。
    #[test]
    fn disk_keys_are_stable_and_track_the_file() {
        let lower = disk_key(Path::new("C:\\Users\\A\\图1.png"), 100);
        let upper = disk_key(Path::new("c:\\users\\a\\图1.png"), 100);
        let touched = disk_key(Path::new("C:\\Users\\A\\图1.png"), 101);
        assert_eq!(lower, upper);
        assert_ne!(lower, touched);
        assert_eq!(lower.len(), 16);
    }

    /// 只有这几类扩展名值得问缩略图；文件夹和文本文件不该走那条慢路径。
    #[test]
    fn only_media_extensions_ask_for_thumbnails() {
        assert!(wants_thumbnail(Path::new("a/b/photo.JPG")));
        assert!(wants_thumbnail(Path::new("a/b/clip.mp4")));
        assert!(wants_thumbnail(Path::new("a/b/manual.pdf")));
        assert!(!wants_thumbnail(Path::new("a/b/notes.txt")));
        assert!(!wants_thumbnail(Path::new("a/b/no-extension")));
    }
}