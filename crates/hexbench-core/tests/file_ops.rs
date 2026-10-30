//! 文件区的自动化验收测试。
//!
//! 直接打面板实际调用的那一层：`sys::{create_folder, rename_path}` 和
//! `shell_ops::{clipboard_*, copy_into, move_into, paste_into, trash,
//! restore_trashed, thumbnail_rgba, data_object_for}`。走的是真的
//! `IFileOperation` / 回收站 / 系统剪贴板，不是 mock。
//!
//! 全部在 `%TEMP%\hexbench-it-<pid>-*` 沙箱里跑，结束即删，不碰用户配置
//! 和真实文件。
//!
//! 跑法（剪贴板和回收站是系统级状态，必须串行）：
//!
//! ```text
//! cargo test -p hexbench-core --test file_ops -- --test-threads=1 --nocapture
//! ```

#![cfg(windows)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use hexbench_core::{shell_ops, sys};
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RETURN;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    PostMessageW, WM_KEYDOWN, WM_KEYUP,
};

// ---------------------------------------------------------------- 排队

/// 剪贴板、回收站是**系统级**状态：cargo 默认会并行跑同一个测试二进制里的用例
/// （CI 就是这样），两个用例同时写剪贴板就会互相覆盖。每个用例先拿这把锁。
/// 本地为了看得清输出仍然建议 `--test-threads=1`，但那不再是必须的。
static SYSTEM_STATE: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SYSTEM_STATE.lock().unwrap_or_else(|error| error.into_inner())
}

// ---------------------------------------------------------------- 沙箱

fn sandbox(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hexbench-it-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("建立沙箱目录");
    // `%TEMP%` 常常是 `C:\Users\ADMINI~1\...` 这种 8.3 短路径；回收站里记的是
    // 长路径，不转成一致的写法，`restore_trashed` 会找不到。
    let long = fs::canonicalize(&dir).unwrap_or(dir);
    match long.to_string_lossy().strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => long,
    }
}

fn write_file(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("建立父目录");
    }
    fs::write(&path, body).expect("写测试文件");
    path
}


// ---------------------------------------------------------------- 新建目录

