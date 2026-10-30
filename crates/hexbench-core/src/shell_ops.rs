//! 交给 Shell 自己做的两件事：移动文件和缩略图。
//!
//! 之前这两块都是手写的：
//!
//! - 移动 = `fs::rename` + 「文件就复制再删」的回退。跨盘移动**文件夹**时
//!   回退分支直接放弃，用户看到的是「拖过去没反应」。
//! - 缩略图根本没有，只有图标。
//!
//! `IFileOperation` 一把解决跨盘、进度条、重名询问、长路径；`IShellItemImageFactory`
//! 一次调用就能拿到指定尺寸的缩略图（照片、视频首帧、Office 预览），
//! 拿不到时它会退化成图标。

use std::path::{Path, PathBuf};

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    FileOperation, IFileOperation, ISharedBitmap, IShellItem, IShellItemImageFactory,
    IThumbnailCache, LocalThumbnailCache, SHCreateItemFromParsingName, FOFX_ADDUNDORECORD,
    FOF_NOCONFIRMMKDIR, FOF_RENAMEONCOLLISION, FOF_WANTNUKEWARNING, SIIGBF_BIGGERSIZEOK,
    SIIGBF_THUMBNAILONLY, WTS_INCACHEONLY,
};

fn wide(path: &Path) -> HSTRING {
    HSTRING::from(path.as_os_str())
}

/// COM 按线程初始化，每个线程第一次用之前调一次。
fn ensure_com() {
    thread_local! {
        static READY: std::cell::OnceCell<()> = const { std::cell::OnceCell::new() };
    }
    READY.with(|cell| {
        cell.get_or_init(|| {
            // SAFETY: 只影响当前线程的公寓模型；已经初始化过会返回 S_FALSE。
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
        });
    });
}

/// 把若干项移进 `dest`。
///
/// 走 `IFileOperation`：跨盘、进度条、重名「是否替换」、长路径都由系统处理，
/// 而且可撤销（进回收站/撤销栈）。
pub fn move_into(sources: &[PathBuf], dest: &Path) -> Result<usize, String> {
    if sources.is_empty() {
        return Ok(0);
    }
    ensure_com();
    // SAFETY: 所有指针都由 Shell 分配并在作用域结束时释放。
    unsafe {
        let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|error| format!("IFileOperation: {error}"))?;
        // 重名自动编号（`文件 (2).txt`），和资源管理器一致，也不弹替换框。
        let flags = FOF_NOCONFIRMMKDIR | FOF_RENAMEONCOLLISION | FOFX_ADDUNDORECORD
            | FOF_WANTNUKEWARNING;
        operation
            .SetOperationFlags(flags)
            .map_err(|error| error.to_string())?;
        let target: IShellItem = SHCreateItemFromParsingName(&wide(dest), None)
            .map_err(|error| format!("目标文件夹: {error}"))?;
        let mut queued = 0usize;
        for source in sources {
            let item: IShellItem = SHCreateItemFromParsingName(&wide(source), None)
                .map_err(|error| format!("{}: {error}", source.display()))?;
            operation
                .MoveItem(&item, &target, None, None)
                .map_err(|error| error.to_string())?;
            queued += 1;
        }
        if queued == 0 {
            return Ok(0);
        }
        operation.PerformOperations().map_err(|error| error.to_string())?;
        if operation
            .GetAnyOperationsAborted()
            .map(|value| value.as_bool())
            .unwrap_or(false)
        {
            return Err("操作已取消".to_string());
        }
        Ok(queued)
    }
}

/// 改名：交给 `IFileOperation`，和资源管理器走同一条路（长路径、大小写、
/// 撤销记录都由系统处理）。名字合法性、重名由调用方先检查。
pub fn rename(path: &Path, new_name: &str) -> Result<(), String> {
    ensure_com();
    let mut new_wide: Vec<u16> = new_name.encode_utf16().collect();
    new_wide.push(0);
    // SAFETY: 字符串在调用期间有效；item / operation 都是本次创建的 COM 对象。
    unsafe {
        let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|error| format!("IFileOperation: {error}"))?;
        operation
            .SetOperationFlags(FOFX_ADDUNDORECORD | FOF_WANTNUKEWARNING)
            .map_err(|error| error.to_string())?;
        let item: IShellItem = SHCreateItemFromParsingName(&wide(path), None)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        operation
            .RenameItem(&item, PCWSTR(new_wide.as_ptr()), None)
            .map_err(|error| error.to_string())?;
        operation
            .PerformOperations()
            .map_err(|error| error.to_string())?;
        if operation
            .GetAnyOperationsAborted()
            .map(|value| value.as_bool())
            .unwrap_or(false)
        {
            return Err("改名被取消".to_string());
        }
    }
    Ok(())
}

