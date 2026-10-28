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