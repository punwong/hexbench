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

use windows::core::{Interface, HSTRING};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    FileOperation, IFileOperation, IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName,
    FOFX_ADDUNDORECORD, FOF_NOCONFIRMMKDIR, FOF_WANTNUKEWARNING, SIIGBF_BIGGERSIZEOK,
    SIIGBF_THUMBNAILONLY,
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
        let flags = FOF_NOCONFIRMMKDIR | FOFX_ADDUNDORECORD | FOF_WANTNUKEWARNING;
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
            .SetOperationFlags(FOF_NOCONFIRMMKDIR | FOFX_ADDUNDORECORD | FOF_WANTNUKEWARNING)
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

/// 把文件列表写进剪贴板（`CF_HDROP`），`cut=true` 时附带「剪切」标记。
///
/// 这样我们自己的粘贴、以及资源管理器的粘贴都能用同一份数据。
pub fn clipboard_set_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
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
        OpenClipboard(None).map_err(|error| error.to_string())?;
        let _ = EmptyClipboard();
        SetClipboardData(CF_HDROP.0 as u32, Some(windows::Win32::Foundation::HANDLE(handle.0)))
            .map_err(|error| error.to_string())?;
        // 剪切标记：Preferred DropEffect
        let name: Vec<u16> = "Preferred DropEffect`0".encode_utf16().collect();
        let format = RegisterClipboardFormatW(windows::core::PCWSTR(name.as_ptr()));
        // DROPEFFECT_MOVE = 2，DROPEFFECT_COPY = 1
        let effect: u32 = if cut { 2 } else { 1 };
        if let Ok(extra) = GlobalAlloc(GMEM_MOVEABLE, 4) {
            let cell = GlobalLock(extra) as *mut u32;
            if !cell.is_null() {
                *cell = effect;
                let _ = GlobalUnlock(extra);
                let _ = SetClipboardData(format, Some(windows::Win32::Foundation::HANDLE(extra.0)));
            }
        }
        let _ = CloseClipboard();
    }
    Ok(())
}

/// 读剪贴板里的文件列表，以及是不是「剪切」。
#[allow(clippy::type_complexity)]
pub fn clipboard_get_files() -> Option<(Vec<PathBuf>, bool)> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    };
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::UI::Shell::DragQueryFileW;

    // SAFETY: 只在持锁期间读，读到的路径立刻拷贝成 owned 数据。
    unsafe {
        OpenClipboard(None).ok()?;
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
        // 剪切标记
        let name: Vec<u16> = "Preferred DropEffect`0".encode_utf16().collect();
        let format = RegisterClipboardFormatW(windows::core::PCWSTR(name.as_ptr()));
        let mut cut = false;
        if let Ok(effect_handle) = GetClipboardData(format) {
            let pointer = windows::Win32::System::Memory::GlobalLock(
                windows::Win32::Foundation::HGLOBAL(effect_handle.0),
            ) as *const u32;
            if !pointer.is_null() {
                cut = (*pointer & 2) != 0;
                let _ = windows::Win32::System::Memory::GlobalUnlock(
                    windows::Win32::Foundation::HGLOBAL(effect_handle.0),
                );
            }
        }
        let _ = CloseClipboard();
        if paths.is_empty() {
            None
        } else {
            Some((paths, cut))
        }
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
        // 剪切完成后清空剪贴板里的「剪切」标记（换成复制语义），
        // 否则再粘贴一次会重复搬同一个文件。
        let _ = clipboard_set_files(&[], false);
        Ok(moved)
    } else {
        let copied = copy_into(&paths, dest)?;
        log::info!("paste: copied {copied}");
        Ok(copied)
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