/// 删除到回收站（Explorer 会自己弹确认框）。
pub fn trash(sources: &[PathBuf]) -> Result<usize, String> {
    if sources.is_empty() {
        return Ok(0);
    }
    ensure_com();
    // SAFETY: 指针均为 Shell 所有，作用域内释放。
    unsafe {
        let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|error| format!("IFileOperation: {error}"))?;
        // FOF_ALLOWUNDO：进回收站；不加 FOF_NOCONFIRMATION，让系统弹确认。
        operation
            .SetOperationFlags(windows::Win32::UI::Shell::FOF_ALLOWUNDO | FOFX_ADDUNDORECORD | FOF_WANTNUKEWARNING)
            .map_err(|error| error.to_string())?;
        let mut queued = 0usize;
        for source in sources {
            let item: IShellItem = SHCreateItemFromParsingName(&wide(source), None)
                .map_err(|error| format!("{}: {error}", source.display()))?;
            operation
                .DeleteItem(&item, None)
                .map_err(|error| error.to_string())?;
            queued += 1;
        }
        if queued == 0 {
            return Ok(0);
        }
        operation.PerformOperations().map_err(|error| error.to_string())?;
        if operation
            .GetAnyOperationsAborted()
            .map(|value| value.as_bool())
            .unwrap_or(false)
        {
            return Err("操作已取消".to_string());
        }
        Ok(queued)
    }
}

/// 复制若干项到 `dest`（走 IFileOperation，带进度条和重名询问）。
pub fn copy_into(sources: &[PathBuf], dest: &Path) -> Result<usize, String> {
    if sources.is_empty() {
        return Ok(0);
    }
    ensure_com();
    // SAFETY: 同上。
    unsafe {
        let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|error| format!("IFileOperation: {error}"))?;
        operation
            .SetOperationFlags(
                FOF_NOCONFIRMMKDIR | FOF_RENAMEONCOLLISION | FOFX_ADDUNDORECORD
                    | FOF_WANTNUKEWARNING,
            )
            .map_err(|error| error.to_string())?;
        let target: IShellItem = SHCreateItemFromParsingName(&wide(dest), None)
            .map_err(|error| format!("目标文件夹: {error}"))?;
        let mut queued = 0usize;
        for source in sources {
            let item: IShellItem = SHCreateItemFromParsingName(&wide(source), None)
                .map_err(|error| format!("{}: {error}", source.display()))?;
            operation
                .CopyItem(&item, &target, None, None)
                .map_err(|error| error.to_string())?;
            queued += 1;
        }
        if queued == 0 {
            return Ok(0);
        }
        operation.PerformOperations().map_err(|error| error.to_string())?;
        if operation
            .GetAnyOperationsAborted()
            .map(|value| value.as_bool())
            .unwrap_or(false)
        {
            return Err("操作已取消".to_string());
        }
        Ok(queued)
    }
}

