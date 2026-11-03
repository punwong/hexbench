//! 桌面右键菜单：把「新建格子」「新建收纳盒」「显示 / 隐藏格子」挂到桌面背景的右键菜单上。
//!
//! 走 `Software\Classes\DesktopBackground\Shell` 里的静态动词：不装 COM DLL、
//! 卸载就是删掉自己的那一项。代价是 Windows 11 的新版右键菜单只列系统认可的
//! 命令，第三方项会落在「显示更多选项」里（7-Zip 这些程序现在也在那儿）。
//! 进新版主菜单要写 `IExplorerCommand` 的 COM 扩展，那是另一个量级的事。
//!
//! 顶层项是一个带子菜单的动词：`SubCommands` 置空字符串，子命令写在
//! `shell\<name>` 下，资源管理器按名字排序，所以键名带数字前缀控制顺序。
//!
//! 写在 `HKLM` 而不是 `HKCU`：这台机器上（26100）实测同样的内容放 HKCU 时，
//! IShellView 枚举能看到、资源管理器真正的右键菜单却不显示，放 HKLM 就正常。
//! 有权限就写 HKLM；没有（普通用户权限）退回 HKCU——在没有这个怪癖的系统上
//! 一样能用。

use std::io::ErrorKind;
use std::path::Path;

use winreg::{RegKey, HKCU, HKLM};

/// 顶层项：右键桌面的空白处 → HexBench → 子菜单。
///
/// 键名带 `ZZ` 前缀是为了排序：`Position=Bottom` 那一段里资源管理器按注册表
/// 键名（不分大小写）排，系统的「显示设置 / 个性化」是 `Display` / `Personalize`。
/// 用不带前缀的 `HexBench` 会正好夹在这两项中间；加前缀后排到它们后面，
/// 和桌面整理类工具追加在末尾的那一项挨着，也不会混进系统设置项里。
const KEY_PATH: &str = r"Software\Classes\DesktopBackground\Shell\ZZHexBench";
/// 以前用过的键名：升级时清掉，免得菜单里出现两遍。
const LEGACY_KEY_PATH: &str = r"Software\Classes\DesktopBackground\Shell\HexBench";
const TOP_LABEL: &str = "HexBench";

/// 顶层项放在菜单的哪一段。
///
/// 桌面右键菜单的顺序由资源管理器拼：`Position=Top` 的静态动词排在最前，
/// 没写 Position 的跟在「粘贴」后面，再往后才是 COM 菜单扩展（Git / VS /
/// 桌面整理类工具），最后是 `Position=Bottom` 的静态动词和「显示设置 / 个性化」。
/// 不写 Position 时我们夹在「粘贴」和一堆第三方项中间，位置看运气；写成
/// Bottom 就和桌面上常见的那些工具一样落在菜单底部，升级系统 / 装别的软件
/// 也不会被挤来挤去。同一段里按注册表键名（不分大小写）排序。
const TOP_POSITION: &str = "Bottom";

/// 子菜单内容：键名（排序用）、显示文字、传给应用自己的命令行参数。
///
/// 命令行参数由 `src-tauri` 的启动参数 / 单实例回调接住，见 `--fence-*` / `--box-*`。
const ITEMS: [(&str, &str, &str); 4] = [
    ("10new", "新建格子…", "--fence-create-pick"),
    ("15box", "新建收纳盒", "--box-create"),
    ("20toggle", "显示 / 隐藏所有格子", "--fence-toggle"),
    ("30settings", "桌面格子设置…", "--desktop-settings"),
];

/// 注册（或刷新）菜单项；`exe` 是当前可执行文件，点菜单时由它接手。
///
/// 每次启动都会调一次，所以先看 HKLM 那份是不是已经指向自己，是就不动注册表。
pub fn install(exe: &Path) -> Result<(), String> {
    let exe = exe.to_string_lossy().to_string();
    let in_machine = HKLM.open_subkey(KEY_PATH).is_ok();
    if in_machine && points_at(&exe) {
        // 以前版本留在 HKCU / 旧键名下的那份清掉，免得出现两遍。
        for root in [HKLM, HKCU] {
            let _ = root.delete_subkey_all(LEGACY_KEY_PATH);
        }
        let _ = HKCU.delete_subkey_all(KEY_PATH);
        return Ok(());
    }
    match write_menu(HKLM, &exe) {
        Ok(()) => {
            let _ = HKCU.delete_subkey_all(KEY_PATH);
            for root in [HKLM, HKCU] {
                let _ = root.delete_subkey_all(LEGACY_KEY_PATH);
            }
        }
        // 没权限写 HKLM 且 HKLM 里还没有：退回当前用户（在没有上面那个怪癖的
        // 系统上一样能用）。HKLM 已经有、只是没法更新，就别再写 HKCU 造成重复。
        Err(error) if error.kind() == ErrorKind::PermissionDenied && !in_machine => {
            write_menu(HKCU, &exe).map_err(|error| format!("写注册表失败：{error}"))?;
            let _ = HKCU.delete_subkey_all(LEGACY_KEY_PATH);
        }
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            return Err("更新右键菜单需要管理员权限".to_string());
        }
        Err(error) => return Err(format!("写注册表失败：{error}")),
    }
    refresh_explorer();
    Ok(())
}