#[test]
fn create_folder_gets_unique_names() {
    let _serial = serial();
    let dir = sandbox("create");
    let first = sys::create_folder(&dir, "新建文件夹").expect("第一次新建");
    let second = sys::create_folder(&dir, "新建文件夹").expect("第二次新建");
    let third = sys::create_folder(&dir, "新建文件夹").expect("第三次新建");

    assert!(first.is_dir() && second.is_dir() && third.is_dir());
    assert_eq!(first.file_name().unwrap(), "新建文件夹");
    assert_eq!(second.file_name().unwrap(), "新建文件夹 (2)");
    assert_eq!(third.file_name().unwrap(), "新建文件夹 (3)");
    assert!(sys::create_folder(&dir.join("nope"), "x").is_err());
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- 重命名

#[test]
fn rename_accepts_good_names_and_rejects_bad_ones() {
    let _serial = serial();
    let dir = sandbox("rename");
    let file = write_file(&dir, "旧名字.txt", "hello");

    let mut current = sys::rename_path(&file, "  新名字.txt  ").expect("正常改名");
    assert_eq!(current.file_name().unwrap(), "新名字.txt", "两头的空格应被去掉");
    assert!(current.exists() && !file.exists());
    assert_eq!(fs::read_to_string(&current).unwrap(), "hello");

    for bad in ["", "a/b.txt", "a:b.txt", "CON", "NUL.txt", "结尾点."] {
        let result = sys::rename_path(&current, bad);
        assert!(result.is_err(), "「{bad}」应该被拒绝");
        assert!(current.exists(), "被拒绝的名字不能动到原文件");
    }

    // 结尾空格会被吃掉，然后按去掉空格的名字改。资源管理器同样会静默去掉。
    current = sys::rename_path(&current, "带空格  ").expect("结尾空格应被去掉后改成功");
    assert_eq!(current.file_name().unwrap(), "带空格");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rename_refuses_to_overwrite_an_existing_file() {
    let _serial = serial();
    let dir = sandbox("rename-clash");
    let a = write_file(&dir, "a.txt", "a");
    write_file(&dir, "b.txt", "b");
    let result = sys::rename_path(&a, "b.txt");
    assert!(result.is_err(), "同名文件不能被覆盖");
    assert!(a.exists());
    assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "b");
    let _ = fs::remove_dir_all(&dir);
}

/// 只改大小写：资源管理器允许，走 `IFileOperation` 之后我们也必须允许。
#[test]
fn rename_allows_case_only_change() {
    let _serial = serial();
    let dir = sandbox("rename-case");
    let file = write_file(&dir, "case.txt", "x");
    let renamed = sys::rename_path(&file, "CASE.txt").expect("只改大小写应该成功");
    assert_eq!(renamed.file_name().unwrap(), "CASE.txt");
    assert_eq!(fs::read_to_string(&renamed).unwrap(), "x");
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- 移动

#[test]
fn move_into_carries_files_and_folders() {
    let _serial = serial();
    let dir = sandbox("move");
    let dest = dir.join("目标");
    fs::create_dir_all(&dest).unwrap();
    let file = write_file(&dir, "文件.txt", "123");
    let folder = dir.join("子文件夹");
    fs::create_dir_all(&folder).unwrap();
    write_file(&folder, "里面.txt", "deep");

    let moved = shell_ops::move_into(&[file.clone(), folder.clone()], &dest).expect("移动");
    assert_eq!(moved, 2);
    assert!(!file.exists() && !folder.exists());
    assert_eq!(fs::read_to_string(dest.join("文件.txt")).unwrap(), "123");
    assert_eq!(
        fs::read_to_string(dest.join("子文件夹").join("里面.txt")).unwrap(),
        "deep"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// 目标里已经有同名文件时，`FOF_RENAMEONCOLLISION` 应自动编号，不能覆盖。
#[test]
fn move_and_copy_rename_on_collision() {
    let _serial = serial();
    let dir = sandbox("collision");
    let dest = dir.join("目标");
    fs::create_dir_all(&dest).unwrap();
    write_file(&dest, "同名.txt", "原有的");

    let mover = write_file(&dir, "同名.txt", "要移过去的");
    let moved = shell_ops::move_into(std::slice::from_ref(&mover), &dest).expect("移动");
    assert_eq!(moved, 1);
    assert_eq!(fs::read_to_string(dest.join("同名.txt")).unwrap(), "原有的");
    let after_move: Vec<String> = fs::read_dir(&dest)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    println!("[info] 移动撞名后目标目录：{after_move:?}");
    assert_eq!(after_move.len(), 2, "撞名应自动编号，而不是覆盖");

    let copier = write_file(&dir, "同名.txt", "要复制的");
    let copied = shell_ops::copy_into(std::slice::from_ref(&copier), &dest).expect("复制");
    assert_eq!(copied, 1);
    assert!(copier.exists(), "复制后源文件必须还在");
    assert_eq!(fs::read_to_string(dest.join("同名.txt")).unwrap(), "原有的");
    let after_copy: Vec<String> = fs::read_dir(&dest)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    println!("[info] 复制撞名后目标目录：{after_copy:?}");
    assert_eq!(after_copy.len(), 3, "复制撞名应自动编号，而不是覆盖");
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- 剪贴板

fn set_clipboard_with_retry(paths: &[PathBuf], cut: bool) {
    let mut last = String::new();
    for _ in 0..10 {
        match shell_ops::clipboard_set_files(paths, cut) {
            Ok(()) => return,
            Err(error) => {
                last = error;
                thread::sleep(Duration::from_millis(120));
            }
        }
    }
    panic!("剪贴板写不进去：{last}");
}

#[test]
fn clipboard_copy_then_paste_duplicates() {
    let _serial = serial();
    let dir = sandbox("copy");
    let source = write_file(&dir, "原文件.txt", "copy me");
    let dest = dir.join("粘贴到");
    fs::create_dir_all(&dest).unwrap();

    set_clipboard_with_retry(std::slice::from_ref(&source), false);
    let (paths, cut) = shell_ops::clipboard_get_files().expect("读回剪贴板");
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].file_name(), source.file_name());
    assert!(!cut, "复制不该带剪切标记");

    let pasted = shell_ops::paste_into(&dest).expect("粘贴");
    assert_eq!(pasted, 1);
    assert!(source.exists(), "复制后源文件必须还在");
    assert_eq!(fs::read_to_string(dest.join("原文件.txt")).unwrap(), "copy me");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn clipboard_cut_then_paste_moves() {
    let _serial = serial();
    let dir = sandbox("cut");
    let source = write_file(&dir, "剪走的.txt", "cut me");
    let dest = dir.join("粘贴到");
    fs::create_dir_all(&dest).unwrap();

    set_clipboard_with_retry(std::slice::from_ref(&source), true);
    let (_, cut) = shell_ops::clipboard_get_files().expect("读回剪贴板");
    assert!(cut, "剪切必须带标记");

    let moved = shell_ops::paste_into(&dest).expect("粘贴");
    assert_eq!(moved, 1);
    assert!(!source.exists(), "剪切粘贴后源文件应消失");
    assert_eq!(fs::read_to_string(dest.join("剪走的.txt")).unwrap(), "cut me");

    // 资源管理器剪切粘贴完会清掉剪贴板；这里必须保持一致。
    match shell_ops::clipboard_get_files() {
        Some((paths, cut)) => {
            panic!("剪切粘贴后剪贴板还有 {} 项（cut={cut}），应已清空", paths.len())
        }
        None => println!("[info] 剪切粘贴后剪贴板已清空"),
    }
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- 删除 / 还原

/// 面板的 Delete 走 `IFileOperation`，不加 `FOF_NOCONFIRMATION`，系统会弹
/// 确认框。自动化测试必须替用户按下「是」，否则会卡在对话框上。
/// 返回被点掉的对话框标题，方便诊断。
fn spawn_dialog_dismisser(duration: Duration) -> (Arc<AtomicBool>, JoinHandle<Vec<String>>) {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let mut seen: Vec<String> = Vec::new();
        let mut last_press: HashMap<isize, Instant> = HashMap::new();
        let deadline = Instant::now() + duration;
        let pid = unsafe { GetCurrentProcessId() };
        while !flag.load(Ordering::SeqCst) && Instant::now() < deadline {
            let mut windows: Vec<HWND> = Vec::new();
            unsafe {
                EnumWindows(Some(collect_window), &mut windows as *mut _ as LPARAM);
            }
            for hwnd in windows {
                let mut owner = 0u32;
                unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
                if owner != pid || unsafe { IsWindowVisible(hwnd) } == 0 {
                    continue;
                }
                if window_text(hwnd, GetClassNameW) != "#32770" {
                    continue;
                }
                let title = window_text(hwnd, GetWindowTextW);
                if !seen.contains(&title) {
                    seen.push(title.clone());
                }
                // 同一个对话框最多 3 次/秒，避免连发回车。
                let recently = last_press
                    .get(&(hwnd as isize))
                    .is_some_and(|at| at.elapsed() < Duration::from_millis(300));
                if !recently {
                    unsafe {
                        PostMessageW(hwnd, WM_KEYDOWN, VK_RETURN as usize, 0x001C_0001);
                        PostMessageW(hwnd, WM_KEYUP, VK_RETURN as usize, 0xC01C_0001u32 as isize);
                    }
                    last_press.insert(hwnd as isize, Instant::now());
                }
            }
            thread::sleep(Duration::from_millis(80));
        }
        seen
    });
    (stop, handle)
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam as *mut Vec<HWND>) };
    list.push(hwnd);
    1
}

fn window_text(hwnd: HWND, api: unsafe extern "system" fn(HWND, *mut u16, i32) -> i32) -> String {
    let mut buffer = vec![0u16; 256];
    let length = unsafe { api(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if length <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}

#[test]
fn trash_and_restore_roundtrip() {
    let _serial = serial();
    let dir = sandbox("trash");
    let file = write_file(&dir, "删掉又还回来.txt", "recycle me");

    let (stop, dismisser) = spawn_dialog_dismisser(Duration::from_secs(20));
    let removed = shell_ops::trash(std::slice::from_ref(&file));
    stop.store(true, Ordering::SeqCst);
    let dialogs = dismisser.join().unwrap_or_default();
    println!("[info] 删除过程中出现的对话框：{dialogs:?}");

    assert_eq!(removed.expect("删除到回收站"), 1);
    assert!(!file.exists(), "删除后原位置应该没有文件");

    shell_ops::restore_trashed(&file).expect("从回收站还原");
    assert!(file.exists(), "还原后文件应该回到原位置");
    assert_eq!(fs::read_to_string(&file).unwrap(), "recycle me");
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- 图标 / 缩略图 / 拖拽

#[test]
fn shell_provides_icons_and_thumbnails() {
    let _serial = serial();
    let dir = sandbox("icons");
    let text = write_file(&dir, "说明.txt", "plain");

    // `file_icon_base64` 给的是裸 base64，`data:image/png;base64,` 前缀由前端补。
    // Shell 取图标偶尔会抽风（实测同一文件前后两次一次成功一次失败），
    // 所以重试几次再判失败。
    let icon_of = |path: &Path, is_dir: bool| {
        for attempt in 0..5 {
            if let Some(data) = hexbench_core::icons::file_icon_base64(path, is_dir) {
                if !data.is_empty() {
                    return Some(data);
                }
            }
            println!("[retry] 第 {} 次没取到图标：{}", attempt + 1, path.display());
            thread::sleep(Duration::from_millis(300));
        }
        None
    };
    assert!(icon_of(&text, false).is_some(), "文本文件要有图标");
    assert!(icon_of(&dir, true).is_some(), "文件夹要有图标");

    // 缩略图依赖 Shell 的缩略图处理器，取不到时会退化成图标（面板本身也这么兜底），
    // 所以这里只记录、不判失败。
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/assets/launcher-files.png");
    if image.exists() {
        // 先探一下系统缩略图缓存：命中就直接返回，没命中也不该报错。
        match shell_ops::thumbnail_cached_rgba(&image, 128) {
            Some((_, width, height)) => println!("[info] 系统缩略图缓存命中 {width}x{height}"),
            None => println!("[info] 系统缩略图缓存没有这张（会走 GetImage 抽取）"),
        }
        match shell_ops::thumbnail_rgba(&image, 96) {
            Some((pixels, width, height)) => {
                assert_eq!(pixels.len() as u32, width * height * 4);
                println!("[info] 缩略图 {width}x{height}");
            }
            None => println!("[info] 这次没取到缩略图（会退化成文件图标）"),
        }
    } else {
        println!("[skip] 没有测试图片 {}", image.display());
    }
    let _ = fs::remove_dir_all(&dir);
}

/// 面板现在拿的是缓存键，靠 `hbicon://<键>` 从磁盘取图：键必须是合法的
/// 缓存文件名，文件必须真的落盘。
#[test]
fn icon_keys_point_at_real_cache_files() {
    let _serial = serial();
    let dir = sandbox("icon-key");
    let text = write_file(&dir, "说明.txt", "plain");
    let key = (0..5)
        .find_map(|_| hexbench_core::icons::icon_key(&text, false))
        .expect("文本文件应该有图标键");
    assert_eq!(key.len(), 16);
    assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let file = hexbench_core::icons::cache_file(&key).expect("缓存文件路径");
    assert!(file.exists(), "缓存键对应的 PNG 应该已经写盘");
    assert!(file.metadata().unwrap().len() > 0);
    assert_eq!(
        Some(key),
        hexbench_core::icons::icon_key(&text, false),
        "同一个文件两次拿到的键必须一致"
    );

    // 协议那边直接拿路径拼文件名，非法键必须挡在缓存目录之外。
    for bad in ["../evil", "a/b", "", "0123456789abcde", "0123456789abcdef0"] {
        assert!(
            hexbench_core::icons::cache_file(bad).is_none(),
            "「{bad}」不该被当成缓存键"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// 取不到图标不能落盘成“永久没有”：Shell 偶发失败一次，文件就再也不出图
/// 是之前的真 bug。
#[test]
fn missing_icons_are_not_persisted_to_disk() {
    let _serial = serial();
    let ghost = PathBuf::from(r"C:\hexbench-missing\ghost.xyz");
    assert!(hexbench_core::icons::icon_key(&ghost, false).is_none());
    if let Some(dir) = hexbench_core::icons::cache_dir() {
        let stale = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| {
                        entry.path().extension().and_then(|value| value.to_str()) == Some("none")
                    })
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(stale, 0, "不该再有 .none 落盘记录");
    }
}

#[test]
fn drag_payload_is_a_real_data_object() {
    let _serial = serial();
    let dir = sandbox("drag");
    let file = write_file(&dir, "拖出去.txt", "drag");
    let object = shell_ops::data_object_for(&[file]);
    assert!(object.is_some(), "拖拽应能构造 IDataObject");
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- 规模

/// 面板一次最多列 1000 项（`list_folder` 里的硬上限）。这里确认底层列目录
/// 本身没有问题，上限在应用层，属于和资源管理器的已知差距。
#[test]
fn large_directory_is_listed_quickly_by_the_os() {
    let _serial = serial();
    let dir = sandbox("many");
    for index in 0..1000 {
        fs::write(dir.join(format!("file-{index:04}.txt")), "x").unwrap();
    }
    let started = Instant::now();
    let count = fs::read_dir(&dir).unwrap().count();
    println!("[info] 1000 项列目录耗时 {:?}", started.elapsed());
    assert_eq!(count, 1000);
    let _ = fs::remove_dir_all(&dir);
}