/// 剪贴板经常被别的程序短暂占着（输入法、剪贴板历史、浏览器）。资源管理器
/// 也是重试几次；只试一次的话，`Ctrl+X` 会偶尔静默地什么都没写进去。
fn open_clipboard_with_retry() -> bool {
    use windows::Win32::System::DataExchange::OpenClipboard;
    for _ in 0..8 {
        // SAFETY: 只是尝试取得剪贴板所有权，拿不到就等一会儿再试。
        if unsafe { OpenClipboard(None) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    false
}

/// 把文件列表写进剪贴板（`CF_HDROP`），`cut=true` 时附带「剪切」标记。
///
/// 这样我们自己的粘贴、以及资源管理器的粘贴都能用同一份数据。
pub fn clipboard_set_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::CF_HDROP;

    if paths.is_empty() {
        return Err("没有要复制的文件".to_string());
    }
    // DROPFILES + 双 0 结尾的宽字符路径列表
    let mut payload: Vec<u16> = Vec::new();
    for path in paths {
        use std::os::windows::ffi::OsStrExt;
        payload.extend(path.as_os_str().encode_wide());
        payload.push(0);
    }
    payload.push(0);

    #[repr(C)]
    struct DropFiles {
        p_files: u32,
        pt_x: i32,
        pt_y: i32,
        f_nc: i32,
        f_wide: i32,
    }
    let header = std::mem::size_of::<DropFiles>();
    let bytes = header + payload.len() * 2;
    // SAFETY: 全程持锁操作剪贴板，分配的内存所有权交给系统。
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|error| error.to_string())?;
        let pointer = GlobalLock(handle) as *mut u8;
        if pointer.is_null() {
            return Err("GlobalLock 失败".to_string());
        }
        let drop_files = DropFiles {
            p_files: header as u32,
            pt_x: 0,
            pt_y: 0,
            f_nc: 0,
            f_wide: 1,
        };
        std::ptr::copy_nonoverlapping(
            &drop_files as *const DropFiles as *const u8,
            pointer,
            header,
        );
        std::ptr::copy_nonoverlapping(
            payload.as_ptr() as *const u8,
            pointer.add(header),
            payload.len() * 2,
        );
        let _ = GlobalUnlock(handle);
        if !open_clipboard_with_retry() {
            return Err("剪贴板被其他程序占用，稍后再试".to_string());
        }
        let _ = EmptyClipboard();
        if let Err(error) =
            SetClipboardData(CF_HDROP.0 as u32, Some(windows::Win32::Foundation::HANDLE(handle.0)))
        {
            // 失败也要把剪贴板交还，否则会把其他程序挡在外面。
            let _ = CloseClipboard();
            return Err(error.to_string());
        }
        // 剪切标记：Preferred DropEffect。
        // 名字必须带结尾 NUL：RegisterClipboardFormat 不会替你补。
        let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
        let format = RegisterClipboardFormatW(windows::core::PCWSTR(name.as_ptr()));
        // DROPEFFECT_MOVE = 2，DROPEFFECT_COPY = 1
        let effect: u32 = if cut { 2 } else { 1 };
        if let Ok(extra) = GlobalAlloc(GMEM_MOVEABLE, 4) {
            let cell = GlobalLock(extra) as *mut u32;
            if !cell.is_null() {
                *cell = effect;
                let _ = GlobalUnlock(extra);
                let written =
                    SetClipboardData(format, Some(windows::Win32::Foundation::HANDLE(extra.0)));
                if let Err(error) = &written {
                    log::warn!("clipboard: drop effect 写入失败 format={format}: {error}");
                }
                log::debug!("clipboard: wrote effect={effect} format={format} ok={}", written.is_ok());
            }
        }
        let _ = CloseClipboard();
    }
    Ok(())
}

/// 读剪贴板里的文件列表，以及是不是「剪切」。
#[allow(clippy::type_complexity)]
pub fn clipboard_get_files() -> Option<(Vec<PathBuf>, bool)> {
    // 刚写完剪贴板立刻读，自定义格式偶尔要晚十几毫秒才出现 —— 实测同一进程
    // 写完后第一次读会读不到 DropEffect，`Ctrl+X` 紧跟 `Ctrl+V` 就退化成复制。
    for attempt in 0..3 {
        let (paths, effect) = clipboard_read_files()?;
        match effect {
            Some(value) => return Some((paths, value & 2 != 0)),
            None if attempt < 2 => {
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            None => return Some((paths, false)),
        }
    }
    None
}

/// 读一次剪贴板。`None` 表示没有文件；第二个值 `None` 表示有文件、但剪切
/// 标记还没读到（调用方决定要不要稍后再读）。
fn clipboard_read_files() -> Option<(Vec<PathBuf>, Option<u32>)> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, RegisterClipboardFormatW,
    };
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::UI::Shell::DragQueryFileW;

    // SAFETY: 只在持锁期间读，读到的路径立刻拷贝成 owned 数据。
    unsafe {
        if !open_clipboard_with_retry() {
            return None;
        }
        if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_err() {
            let _ = CloseClipboard();
            return None;
        }
        let handle = match GetClipboardData(CF_HDROP.0 as u32) {
            Ok(handle) => handle,
            Err(_) => {
                let _ = CloseClipboard();
                return None;
            }
        };
        let drop = windows::Win32::UI::Shell::HDROP(handle.0);
        let count = DragQueryFileW(drop, 0xFFFF_FFFF, None);
        let mut paths = Vec::new();
        for index in 0..count {
            let length = DragQueryFileW(drop, index, None);
            if length == 0 {
                continue;
            }
            let mut buffer = vec![0u16; length as usize + 1];
            let written = DragQueryFileW(drop, index, Some(&mut buffer));
            if written == 0 {
                continue;
            }
            buffer.truncate(written as usize);
            paths.push(PathBuf::from(String::from_utf16_lossy(&buffer)));
        }
        // 剪切标记：名字必须带结尾 NUL，否则注册到的是另一段内存。
        let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
        let format = RegisterClipboardFormatW(windows::core::PCWSTR(name.as_ptr()));
        let mut effect_value = None;
        if let Ok(effect_handle) = GetClipboardData(format) {
            let pointer = windows::Win32::System::Memory::GlobalLock(
                windows::Win32::Foundation::HGLOBAL(effect_handle.0),
            ) as *const u32;
            if !pointer.is_null() {
                effect_value = Some(*pointer);
                let _ = windows::Win32::System::Memory::GlobalUnlock(
                    windows::Win32::Foundation::HGLOBAL(effect_handle.0),
                );
            }
        }
        let _ = CloseClipboard();
        if paths.is_empty() {
            return None;
        }
        Some((paths, effect_value))
    }
}