/// HKLM 里那份菜单是不是已经是最新的一版：指向 `exe`、位置正确、
/// 每个子项都在。参数变了（比如加了「新建收纳盒」）也会重写一遍。
fn points_at(exe: &str) -> bool {
    let Ok(menu) = HKLM.open_subkey(KEY_PATH) else {
        return false;
    };
    let position_ok = menu
        .get_value::<String, _>("Position")
        .map(|value| value == TOP_POSITION)
        .unwrap_or(false);
    position_ok
        && ITEMS.iter().all(|(name, _, args)| {
            menu.open_subkey(format!("shell\\{name}\\command"))
                .and_then(|command| command.get_value::<String, _>(""))
                .map(|value| value == command_line(exe, args))
                .unwrap_or(false)
        })
}

/// 把整棵菜单树写到 `root` 下。`PermissionDenied` 表示没这个权限，交给调用方决定。
fn write_menu(root: &RegKey, exe: &str) -> std::io::Result<()> {
    let menu = root.create_subkey(KEY_PATH)?.0;
    menu.set_value("MUIVerb", &TOP_LABEL.to_string())?;
    // 空字符串 = 子菜单取自下面的 shell\<name>。
    menu.set_value("SubCommands", &String::new())?;
    menu.set_value("Position", &TOP_POSITION.to_string())?;
    menu.set_value("Icon", &format!("{exe},0"))?;
    for (name, label, args) in ITEMS {
        let item = menu.create_subkey(format!("shell\\{name}"))?.0;
        item.set_value("MUIVerb", &label.to_string())?;
        let command = item.create_subkey("command")?.0;
        command.set_value("", &command_line(exe, args))?;
    }
    Ok(())
}

/// 关掉开关时把自己那一项整个删掉（别人的菜单项不动）。
pub fn uninstall() -> Result<(), String> {
    let mut first_error = None;
    for root in [HKLM, HKCU] {
        for path in [KEY_PATH, LEGACY_KEY_PATH] {
            match root.delete_subkey_all(path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                // 没权限删 HKLM：HKCU 那份仍然清掉，报错交给调用方。
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
    }
    if let Some(error) = first_error {
        return Err(format!("删注册表项失败：{error}"));
    }
    refresh_explorer();
    Ok(())
}

/// 菜单项现在在不在（启动时用来对一下状态）。
pub fn is_installed() -> bool {
    [HKLM, HKCU].iter().any(|root| {
        root.open_subkey(KEY_PATH).is_ok() || root.open_subkey(LEGACY_KEY_PATH).is_ok()
    })
}

/// `"C:\...\hexbench-app.exe" --fence-toggle`
fn command_line(exe: &str, args: &str) -> String {
    format!("\"{exe}\" {args}")
}

/// 资源管理器会把动词表缓存一阵；改完注册表通知一声，右键立刻能看到。
fn refresh_explorer() {
    use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手动诊断：把桌面背景的右键菜单整个枚举出来，看资源管理器到底读到了什么。
    ///
    /// `cargo test -p hexbench-core probe_desktop_background_menu -- --ignored --nocapture`
    #[test]
    #[ignore = "手动：需要桌面会话，人工看输出"]
    fn probe_desktop_background_menu() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
            let folder = windows::Win32::UI::Shell::SHGetDesktopFolder().unwrap();
            let view: windows::Win32::UI::Shell::IShellView = folder
                .CreateViewObject(windows::Win32::Foundation::HWND::default())
                .unwrap();
            let menu: windows::Win32::UI::Shell::IContextMenu = view
                .GetItemObject(windows::Win32::UI::Shell::SVGIO_BACKGROUND)
                .unwrap();
            let hmenu = windows::Win32::UI::WindowsAndMessaging::CreatePopupMenu().unwrap();
            menu.QueryContextMenu(hmenu, 0, 1, 0x7FFF, 0)
                .unwrap();
            dump_menu(hmenu, 0);
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyMenu(hmenu);
        }
    }

    unsafe fn dump_menu(hmenu: windows::Win32::UI::WindowsAndMessaging::HMENU, depth: usize) {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetMenuItemCount, GetMenuItemInfoW, MENUITEMINFOW, MIIM_STRING, MIIM_SUBMENU,
        };
        let count = unsafe { GetMenuItemCount(Some(hmenu)) };
        println!("{}[{}] {} 项", "  ".repeat(depth), depth, count);
        for index in 0..count {
            let mut buffer = [0u16; 256];
            let mut info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING | MIIM_SUBMENU,
                dwTypeData: windows::core::PWSTR(buffer.as_mut_ptr()),
                cch: buffer.len() as u32,
                ..Default::default()
            };
            if unsafe { GetMenuItemInfoW(hmenu, index as u32, true, &mut info) }.is_ok() {
                let text = String::from_utf16_lossy(&buffer[..info.cch as usize]);
                println!("{}{:02} {:?}", "  ".repeat(depth), index, text);
                if !info.hSubMenu.is_invalid() {
                    unsafe { dump_menu(info.hSubMenu, depth + 1) };
                }
            }
        }
    }

    #[test]
    fn command_line_quotes_the_exe() {
        assert_eq!(
            command_line(r"C:\Program Files\HexBench\hexbench-app.exe", "--fence-toggle"),
            "\"C:\\Program Files\\HexBench\\hexbench-app.exe\" --fence-toggle"
        );
    }

    #[test]
    fn submenu_order_is_explicit() {
        let names: Vec<&str> = ITEMS.iter().map(|(name, _, _)| *name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "键名前缀就是显示顺序，别写乱");
    }
}