/// 粘贴：读剪贴板，再按「复制 / 剪切」执行到 `dest`。
pub fn paste_into(dest: &Path) -> Result<usize, String> {
    let Some((paths, cut)) = clipboard_get_files() else {
        log::warn!("paste: clipboard has no files");
        return Err("剪贴板里没有文件".to_string());
    };
    log::info!(
        "paste: {} items cut={cut} -> {}",
        paths.len(),
        dest.display()
    );
    if cut {
        let moved = move_into(&paths, dest)?;
        log::info!("paste: moved {moved}");
        // 剪切完成后清空剪贴板，和资源管理器一致。
        clipboard_clear();
        Ok(moved)
    } else {
        let copied = copy_into(&paths, dest)?;
        log::info!("paste: copied {copied}");
        Ok(copied)
    }
}

/// 把之前删除（进回收站）的文件还原到原位置。
///
/// Windows 的回收站是「每卷一个」：`<卷>:\$Recycle.Bin\<SID>\` 下成对放
/// `$I<id>`（元数据，里面有原始完整路径）和 `$R<id>`（实际内容）。
/// 这里按原始路径找到对应的一对，把 `$R` 搬回去、删掉 `$I`，
/// 效果就等于在回收站里点「还原」。
/// 清空剪贴板。剪切粘贴完成后调用，和资源管理器一致 —— 否则同一批文件
/// 还能再粘一次，而源已经没了。
pub fn clipboard_clear() {
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard};
    // SAFETY: 拿到所有权才清空；被别的程序占着就直接放弃。
    unsafe {
        if open_clipboard_with_retry() {
            let _ = EmptyClipboard();
            let _ = CloseClipboard();
        }
    }
}

pub fn restore_trashed(original: &std::path::Path) -> Result<(), String> {
    use std::path::Component;

    let prefix = match original.components().next() {
        Some(Component::Prefix(prefix)) => prefix.as_os_str().to_string_lossy().to_string(),
        _ => return Err("只能还原本机磁盘上的文件".to_string()),
    };
    let bin = std::path::PathBuf::from(format!("{prefix}\\$Recycle.Bin"));
    if original.exists() {
        return Err(format!("{} 已经存在了", original.display()));
    }

    let want = original.to_string_lossy().to_lowercase();
    let Ok(sids) = std::fs::read_dir(&bin) else {
        return Err("读不到回收站".to_string());
    };
    for sid in sids.flatten() {
        let Ok(entries) = std::fs::read_dir(sid.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("$I") || name.len() < 3 {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            if bytes.len() < 28 {
                continue;
            }
            let version = u64::from_le_bytes(bytes[0..8].try_into().unwrap_or([0; 8]));
            if version != 2 {
                continue;
            }
            let length = u32::from_le_bytes(bytes[24..28].try_into().unwrap_or([0; 4])) as usize;
            if length == 0 || bytes.len() < 28 + length * 2 {
                continue;
            }
            let utf16: Vec<u16> = bytes[28..28 + length * 2]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .take_while(|code| *code != 0)
                .collect();
            let recorded = String::from_utf16_lossy(&utf16);
            if recorded.to_lowercase() != want {
                continue;
            }
            let payload = entry.path().with_file_name(format!("$R{}", &name[2..]));
            if !payload.exists() {
                continue;
            }
            if let Some(parent) = original.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::rename(&payload, original)
                .map_err(|error| format!("还原 {}: {error}", original.display()))?;
            let _ = std::fs::remove_file(entry.path());
            log::info!("restore: {} 已还原", original.display());
            return Ok(());
        }
    }
    Err(format!(
        "回收站里找不到 {}（可能已经被清空）",
        original.display()
    ))
}
/// 只从**系统缩略图缓存**里取，不触发抽取：命中就立刻返回，没命中也是立刻
/// 放弃。资源管理器用的是同一份缓存，所以它显示过的图这里多半也有。
pub fn thumbnail_cached_rgba(path: &Path, size: u32) -> Option<(Vec<u8>, u32, u32)> {
    ensure_com();
    // SAFETY: 位图归 ISharedBitmap 所有（Detach 才会转移所有权），读完随接口
    // 一起释放，这里不能 DeleteObject。
    unsafe {
        let cache: IThumbnailCache =
            CoCreateInstance(&LocalThumbnailCache, None, CLSCTX_ALL).ok()?;
        let item: IShellItem = SHCreateItemFromParsingName(&wide(path), None).ok()?;
        let mut thumb: Option<ISharedBitmap> = None;
        cache
            .GetThumbnail(&item, size, WTS_INCACHEONLY, Some(&mut thumb), None, None)
            .ok()?;
        let bitmap = thumb?.GetSharedBitmap().ok()?;
        bitmap_to_rgba(bitmap)
    }
}

/// 取缩略图（RGBA）。拿不到缩略图时 Shell 会退化成图标，两者都失败才返回 None。
pub fn thumbnail_rgba(path: &Path, size: u32) -> Option<(Vec<u8>, u32, u32)> {
    ensure_com();
    // SAFETY: bitmap 由 Shell 返回，读完立刻释放。
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&wide(path), None).ok()?;
        let want = SIZE {
            cx: size as i32,
            cy: size as i32,
        };
        // 先只要真缩略图（照片/视频/文档）；没有的话再退一步要图标。
        let bitmap: HBITMAP = factory
            .GetImage(want, SIIGBF_BIGGERSIZEOK | SIIGBF_THUMBNAILONLY)
            .or_else(|_| factory.GetImage(want, SIIGBF_BIGGERSIZEOK))
            .ok()?;
        let result = bitmap_to_rgba(bitmap);
        let _ = DeleteObject(bitmap.into());
        result
    }
}

unsafe fn bitmap_to_rgba(bitmap: HBITMAP) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        let mut info = BITMAP::default();
        let inspected = GetObjectW(
            bitmap.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut BITMAP as *mut std::ffi::c_void),
        );
        if inspected == 0 || info.bmWidth <= 0 || info.bmHeight == 0 {
            return None;
        }
        let width = info.bmWidth as u32;
        let height = info.bmHeight.unsigned_abs();

        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // 负高度 = 从上往下，省得再翻一次。
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height,
            Some(pixels.as_mut_ptr().cast()),
            &mut header,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        if lines == 0 {
            return None;
        }

        // BGRA → RGBA；整张图不带 alpha 的（图标多半是）补成不透明。
        let opaque = pixels.chunks_exact(4).all(|pixel| pixel[3] == 0);
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            if opaque {
                pixel[3] = 255;
            }
        }
        Some((pixels, width, height))
    }
}

/// 给任意一组路径（可以跨目录）造一个数据对象，用来拖出去。
///
/// 之前是 `SHCreateDataObject`，它要求所有项属于同一个父目录，于是跨目录的
/// 多选会被**静默丢掉一部分**。改成先用 `SHParseDisplayName` 拿到绝对 PIDL，
/// 再 `SHCreateShellItemArrayFromIDLists` + `BindToHandler(BHID_DataObject)`，
/// 就能一次拖任意组合。
pub fn data_object_for(paths: &[PathBuf]) -> Option<*mut std::ffi::c_void> {
    if paths.is_empty() {
        return None;
    }
    ensure_com();
    // SAFETY: PIDL 由 Shell 分配，用完统一 CoTaskMemFree。
    unsafe {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::Common::ITEMIDLIST;
        use windows::Win32::UI::Shell::{
            SHCreateShellItemArrayFromIDLists, SHParseDisplayName, BHID_DataObject,
        };

        let mut pidls: Vec<*const ITEMIDLIST> = Vec::new();
        for path in paths {
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            let parsed = SHParseDisplayName(&wide(path), None, &mut pidl, 0, None);
            if parsed.is_err() || pidl.is_null() {
                for done in &pidls {
                    CoTaskMemFree(Some(*done as *const std::ffi::c_void));
                }
                return None;
            }
            pidls.push(pidl);
        }

        let array = SHCreateShellItemArrayFromIDLists(&pidls);
        for pidl in &pidls {
            CoTaskMemFree(Some(*pidl as *const std::ffi::c_void));
        }
        let array = array.ok()?;
        let data: windows::Win32::System::Com::IDataObject =
            array.BindToHandler(None, &BHID_DataObject).ok()?;
        Some(data.into_raw())
    }
}
