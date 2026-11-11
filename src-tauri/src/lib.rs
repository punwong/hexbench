use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, RunEvent, State, WebviewUrl,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_updater::UpdaterExt;

/// Set only by the tray "quit" item (and app.exit calls), so the exit veto can
/// tell "the user closed the window" from "the user wants to quit".
static QUITTING: AtomicBool = AtomicBool::new(false);

/// WebView2 自带的「原生窗口遮挡」计算会在窗口被别的程序挡住一段时间后
/// 冻结渲染进程；再点回来时主线程等它恢复，窗口就变成「未响应」。
/// 设置窗口常被压在别的窗口后面，所以所有 WebView 窗口都关掉这项计算。
/// `--disable-features` 会覆盖 wry 默认带的那几个，必须一起写上。
const WEBVIEW_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion";

use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use hexbench_core::config::{
    self, Config, DesktopSettings, FenceKind, FenceRule, FenceSettings, NotePin,
};
use hexbench_core::engine::Engine;
use hexbench_core::geometry::Region;
use hexbench_core::model::{Monitor, Rect, Side};
use hexbench_core::trigger::{Direction, Hit};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MonitorDto {
    index: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    /// Work area (taskbar excluded): the trigger band and fence geometry use it.
    work_x: i32,
    work_y: i32,
    work_w: i32,
    work_h: i32,
    primary: bool,
    device: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegionDto {
    monitor: usize,
    side: Side,
    edge_x: i32,
    y0: i32,
    y1: i32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HitDto {
    monitor: usize,
    side: Side,
    edge_x: i32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsDto {
    ticks: u64,
    fires: u64,
    failures: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PointDto {
    x: i32,
    y: i32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    config: Config,
    monitors: Vec<MonitorDto>,
    regions: Vec<RegionDto>,
    cursor: Option<PointDto>,
    hit: Option<HitDto>,
    guard: Option<String>,
    paused: bool,
    stats: StatsDto,
    version: String,
    config_path: String,
    autostart: bool,
    desktop_count: Option<usize>,
    engine_phase: String,
    accent: Option<String>,
    hotkey_error: Option<String>,
}

struct Shared {
    config: Config,
    paused: bool,
    cursor: Option<(i32, i32)>,
    hit: Option<Hit>,
    guard: Option<String>,
    monitors: Vec<MonitorDto>,
    regions: Vec<RegionDto>,
    stats: StatsDto,
    autostart: bool,
    desktop_count: Option<usize>,
    engine_phase: String,
    accent: Option<String>,
    hotkey_error: Option<String>,
    refresh_requested: bool,
}

impl Shared {
    fn new(config: Config) -> Self {
        Self {
            config,
            paused: false,
            cursor: None,
            hit: None,
            guard: None,
            monitors: Vec::new(),
            regions: Vec::new(),
            engine_phase: "idle".to_string(),
            stats: StatsDto { ticks: 0, fires: 0, failures: 0 },
            autostart: false,
            desktop_count: None,
            accent: None,
            hotkey_error: None,
            refresh_requested: false,
        }
    }
}

struct AppState {
    shared: Arc<Mutex<Shared>>,
    config_path: PathBuf,
    /// Installer bytes fetched by `update_download`, waiting for
    /// `update_install`. Kept in memory (a couple of MB) so the user can
    /// download first and restart later.
    pending_update: Mutex<Option<Vec<u8>>>,
    /// Result of the last update check, shared by every window so opening the
    /// main window does not hit the network again.
    update_status: Mutex<UpdateStatus>,
    /// 面板里点了「设置」但主窗口这会儿不在：记下要去哪一页，
    /// 等窗口（和页面）起来之后由前端来取。主窗口关掉一阵子会被销毁，
    /// 直接 emit 会发给一个还不存在的页面，用户看到的就是「点齿轮没反应」。
    pending_page: Mutex<Option<(String, Option<String>)>>,
    /// 托盘菜单：勾选项要跟着暂停状态走。
    tray_menu: Mutex<Option<Menu<tauri::Wry>>>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateStatus {
    version: Option<String>,
    error: Option<String>,
    checked_at: i64,
    /// 上次查到的新版本说明与日期：窗口重开时也直接能显示。
    notes: Option<String>,
    date: Option<String>,
}

#[tauri::command]
fn diagnostics(app: AppHandle, state: State<'_, AppState>) -> String {
    let mut out = String::new();
    out.push_str(&format!("HexBench {}\n", app.package_info().version));
    out.push_str(&format!(
        "when: {:?}  os: {}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        std::env::consts::OS
    ));

    {
        let shared = state.shared.lock().unwrap();
        out.push_str(&format!(
            "phase={} paused={} cursor={:?} hit={:?} guard={:?}\n",
            shared.engine_phase, shared.paused, shared.cursor, shared.hit, shared.guard
        ));
        out.push_str(&format!(
            "ticks={} fires={} sendFailures={} desktops={:?}\n",
            shared.stats.ticks,
            shared.stats.fires,
            shared.stats.failures,
            shared.desktop_count
        ));
        out.push_str("--- monitors ---\n");
        for monitor in &shared.monitors {
            out.push_str(&format!(
                "#{} {}x{} at {},{}{}\n",
                monitor.index,
                monitor.w,
                monitor.h,
                monitor.x,
                monitor.y,
                if monitor.primary { " (primary)" } else { "" }
            ));
        }
        out.push_str("--- config (password removed) ---\n");
        let mut config = shared.config.clone();
        config.sync.password.clear();
        if let Ok(text) = serde_json::to_string_pretty(&config) {
            out.push_str(&text);
            out.push('\n');
        }
    }

    if let Ok(dir) = app.path().app_log_dir() {
        let path = dir.join("HexBench.log");
        if let Ok(text) = std::fs::read_to_string(&path) {
            out.push_str("--- log tail ---\n");
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(150);
            for line in &lines[start..] {
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    out
}

#[tauri::command]
fn snapshot(state: State<'_, AppState>) -> Snapshot {
    let shared = state.shared.lock().unwrap();
    Snapshot {
        config: shared.config.clone(),
        monitors: shared.monitors.clone(),
        regions: shared.regions.clone(),
        cursor: shared.cursor.map(|(x, y)| PointDto { x, y }),
        hit: shared.hit.map(|hit| HitDto {
            monitor: hit.monitor,
            side: hit.side,
            edge_x: hit.edge_x,
        }),
        guard: shared.guard.clone(),
        paused: shared.paused,
        stats: shared.stats.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        config_path: state.config_path.display().to_string(),
        autostart: shared.autostart,
        desktop_count: shared.desktop_count,
        engine_phase: shared.engine_phase.clone(),
        accent: shared.accent.clone(),
        hotkey_error: shared.hotkey_error.clone(),
    }
}

/// 主题偏好：system / light / dark。面板和网页窗口自己解析 system。
#[tauri::command]
fn ui_theme_preference(state: State<'_, AppState>) -> String {
    let theme = state.shared.lock().unwrap().config.ui.theme;
    match theme {
        hexbench_core::config::Theme::Dark => "dark",
        hexbench_core::config::Theme::Light => "light",
        hexbench_core::config::Theme::System => "system",
    }
    .to_string()
}
/// 主题变了就主动推给常驻窗口。
///
/// 呼出面板和网页窗口是常驻的（只是 hide/show），改设置不会让它们重新加载，
/// 页面里那一次 useEffect 也就不会重跑，所以必须从这边发一段 JS 过去。
fn push_theme_to_windows(app: &AppHandle) {
    let theme = app.state::<AppState>().shared.lock().unwrap().config.ui.theme;
    let value = match theme {
        hexbench_core::config::Theme::Dark => "dark",
        hexbench_core::config::Theme::Light => "light",
        hexbench_core::config::Theme::System => "system",
    };
    // 「跟系统」交给页面自己算：每个窗口的 prefers-color-scheme 才是它真正
    // 该看的（窗口跨显示器、系统深浅在运行中切换，都只有页面自己知道）。
    let script = format!(
        "(function(){{var p='{value}';var d=p==='dark'||(p==='system'&&window.matchMedia('(prefers-color-scheme: dark)').matches);document.documentElement.dataset.theme=d?'dark':'light';}})();"
    );
    for label in ["launcher", WEB_PANEL] {
        if label == WEB_PANEL {
            if let Some(window) = web_window_handle() {
                let _ = window.eval(&script);
            }
        } else if let Some(window) = app.get_webview_window(label) {
            let _ = window.eval(&script);
        }
    }
    // 钉住的便签是常驻窗口，改深浅主题也要当场跟上。
    for (label, window) in app.webview_windows() {
        if label.starts_with("note-") {
            let _ = window.eval(&script);
        }
    }
    // 格子是原生窗口，主题进 `Frame`，由管理器按配置重推。
    log::info!("theme: pushed {value} to launcher / web");
}

/// 手势设置和主题一样是常驻窗口的：设置页改完直接推给每个格子，
/// 不用为了换开关去重开格子。面板走 `launcher-changed` 自己重读。
fn push_gestures_to_windows(_app: &AppHandle, settings: &hexbench_core::config::GestureSettings) {
    // 格子直接投给渲染线程。
    if let Some(host) = NATIVE_FENCES.lock().ok().and_then(|slot| slot.clone()) {
        host.set_gestures(settings.clone());
    }
}

#[tauri::command]
fn save_config(
    config: Config,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    // 设置窗口发回来的是它手里那份整份配置，可能已经旧了：面板尺寸、网页
    // 窗口几何、上次停留的位置、同步时间戳只有后端在写（面板收起时记、
    // 拖动时记），拿旧快照覆盖就会把这些刚记下的值冲回旧值——表现就是
    // 「设置里改了一处，面板又回到上次的位置/尺寸」。这些字段以内存为准。
    let mut config = config;
    {
        let shared = state.shared.lock().unwrap();
        let live = &shared.config.launcher;
        let launcher = &mut config.launcher;
        launcher.panel_w = live.panel_w;
        launcher.panel_h = live.panel_h;
        launcher.panel_pct_w = live.panel_pct_w;
        launcher.panel_pct_h = live.panel_pct_h;
        launcher.site_x = live.site_x;
        launcher.site_y = live.site_y;
        launcher.site_w = live.site_w;
        launcher.site_h = live.site_h;
        launcher.site_pct_w = live.site_pct_w;
        launcher.site_pct_h = live.site_pct_h;
        launcher.last_location = live.last_location.clone();
        // 视图 / 排序 / 缩放也是面板直接写的：设置窗口拿着打开时的旧快照保存，
        // 会把刚滚出来的缩放冲回去。
        launcher.view = live.view.clone();
        launcher.sort_key = live.sort_key.clone();
        launcher.sort_dir = live.sort_dir;
        launcher.pinned_view = live.pinned_view.clone();
        launcher.pinned_sort_key = live.pinned_sort_key.clone();
        launcher.pinned_sort_dir = live.pinned_sort_dir;
        launcher.view_zoom = live.view_zoom;
        launcher.pinned_view_zoom = live.pinned_view_zoom;
        // 钉住的便签由后端自己维护（钉 / 取消 / 拖动），设置页拿的是旧快照，
        // 不能把它覆盖成「什么都没钉」。
        config.notes.pinned = shared.config.notes.pinned.clone();
        config.sync.last_push = shared.config.sync.last_push;
        config.sync.last_pull = shared.config.sync.last_pull;
        // 桌面格子的位置 / 尺寸 / 缩放 / 卷起都是窗口自己写的，设置页的旧快照
        // 不能把它们带回去；格子的增删也只走专属命令。
        hexbench_core::config::merge_desktop_from_settings(
            &mut config.desktop,
            &shared.config.desktop,
        );
    }
    config::save(&state.config_path, &config).map_err(|error| error.to_string())?;
    let (theme_changed, gestures_changed, gestures) = {
        let mut shared = state.shared.lock().unwrap();
        let theme_changed = shared.config.ui.theme != config.ui.theme;
        let gestures_changed = shared.config.launcher.gestures != config.launcher.gestures;
        let gestures = config.launcher.gestures.clone();
        shared.config = config;
        (theme_changed, gestures_changed, gestures)
    };
    // 常驻窗口（面板、网页、每个格子）不会重新加载页面，主题改了要
    // 当场推过去；否则设置窗口变了色，桌面格子还是原来那一套。
    if theme_changed {
        push_theme_to_windows(&app);
    }
    // 手势设置同样：格子一直开着，改开关 / 方向 / 距离要立刻生效。
    if gestures_changed {
        push_gestures_to_windows(&app, &gestures);
    }
    register_hotkeys(&app);
    // 面板打开着的时候也能看到路径/分类变化。
    let _ = app.emit("launcher-changed", ());

    Ok(())
}

#[tauri::command]
fn set_paused(paused: bool, app: AppHandle, state: State<'_, AppState>) {
    state.shared.lock().unwrap().paused = paused;
    sync_tray_checks(&app);
}

/// Re-register every global shortcut from the config.
///
/// Shortcuts are declared in one place (`config.hotkeys`) and dispatched here,
/// so adding a tool cannot create a second, conflicting registration.
fn register_hotkeys(app: &AppHandle) {
    let (pause, launcher) = {
        let state = app.state::<AppState>();
        let shared = state.shared.lock().unwrap();
        (
            shared.config.hotkeys.pause.clone(),
            shared.config.hotkeys.launcher.clone(),
        )
    };

    let manager = app.global_shortcut();
    let _ = manager.unregister_all();

    let mut error = None;
    for (label, combo) in [("暂停 / 继续", &pause), ("呼出面板", &launcher)] {
        if combo.trim().is_empty() {
            continue;
        }
        if let Err(reason) = manager.register(combo.as_str()) {
            log::warn!("注册热键「{label}」{combo} 失败：{reason}");
            error = Some(format!(
                "「{label}」{combo} 注册失败，可能已被其他程序占用，换一个组合键再试"
            ));
        }
    }
    app.state::<AppState>().shared.lock().unwrap().hotkey_error = error;
}

fn shortcut_matches(shortcut: &Shortcut, spec: &str) -> bool {
    use std::str::FromStr;
    Shortcut::from_str(spec)
        .map(|parsed| parsed == *shortcut)
        .unwrap_or(false)
}

/// 托盘「显示全部格子」：把配置里隐藏掉的格子一次性放回来。
fn show_all_fences(app: &AppHandle) {
    let state = app.state::<AppState>();
    let changed = {
        let mut shared = state.shared.lock().unwrap();
        let mut changed = false;
        for fence in shared.config.desktop.fences.iter_mut() {
            if !fence.visible {
                fence.visible = true;
                changed = true;
            }
        }
        changed
    };
    if !changed {
        return;
    }
    let config = state.shared.lock().unwrap().config.clone();
    if let Err(error) = config::save(&state.config_path, &config) {
        log::warn!("tray: 显示全部格子保存失败: {error}");
    }
    apply_fence_windows(app);
}

/// Toggle the pause flag from outside the UI (tray, global shortcut).
fn toggle_pause(app: &AppHandle) {
    {
        let state = app.state::<AppState>();
        let mut shared = state.shared.lock().unwrap();
        shared.paused = !shared.paused;
    }
    sync_tray_checks(app);
}

#[tauri::command]
fn refresh_layout(state: State<'_, AppState>) {
    state.shared.lock().unwrap().refresh_requested = true;
}

#[tauri::command]
fn set_autostart(enabled: bool, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let manager = app.autolaunch();
    let result = if enabled { manager.enable() } else { manager.disable() };
    result.map_err(|error| error.to_string())?;
    state.shared.lock().unwrap().autostart = enabled;
    Ok(())
}

#[tauri::command]
fn open_config_folder(state: State<'_, AppState>) {
    if let Some(dir) = state.config_path.parent() {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayRect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayPayload {
    bands: Vec<OverlayRect>,
    zones: Vec<OverlayRect>,
}

/// Trigger zones in physical pixels, relative to the top-left of the virtual
/// screen, plus the virtual screen bounds the overlay window has to cover.
fn overlay_geometry(
    monitors: &[MonitorDto],
    regions: &[RegionDto],
    ratio: f64,
    position: hexbench_core::model::BandPosition,
) -> Option<(i32, i32, u32, u32, OverlayPayload)> {
    let origin_x = monitors.iter().map(|m| m.x).min()?;
    let origin_y = monitors.iter().map(|m| m.y).min()?;
    let right = monitors.iter().map(|m| m.x + m.w).max()?;
    let bottom = monitors.iter().map(|m| m.y + m.h).max()?;

    // A 1px line would be invisible on a real screen; draw a narrow strip just
    // inside the edge, which is still exactly the area the cursor has to touch.
    const STRIP: i32 = 10;

    let bands = monitors
        .iter()
        .flat_map(|monitor| {
            // 只画工作区：任务栏不参与触发（引擎里由 exclude_taskbar 拦着），
            // 预览也别糊在任务栏上。区域本身按整块屏算，但任务栏那一段永远不会命中。
            let height = ((monitor.work_h as f64 * ratio).round() as i32).min(monitor.work_h);
            let top = monitor.work_y;
            let bottom = monitor.work_y + monitor.work_h - height;
            let ranges: Vec<(i32, i32)> = match position {
                hexbench_core::model::BandPosition::Bottom => vec![(bottom, monitor.work_y + monitor.work_h)],
                hexbench_core::model::BandPosition::Top => vec![(top, top + height)],
                hexbench_core::model::BandPosition::Both => {
                    if top + height >= bottom {
                        vec![(top, monitor.work_y + monitor.work_h)]
                    } else {
                        vec![(top, top + height), (bottom, monitor.work_y + monitor.work_h)]
                    }
                }
            };
            ranges.into_iter().map(move |(y0, y1)| OverlayRect {
                x: monitor.x - origin_x,
                y: y0 - origin_y,
                w: monitor.w,
                h: y1 - y0,
            })
        })
        .collect();

    let zones = regions
        .iter()
        .map(|region| {
            let x = match region.side {
                Side::Left => region.edge_x,
                Side::Right => region.edge_x - STRIP + 1,
            };
            OverlayRect {
                x: x - origin_x,
                y: region.y0 - origin_y,
                w: STRIP,
                h: region.y1 - region.y0,
            }
        })
        .collect();

    Some((
        origin_x,
        origin_y,
        (right - origin_x) as u32,
        (bottom - origin_y) as u32,
        OverlayPayload { bands, zones },
    ))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunningAppDto {
    name: String,
    title: String,
}

/// Visible applications, for the "add to blacklist" picker.
#[tauri::command]
fn running_apps() -> Vec<RunningAppDto> {
    hexbench_core::sys::window_apps()
        .into_iter()
        .filter(|entry| entry.name != "hexbench-app.exe" && entry.name != "hexbench.exe")
        .map(|entry| RunningAppDto { name: entry.name, title: entry.title })
        .collect()
}

#[tauri::command]
fn overlay_zones(state: State<'_, AppState>) -> OverlayPayload {
    let shared = state.shared.lock().unwrap();
    overlay_geometry(&shared.monitors, &shared.regions, shared.config.trigger.ratio, shared.config.trigger.position)
        .map(|(_, _, _, _, payload)| payload)
        .unwrap_or(OverlayPayload { bands: Vec::new(), zones: Vec::new() })
}


/// `报告.txt` 已存在时变成 `报告 (1).txt`，和资源管理器一致。
fn unique_target(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let mut candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let ext = path.extension().and_then(|value| value.to_str());
    for index in 1..1000 {
        let file = match ext {
            Some(ext) => format!("{stem} ({index}).{ext}"),
            None => format!("{stem} ({index})"),
        };
        candidate = dir.join(file);
        if !candidate.exists() {
            break;
        }
    }
    candidate
}

/// 把一批文件移动进某个目录：同盘是 rename（瞬间），跨盘退化成复制后删除。
/// 重名自动变成 `文件 (1).txt`，和资源管理器一致。
fn move_paths_into(dir: &std::path::Path, paths: &[std::path::PathBuf]) -> usize {
    if std::fs::create_dir_all(dir).is_err() {
        log::warn!("move: cannot create {}", dir.display());
        return 0;
    }
    // 已经在目标目录里 / 把自己拖进自己的，先剔掉。
    let sources: Vec<std::path::PathBuf> = paths
        .iter()
        .filter(|path| path.parent() != Some(dir) && path.as_path() != dir)
        .cloned()
        .collect();
    if sources.is_empty() {
        return 0;
    }
    // 交给 Shell：跨盘、进度条、重名询问、长路径都由它处理。
    match hexbench_core::shell_ops::move_into(&sources, dir) {
        Ok(count) => return count,
        Err(error) => log::warn!("move: shell 失败（{error}），退回手写逻辑"),
    }
    let mut moved = 0;
    for source in paths {
        // 已经在目标目录里：不动，否则会复制成「文件 (1).txt」。
        // 也不能把自己拖进自己。
        if source.parent() == Some(dir) || source == dir {
            continue;
        }
        let Some(name) = source.file_name() else {
            continue;
        };
        let target = unique_target(dir, &name.to_string_lossy());
        let ok = std::fs::rename(source, &target).is_ok() || {
            let copied = source.is_file() && std::fs::copy(source, &target).is_ok();
            if copied {
                let _ = std::fs::remove_file(source);
            }
            copied
        };
        if ok {
            moved += 1;
        } else {
            log::warn!("move: could not take {}", source.display());
        }
    }
    moved
}


fn open_overlay(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (monitors, regions, trigger) = {
        let shared = state.shared.lock().unwrap();
        (
            shared.monitors.clone(),
            shared.regions.clone(),
            shared.config.trigger.clone(),
        )
    };

    let Some((origin_x, origin_y, width, height, payload)) =
        overlay_geometry(&monitors, &regions, trigger.ratio, trigger.position)
    else {
        log::warn!("overlay: no monitors yet");
        return Err("还没读到显示器，点「刷新」再试".to_string());
    };

    let overlay = match app.get_webview_window("overlay") {
        Some(window) => window,
        None => WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
            .title("HexBench")
            .additional_browser_args(WEBVIEW_ARGS)
            .decorations(false)
        .zoom_hotkeys_enabled(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .shadow(false)
            .resizable(false)
            .focused(false)
            .visible(false)
            .build()
            .map_err(|error| error.to_string())?,
    };

    if let Err(error) = overlay.set_position(PhysicalPosition::new(origin_x, origin_y)) {
        log::warn!("overlay: set_position failed: {error}");
        return Err(error.to_string());
    }
    if let Err(error) = overlay.set_size(PhysicalSize::new(width, height)) {
        log::warn!("overlay: set_size failed: {error}");
        return Err(error.to_string());
    }
    if let Err(error) = overlay.set_ignore_cursor_events(true) {
        log::warn!("overlay: set_ignore_cursor_events failed: {error}");
        return Err(error.to_string());
    }
    if let Err(error) = overlay.show() {
        log::warn!("overlay: show failed: {error}");
        return Err(error.to_string());
    }
    log::info!("overlay: shown at {origin_x},{origin_y} {width}x{height} ({} bands, {} zones)", payload.bands.len(), payload.zones.len());
    if let Err(error) = overlay.emit("overlay-zones", payload) {
        // 交不过去就别把它晾在屏幕上。
        let _ = overlay.hide();
        return Err(error.to_string());
    }

    // 每次预览一个编号：5 秒内又点了一次，让后一次自己收尾，前一次的定时器
    // 不能把刚显示的预览关掉。
    let generation = OVERLAY_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = overlay.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(5000));
        if OVERLAY_GEN.load(Ordering::SeqCst) != generation {
            return;
        }
        let _ = handle.hide();
    });

    Ok(())
}

#[tauri::command]
fn show_trigger_overlay(app: AppHandle) -> Result<(), String> {
    // Building a WebView2 window re-enters the event loop: doing it on the
    // command thread deadlocks the window creation, which is why the overlay
    // used to appear as an invisible 800x600 window and nothing else.
    thread::spawn(move || {
        if let Err(error) = open_overlay(&app) {
            log::warn!("overlay: {error}");
        }
    });
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PickedApp {
    name: String,
    title: String,
    class: String,
}

const SYNC_FILE: &str = "hexbench-config.json";

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn sync_client(state: &AppState) -> Result<hexbench_core::sync::WebDav, String> {
    let (url, username, sealed) = {
        let shared = state.shared.lock().unwrap();
        let settings = &shared.config.sync;
        (
            settings.url.clone(),
            settings.username.clone(),
            settings.password.clone(),
        )
    };
    if url.trim().is_empty() {
        return Err("先填写 WebDAV 地址".to_string());
    }
    let password = hexbench_core::sync::open_secret(&sealed)
        .ok_or_else(|| "保存的密码读不出来，请重新填写".to_string())?;
    Ok(hexbench_core::sync::WebDav::new(&url, &username, &password))
}

#[tauri::command(async)]
fn sync_test(
    url: String,
    username: String,
    password: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // An empty field means "use the saved password".
    let password = if password.is_empty() {
        let sealed = state.shared.lock().unwrap().config.sync.password.clone();
        hexbench_core::sync::open_secret(&sealed).unwrap_or_default()
    } else {
        password
    };
    hexbench_core::sync::WebDav::new(&url, &username, &password).test()
}

#[tauri::command]
fn sync_save(
    url: String,
    username: String,
    password: String,
    enabled: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let config = {
        let mut shared = state.shared.lock().unwrap();
        let settings = &mut shared.config.sync;
        settings.url = url;
        settings.username = username;
        if !password.is_empty() {
            settings.password = hexbench_core::sync::seal_secret(&password);
        }
        settings.enabled = enabled;
        shared.config.clone()
    };
    config::save(&state.config_path, &config).map_err(|error| error.to_string())?;
    // 这些字段是后端直接写的：告诉设置窗口，别让它下次保存时用旧值盖回去。
    let _ = app.emit("config-changed", ());
    Ok(())
}

#[tauri::command(async)]
fn sync_push(state: State<'_, AppState>) -> Result<String, String> {
    let client = sync_client(state.inner())?;
    let mut config = state.shared.lock().unwrap().config.clone();
    // Credentials are machine-bound; never upload them.
    config.sync.password.clear();
    let body = serde_json::to_string_pretty(&config).map_err(|error| error.to_string())?;
    client.put(SYNC_FILE, &body)?;

    let config = {
        let mut shared = state.shared.lock().unwrap();
        shared.config.sync.last_push = now_secs();
        shared.config.clone()
    };
    let _ = config::save(&state.config_path, &config);
    Ok("已上传".to_string())
}

#[tauri::command(async)]
fn sync_pull(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let client = sync_client(state.inner())?;
    let Some(text) = client.get(SYNC_FILE)? else {
        return Err("远程还没有配置文件，先上传一次".to_string());
    };
    let mut remote: Config =
        serde_json::from_str(&text).map_err(|error| format!("远程文件不是有效配置：{error}"))?;

    {
        let mut shared = state.shared.lock().unwrap();
        remote.sync.url = shared.config.sync.url.clone();
        remote.sync.username = shared.config.sync.username.clone();
        remote.sync.password = shared.config.sync.password.clone();
        remote.sync.enabled = shared.config.sync.enabled;
        remote.sync.last_push = shared.config.sync.last_push;
        remote.sync.last_pull = now_secs();
        // 桌面格子：格子列表取并集，几何 / 视图留在本机；总开关也以本机为准。
        hexbench_core::config::merge_desktop_from_sync(&mut remote.desktop, &shared.config.desktop);
        remote.desktop.enabled = shared.config.desktop.enabled;
        remote.desktop.keep_on_show_desktop = shared.config.desktop.keep_on_show_desktop;
        config::save(&state.config_path, &remote).map_err(|error| error.to_string())?;
        shared.config = remote;
    }
    let _ = app.emit("config-changed", ());
    Ok("已拉取并应用".to_string())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateInfoDto {
    version: String,
    notes: String,
    date: String,
}

#[derive(Clone, Serialize)]
struct UpdateProgress {
    chunk: usize,
    total: Option<u64>,
}

fn build_updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let endpoint = {
        let state = app.state::<AppState>();
        let shared = state.shared.lock().unwrap();
        shared.config.updates.endpoint.clone()
    };

    let mut builder = app.updater_builder();
    if !endpoint.trim().is_empty() {
        let url: tauri::Url = endpoint
            .trim()
            .parse()
            .map_err(|error| format!("更新地址无效：{error}"))?;
        builder = builder
            .endpoints(vec![url])
            .map_err(|error| error.to_string())?;
    }
    builder.build().map_err(|error| error.to_string())
}

#[tauri::command]
async fn update_check(app: AppHandle) -> Result<Option<UpdateInfoDto>, String> {
    let result = check_once(&app).await;

    let state = app.state::<AppState>();
    let snapshot = {
        let mut status = state.update_status.lock().unwrap();
        status.checked_at = now_secs();
        match &result {
            Ok(info) => {
                status.version = info.as_ref().map(|info| info.version.clone());
                status.notes = info.as_ref().map(|info| info.notes.clone());
                status.date = info.as_ref().map(|info| info.date.clone());
                status.error = None;
            }
            Err(error) => {
                status.version = None;
                status.notes = None;
                status.date = None;
                status.error = Some(error.clone());
            }
        }
        status.clone()
    };
    if let Some(version) = &snapshot.version {
        log::info!("update: {version} available");
    }
    let _ = app.emit("update-status", snapshot);

    result
}

async fn check_once(app: &AppHandle) -> Result<Option<UpdateInfoDto>, String> {
    let updater = build_updater(app)?;
    let update = updater.check().await.map_err(|error| error.to_string())?;
    Ok(update.map(|update| UpdateInfoDto {
        version: update.version.clone(),
        notes: update.body.clone().unwrap_or_default(),
        date: update.date.map(|date| date.to_string()).unwrap_or_default(),
    }))
}

#[tauri::command]
fn update_status(state: State<'_, AppState>) -> UpdateStatus {
    state.update_status.lock().unwrap().clone()
}

#[tauri::command]
async fn update_download(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateInfoDto, String> {
    let updater = build_updater(&app)?;
    let Some(update) = updater.check().await.map_err(|error| error.to_string())? else {
        return Err("没有可用的更新".to_string());
    };

    let progress_app = app.clone();
    let bytes = update
        .download(
            move |chunk, total| {
                let _ = progress_app.emit("update-progress", UpdateProgress { chunk, total });
            },
            || {},
        )
        .await
        .map_err(|error| error.to_string())?;

    let info = UpdateInfoDto {
        version: update.version.clone(),
        notes: update.body.clone().unwrap_or_default(),
        date: update.date.map(|date| date.to_string()).unwrap_or_default(),
    };
    *state.pending_update.lock().unwrap() = Some(bytes);
    Ok(info)
}

#[tauri::command]
async fn update_install(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let bytes = state
        .pending_update
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "还没有下载更新包".to_string())?;

    let updater = build_updater(&app)?;
    let Some(update) = updater.check().await.map_err(|error| error.to_string())? else {
        return Err("没有可用的更新".to_string());
    };
    // Verifies the minisign signature before handing the package to NSIS.
    update.install(bytes).map_err(|error| error.to_string())
}

/// Let the user click any window to pick it.
#[tauri::command]
fn pick_app(app: AppHandle) {
    thread::spawn(move || {
        // The click that invoked this command is still held down; wait for the
        // release so the pick is a separate press.
        while hexbench_core::sys::mouse_button_down() {
            thread::sleep(Duration::from_millis(30));
        }
        for _ in 0..800 {
            if hexbench_core::sys::mouse_button_down() {
                thread::sleep(Duration::from_millis(70));
                if let Some(info) = hexbench_core::sys::window_under_cursor() {
                    let _ = app.emit(
                        "app-picked",
                        PickedApp {
                            name: info.process,
                            title: info.title,
                            class: info.class,
                        },
                    );
                }
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
    });
}


/// Quit for real: clear the screen, then exit hard so a slow WebView2
/// teardown cannot look like a frozen app.
fn request_quit(app: &AppHandle) {
    QUITTING.store(true, Ordering::SeqCst);
    hide_all_windows(app);
    thread::spawn(|| {
        // Safety net in case the hard exit below never runs.
        thread::spawn(|| {
            thread::sleep(Duration::from_secs(3));
            std::process::exit(0);
        });
        thread::sleep(Duration::from_millis(80));
        log::info!("quit: exiting");
        log::logger().flush();
        std::process::exit(0);
    });
}

/// Hide every window we own, cheapest way to clear the screen before exiting.
fn hide_all_windows(app: &AppHandle) {
    for label in ["main", "launcher", "overlay"] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
    }
    if let Some(host) = NATIVE_FENCES.lock().ok().and_then(|slot| slot.clone()) {
        host.sync(Vec::new());
    }
}

/// 主线程（或正在等主线程的调用方）当前进行到哪一步窗口操作。
///
/// 窗口 API 从别的线程调用时是「排队给主线程 + 阻塞等」的，所以主线程一旦卡在
/// 某个调用里，光看日志只能知道上一步成了、这一步没成。这个标记由调用方在
/// 进调用之前置上，看门狗发现主线程不应答时把它打出来，就能定位到具体调用。
static MAIN_OP: AtomicU32 = AtomicU32::new(0);

fn op(id: u32) {
    MAIN_OP.store(id, Ordering::Relaxed);
}

fn op_name(id: u32) -> &'static str {
    match id {
        0 => "idle",
        1 => "place_launcher",
        2 => "launcher.show",
        3 => "push_theme",
        4 => "web.place",
        5 => "web.show",
        6 => "launcher.set_focus",
        7 => "sync_web_side",
        8 => "site.place+show",
        9 => "emit launcher-shown",
        10 => "store_panel_size",
        11 => "launcher.hide",
        12 => "store_web_geometry",
        13 => "web.hide",
        14 => "close_site_window",
        15 => "read config",
        16 => "web.sync_web_side(hide)",
        17 => "force_foreground",
        _ => "?",
    }
}

/// Note how long the event loop is unavailable, so a "frozen" report can be
/// traced instead of guessed at.
fn spawn_stall_watchdog(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(1000));
        let (tx, rx) = std::sync::mpsc::channel();
        let started = Instant::now();
        if app
            .run_on_main_thread(move || {
                let _ = tx.send(());
            })
            .is_ok()
        {
            let _ = rx.recv_timeout(Duration::from_secs(5));
        }
        let waited = started.elapsed();
        if waited > Duration::from_millis(400) {
            log::warn!(
                "ui: main thread busy {} ms (stuck in {})",
                waited.as_millis(),
                op_name(MAIN_OP.load(Ordering::Relaxed))
            );
        }
    });
}


/// Start moving the window from the custom title bar.
#[tauri::command]
fn begin_drag(window: tauri::WebviewWindow) {
    if let Ok(hwnd) = window.hwnd() {
        hexbench_core::sys::start_manual_drag(hwnd.0 as isize);
    }
}

/// Just the trigger state, for the live highlight in the layout diagram.
/// The full snapshot is too heavy to drive that at 12 fps.
#[tauri::command]
fn hit_state(state: State<'_, AppState>) -> (Option<HitDto>, Option<String>, String) {
    let shared = state.shared.lock().unwrap();
    (
        shared.hit.map(|hit| HitDto {
            monitor: hit.monitor,
            side: hit.side,
            edge_x: hit.edge_x,
        }),
        shared.guard.clone(),
        shared.engine_phase.clone(),
    )
}

#[tauri::command]
fn create_virtual_desktop(state: State<'_, AppState>) -> Result<(), String> {
    let origin = hexbench_core::sys::virtual_desktop_state().map(|(_, index)| index);
    if !hexbench_core::sys::create_virtual_desktop() {
        return Err("发送快捷键失败：前台可能是管理员窗口".to_string());
    }

    // Windows appends the new desktop to the end of the strip and switches to
    // it, so "come back" is not always one step to the left: it is as many steps
    // as there are desktops to the right of where the user was.
    let shared = Arc::clone(&state.shared);
    std::thread::spawn(move || {
        let mut steps = None;
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(250));
            let Some((count, index)) = hexbench_core::sys::virtual_desktop_state() else {
                continue;
            };
            let Some(origin_index) = origin else { break };
            if index != origin_index && count >= origin_index {
                // Appended at the end → walk back the whole tail; if some other
                // build inserts next to the origin, one step is enough.
                steps = Some(if index == count {
                    count - origin_index
                } else {
                    1
                });
                break;
            }
        }

        if let Some(steps) = steps {
            std::thread::sleep(Duration::from_millis(250));
            for _ in 0..steps {
                let _ = hexbench_core::sys::send_desktop_switch(Direction::Prev);
                std::thread::sleep(Duration::from_millis(320));
            }
        }

        if let Some(count) = hexbench_core::sys::virtual_desktop_count() {
            shared.lock().unwrap().desktop_count = Some(count);
        }
    });

    Ok(())
}

fn open_main(app: &AppHandle) {
    // Same rule as the launcher panel: never build a webview inside an
    // event-loop callback (tray menu, single-instance hand-off), it re-enters
    // the loop and deadlocks. A worker thread is safe.
    let handle = app.clone();
    thread::spawn(move || open_main_now(&handle));
}

/// Refresh the update status when the window is opened, at most every couple of
/// minutes: a release published a minute ago should not wait for the timer.
fn check_update_if_stale(app: &AppHandle) {
    let stale = {
        let state = app.state::<AppState>();
        let status = state.update_status.lock().unwrap();
        status.checked_at > 0 && now_secs() - status.checked_at > 120
    };
    if !stale {
        return;
    }
    let handle = app.clone();
    thread::spawn(move || {
        let _ = tauri::async_runtime::block_on(update_check(handle));
    });
}

/// 光标所在显示器的缩放比例（物理像素 ÷ 逻辑像素）。
///
/// `work_area_at` 返回的是**物理**像素，而窗口的尺寸 / 位置接口按**逻辑**值
/// 使用；两边不换算，125% / 150% 缩放下算出来的位置会跑到屏幕外，或者面板
/// 一次比一次小。
fn scale_at_cursor(app: &AppHandle) -> f64 {
    let (x, y) = hexbench_core::sys::cursor_position();
    app.monitor_from_point(x as f64, y as f64)
        .ok()
        .flatten()
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(1.0)
}

/// 设置窗口每显示一次就加一。关窗后延迟回收的线程拿它判断这中间是不是又打开过，
/// 免得恰好在回收那一刻打开的窗口被销毁掉。
static MAIN_SHOW_GEN: AtomicU64 = AtomicU64::new(0);

fn open_main_now(app: &AppHandle) {
    check_update_if_stale(app);
    if let Some(window) = app.get_webview_window("main") {
        MAIN_SHOW_GEN.fetch_add(1, Ordering::SeqCst);
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
            return;
    }

    // 设置窗口宽度占工作区六成，高度给到七成二：设置页多是长表单，
    // 高度太省会把标签栏和预览都挤到折叠线以下。下限抬到 600，
    // 保证 125%/150% 缩放下也能露出一屏有效内容。
    // 工作区是物理像素，窗口尺寸要逻辑值，先按这块屏的缩放换算。
    let (cursor_x, cursor_y) = hexbench_core::sys::cursor_position();
    let scale = scale_at_cursor(app);
    let (width, height) = match hexbench_core::sys::work_area_at(cursor_x, cursor_y) {
        Some((left, top, right, bottom)) => (
            (((right - left) * 60 / 100) as f64 / scale).max(840.0),
            (((bottom - top) * 72 / 100) as f64 / scale).max(600.0),
        ),
        None => (980.0, 660.0),
    };
    match WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        // 任务栏和 Alt+Tab 里显示的名字：跟面板、格子区分开。
        .title("HexBench 设置")
        .additional_browser_args(WEBVIEW_ARGS)
        .inner_size(width, height)
        .min_inner_size(840.0, 600.0)
        // The app draws its own title bar and window buttons.
        .decorations(false)
        .zoom_hotkeys_enabled(false)
        .center()
        .build()
    {
        Ok(_) => {
            MAIN_SHOW_GEN.fetch_add(1, Ordering::SeqCst);
        }
        Err(error) => {
            // 打不开设置窗口时不能只写日志：从托盘 / 热键点进来的用户会以为什么都没发生。
            log::error!("main window: build failed: {error}");
            use tauri_plugin_dialog::DialogExt;
            app.dialog()
                .message(format!("打不开设置窗口：{error}"))
                .title("HexBench")
                .show(|_| {});
        }
    }
}

/// 呼出面板：一个窗口装「分类侧栏 + 文件区」。
///
/// 懒创建、之后常驻隐藏：呼出要的是「按一下就出」，重建 WebView2 要几百毫秒，
/// 手感立刻垮掉。第一次呼出慢一点，之后只是一次 show。
/// 拖拽一开始就把文件列表记下来：光标进入面板后 OLE 就不再发事件了，
/// 只能自己盯着光标，松手时若还在面板里就当成落在这里（光标拖出去再拖回来也一样）。
static DRAG_WATCHING: AtomicBool = AtomicBool::new(false);
/// 拖拽被取消过（按 Esc）：系统会在光标**还在面板里**的时候发一次 DragLeave。
/// 不记下来的话，接着松开左键会被盯光标的线程当成有效落点，文件在用户已经
/// 取消的情况下真的被移走。
static DRAG_CANCELLED: AtomicBool = AtomicBool::new(false);

/// 光标此刻是否落在可见的面板里（物理像素比较）。
fn cursor_inside_launcher(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window("launcher") else {
        return false;
    };
    if !panel_visible(&window).unwrap_or(false) {
        return false;
    }
    let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) else {
        return false;
    };
    let (x, y) = hexbench_core::sys::cursor_position();
    x >= position.x
        && y >= position.y
        && x < position.x + size.width as i32
        && y < position.y + size.height as i32
}

/// 外部拖拽期间盯光标：自己每 25ms 看一次光标在哪，不靠系统的拖放事件。
///
/// 系统只在光标进出窗口时各发一次事件，光标拖出面板再拖回来时这条路上
/// 不一定齐全；从面板自己的拖放目标收到「拖进来了」就开始盯，直到松手，
/// 界面才能一直跟着光标切换、也才落得准。
fn watch_external_drag(app: AppHandle, paths: Vec<String>) {
    if DRAG_WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    DRAG_CANCELLED.store(false, Ordering::SeqCst);
    // 和面板内部的拖拽一样，交给界面的坐标用客户区坐标。
    let hwnd = app
        .get_webview_window("launcher")
        .and_then(|window| window.hwnd().ok())
        .map(|handle| handle.0 as isize);
    thread::spawn(move || {
        let mut last_inside = false;
        loop {
            // 25ms 一次：拖拽时界面靠它判断指着哪一行，慢了看着就像「跟不上手」。
            thread::sleep(Duration::from_millis(25));
            let (x, y) = hexbench_core::sys::cursor_position();
            let (client_x, client_y) = match hwnd {
                Some(hwnd) => hexbench_core::sys::screen_to_client(hwnd, x, y),
                None => (x, y),
            };
            let frame = app.get_webview_window("launcher").and_then(|window| {
                if !window.is_visible().unwrap_or(false) {
                    return None;
                }
                let position = window.outer_position().ok()?;
                let size = window.inner_size().ok()?;
                Some((
                    position.x,
                    position.y,
                    size.width as i32,
                    size.height as i32,
                ))
            });
            let inside = match frame {
                Some((left, top, width, height)) => {
                    x >= left && y >= top && x < left + width && y < top + height
                }
                None => false,
            };
            if inside != last_inside {
                last_inside = inside;
                let _ = if inside {
                    app.emit(
                        "launcher-drag-pos",
                        serde_json::json!({ "x": client_x, "y": client_y }),
                    )
                } else {
                    app.emit("launcher-drag-leave", ())
                };
            } else if inside {
                let _ = app.emit(
                    "launcher-drag-pos",
                    serde_json::json!({ "x": client_x, "y": client_y }),
                );
            }
            if !hexbench_core::sys::left_button_down() {
                // 松手：还在面板里就是落在这里了（没有文件列表（拖的不是文件）
                // 就只收起落点提示）。取消过的拖拽（Esc）不算落点——那时系统
                // 已经发过 DragLeave、光标却还在面板里，再照常落下去就是把
                // 用户取消掉的文件移走。
                let cancelled = DRAG_CANCELLED.swap(false, Ordering::SeqCst);
                if inside && !paths.is_empty() && !cancelled {
                    let _ = app.emit(
                        "launcher-dropped",
                        serde_json::json!({
                            "x": client_x,
                            "y": client_y,
                            "paths": paths,
                            "copy": hexbench_core::sys::ctrl_down(),
                        }),
                    );
                } else {
                    let _ = app.emit("launcher-drag-leave", ());
                }
                break;
            }
        }
        DRAG_WATCHING.store(false, Ordering::SeqCst);
    });
}
fn build_launcher(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    let started = Instant::now();
    // 这个窗口可能是在别的线程上创建的，拖放目标注册要求那线程也初始化过 OLE，
    // 否则外部拖进来的文件会被系统交给主窗口，而不是这里。
    hexbench_core::sys::init_ole();
    // 新窗口就是全新的前端状态（网页没了、视图回到文件），
    // 竖版标志必须跟着清掉，否则会出现「竖版空壳」。
    LAUNCHER_WEB_MODE.store(false, Ordering::Relaxed);
    let built = WebviewWindowBuilder::new(app, "launcher", WebviewUrl::App("launcher.html".into()))
        .title("HexBench 快捷面板")
        .additional_browser_args(WEBVIEW_ARGS)
        .inner_size(780.0, 500.0)
        .min_inner_size(520.0, 320.0)
        .decorations(false)
        .resizable(true)
        .zoom_hotkeys_enabled(false)
        // 透明窗口 + 改尺寸是已知坏组合（issue #13906、PR #16192）：
        // 页面会错位/溢出，窗口尺寸也可能不生效。圆角改用 DWM 设置。
        .transparent(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|error| {
            log::warn!("launcher: build failed: {error}");
            error
        })
        .ok();
    if let Some(window) = &built {
        // Tauri 只在先建的那个 webview 上登记拖放目标，面板是后建的，得自己挂一个，
        // 否则外部拖进来的文件系统根本不会通知我们。
        match window.hwnd() {
            Ok(hwnd) => {
                // 面板是工具窗口，系统默认不给圆角；显式要一个和设置窗口一样的。
                hexbench_core::sys::round_window_corners(hwnd.0 as isize);
                let app = app.clone();
                let late_app = app.clone();
                let main_app = app.clone();
                let raw = hwnd.0 as isize;
                // OLE 的拖放回调只发到「注册目标的那条线程」，而那条线程必须有消息循环。
                // 面板窗口属于主线程，这里却站在建窗的 worker 线程上；直接注册的话，
                // 资源管理器拖进来时 DragEnter 会一直等一条从不泵消息的线程——表现就是
                // 鼠标拖着文件悬在面板上，整个拖拽卡死。所以注册排到主线程去做。
                let _ = main_app.run_on_main_thread(move || {
                    let attached = hexbench_core::drop_target::attach(
                        raw,
                        Box::new(move |event| {
                            use hexbench_core::drop_target::DropEvent;
                            match event {
                                DropEvent::Enter { paths, x, y, .. } => {
                                    // 拖拽期间自己盯光标，不靠系统的进出事件：光标拖出面板
                                    // 再拖回来，这条路上事件不一定齐全（离开发了、回来没发），
                                    // 界面就会一直停在上一次的位置。进了面板就开始盯，
                                    // 松手时它自己收尾。
                                    let list: Vec<String> = paths
                                        .iter()
                                        .map(|path| path.to_string_lossy().to_string())
                                        .collect();
                                    log::info!("launcher: drag entered with {} item(s)", list.len());
                                    watch_external_drag(app.clone(), list);
                                    let _ = app.emit(
                                        "launcher-drag-pos",
                                        serde_json::json!({ "x": x, "y": y }),
                                    );
                                }
                                DropEvent::Over { x, y, .. } => {
                                    let _ = app.emit(
                                        "launcher-drag-pos",
                                        serde_json::json!({ "x": x, "y": y }),
                                    );
                                }
                                DropEvent::Leave => {
                                    // 光标还在面板里就收到 Leave：按 Esc 取消了拖拽。
                                    if cursor_inside_launcher(&app) {
                                        DRAG_CANCELLED.store(true, Ordering::SeqCst);
                                    }
                                    let _ = app.emit("launcher-drag-leave", ());
                                }
                                DropEvent::Drop { paths, x, y, keys } => {
                                    let list: Vec<String> = paths
                                        .iter()
                                        .map(|path| path.to_string_lossy().to_string())
                                        .collect();
                                    log::info!("launcher: dropped {} item(s)", list.len());
                                    let _ = app.emit(
                                        "launcher-dropped",
                                        serde_json::json!({
                                            "x": x,
                                            "y": y,
                                            "paths": list,
                                            // MK_CONTROL = 0x0008：按住 Ctrl 是复制。
                                            "copy": keys & 0x0008 != 0,
                                        }),
                                    );
                                }
                            }
                            // 面板里的落点由网页那侧决定，效果用默认的复制。
                            None
                        }),
                    );
                    match attached {
                        Ok(()) => log::info!("launcher: drop target registered"),
                        Err(error) => log::warn!("launcher: drop target failed: {error}"),
                    }
                });
                // WebView2 的子窗口是异步创建的：稍后再扫一遍补挂（同样在主线程上）。
                let late = late_app;
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(900));
                    let retry = late.clone();
                    let _ = late.run_on_main_thread(move || {
                        if let Some(window) = retry.get_webview_window("launcher") {
                            if let Ok(hwnd) = window.hwnd() {
                                match hexbench_core::drop_target::attach_more(hwnd.0 as isize) {
                                    Ok(()) => log::info!("launcher: drop target re-scanned"),
                                    Err(error) => {
                                        log::warn!("launcher: re-scan failed: {error}")
                                    }
                                }
                            }
                        }
                    });
                });
            }
            Err(error) => log::warn!("launcher: no hwnd: {error}"),
        }
    }
    // 面板缩放时，网页窗口里的子 WebView 会错位（表现为半边空白）。
    // 缩放结束后按网页窗口当前尺寸重排一次。
    if let Some(window) = &built {
        let probe = window.clone();
        window.on_window_event(move |event| {
            if !matches!(event, tauri::WindowEvent::Resized(_)) {
                return;
            }
            let handle = probe.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(220));
                let app = handle.app_handle();
                if let Some(web) = web_window_handle() {
                    if web.is_visible().unwrap_or(false) {
                        let scale = web.scale_factor().unwrap_or(1.0);
                        if let Ok(size) = web.inner_size() {
                            reflow_site_page(
                                app,
                                size.width as f64 / scale,
                                size.height as f64 / scale,
                            );
                        }
                    }
                }
            });
        });
    }

    match built {
        Some(_) => log::info!("launcher: built in {} ms", started.elapsed().as_millis()),
        None => log::warn!("launcher: build failed"),
    }
    built
}


/// 面板出现在鼠标所在那块屏幕：水平居中、垂直偏上，就是命令面板该在的位置。
fn place_launcher(window: &tauri::WebviewWindow, follow_cursor: bool, size_percent: (u32, u32)) {
    let (cursor_x, cursor_y) = hexbench_core::sys::cursor_position();
    let area = hexbench_core::sys::work_area_at(cursor_x, cursor_y);
    // 拖过就按拖出来的尺寸；否则按百分比。
    let saved_size = {
        let state = window.state::<AppState>();
        let shared = state.shared.lock().unwrap();
        let launcher = &shared.config.launcher;
        (launcher.panel_remember_size
            && launcher.panel_w > 0
            && launcher.panel_pct_w == launcher.width_percent.clamp(10, 100)
            && launcher.panel_pct_h == launcher.height_percent.clamp(10, 100))
            .then_some((launcher.panel_w, launcher.panel_h))
    };

    // 尺寸按工作区的百分比算，换显示器也不会一下子占满或小得没法用。
    let (width, height) = match saved_size {
        // 记下来的尺寸是逻辑像素（store_panel_size 里除过缩放）；下面是
        // PhysicalSize，先乘回这块屏的缩放。以前直接当物理值用，125% / 150%
        // 下每呼出一次就小一圈，最后缩到最小尺寸。
        Some((saved_w, saved_h)) => {
            let scale = scale_at_cursor(window.app_handle());
            (
                (saved_w as f64 * scale).round() as i32,
                (saved_h as f64 * scale).round() as i32,
            )
        }
        None => match area {
        Some((left, top, right, bottom)) => {
            let width = ((right - left) * size_percent.0 as i32 / 100).clamp(520, 1400);
            let height = ((bottom - top) * size_percent.1 as i32 / 100).clamp(320, 1100);
            (width, height)
        }
        None => (780, 500),
        },
    };
    let _ = window.set_size(tauri::PhysicalSize::new(
        width as u32,
        height as u32,
    ));
    let (mut x, mut y) = match area {
        Some(_) if follow_cursor => {
            // 贴着鼠标：面板中心落在光标上。
            (cursor_x - width / 2, cursor_y - height / 2)
        }
        Some((left, top, right, bottom)) => (
            left + ((right - left) - width) / 2,
            top + ((bottom - top) - height) / 3,
        ),
        None => (cursor_x - width / 2, cursor_y - height / 2),
    };
    // 无论哪种模式，都不让它跑出工作区。
    if let Some((left, top, right, bottom)) = area {
        x = x.clamp(left, (right - width).max(left));
        y = y.clamp(top, (bottom - height).max(top));
    }
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

/// 面板显示时刻（毫秒）。「失焦即隐藏」需要一个宽限期：show 和抢焦点都是
/// 异步的，刚显示那一刻还没拿到焦点，失焦事件会先到，窗口就会被立刻收起。
static LAUNCHER_SHOWN_AT: AtomicU64 = AtomicU64::new(0);

/// 右键菜单弹出时面板会短暂失去焦点，那期间不能自动收起。
static LAUNCHER_MENU_OPEN: AtomicBool = AtomicBool::new(false);
/// 触发区预览（overlay）的编号：只有最新一次预览的定时器有权收起它。
static OVERLAY_GEN: AtomicU64 = AtomicU64::new(0);

/// 面板现在浏览到哪个文件夹：中转站可以进子目录，拖进来的文件就放进这里。
static LAUNCHER_DROP_TARGET: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// 呼出面板之前的前台窗口。收藏里的「发送」要收起面板、把焦点还回去再输入文字，
/// 所以只能在**面板显示之前**记：那一刻前台窗口才是用户真正想输入的地方。
static LAUNCHER_RETURN: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);

/// 记下「呼出面板前的前台窗口」。自己的窗口（面板、内置网页）不能当目标，
/// 否则发送时会把文字打回面板自己。
fn capture_launcher_return() {
    let usable = hexbench_core::sys::foreground_hwnd().filter(|hwnd| {
        hexbench_core::sys::window_valid(*hwnd)
            && hexbench_core::sys::window_visible(*hwnd)
            && !hexbench_core::sys::hwnd_is_current_process(*hwnd)
    });
    log::debug!("launcher: return target = {usable:?}");
    if let Ok(mut slot) = LAUNCHER_RETURN.lock() {
        *slot = usable;
    }
}

/// 配置是在日志系统起来之前读的，来源先存着，等 setup 里再报。
static LOAD_SOURCE: std::sync::OnceLock<hexbench_core::config::LoadSource> = std::sync::OnceLock::new();

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 面板的呼出外观：是否贴着鼠标 + 尺寸百分比。
fn launcher_appearance(app: &AppHandle) -> (bool, (u32, u32)) {
    let shared = app.state::<AppState>();
    let shared = shared.shared.lock().unwrap();
    let launcher = &shared.config.launcher;
    (
        launcher.follow_cursor,
        (
            launcher.width_percent.clamp(25, 80),
            launcher.height_percent.clamp(30, 80),
        ),
    )
}
/// 面板**真实**的可见状态（直接问 Win32）。
///
/// tao 内部缓存了一份 VISIBLE 标志，`is_visible()` 读的是那份缓存。只要有人在
/// 它背后改过窗口显示状态，缓存就和真实窗口对不上，之后它的 `show()`/`hide()`
/// 会因为「标志没变化」直接空转：按热键毫无反应、面板再也呼不出来。
fn panel_visible(window: &tauri::WebviewWindow) -> Option<bool> {
    window
        .hwnd()
        .ok()
        .map(|hwnd| hexbench_core::sys::window_visible(hwnd.0 as isize))
}

fn reveal_launcher(window: &tauri::WebviewWindow, appearance: (bool, (u32, u32))) {
    let started = Instant::now();
    op(1);
    place_launcher(window, appearance.0, appearance.1);
    op(0);
    LAUNCHER_SHOWN_AT.store(now_millis(), Ordering::Relaxed);
    // 网页模式下窗口是竖版的：这时候再按配置的 50%×50% 摆一次，
    // 网页仍按竖版矩形挂着，两边尺寸对不上就是「排版错位」。
    op(2);
    let _ = window.show();
    op(0);
    let after_show = started.elapsed();
    // show() 可能是空转的（tao 的可见性缓存被别的代码弄乱了）。这里核对
    // 真实窗口状态，没出来就直接显示一次，把真实状态和缓存重新对齐。
    if panel_visible(window) == Some(false) {
        log::warn!("launcher: show() 没生效，直接纠正窗口显示状态");
        if let Ok(hwnd) = window.hwnd() {
            hexbench_core::sys::show_window(hwnd.0 as isize);
        }
    }
    // 面板窗口是常驻的：页面只在第一次加载时播 CSS 动画。每次呼出都告诉
    // 前端一声，入场动效才每次都播（不播也不影响功能）。
    let _ = window.app_handle().emit("launcher-shown", ());
    // 每次呼出都同步一次主题：设置里改了颜色，常驻窗口要跟上。
    op(3);
    push_theme_to_windows(window.app_handle());
    op(0);
    // 设置里开了内置网页：把预建好的网页窗口摆到右侧并显示
    // （只 show，不在这里创建窗口——运行中建窗口会卡住面板）。
    let mut web_ms = 0u128;
    {
        let app = window.app_handle();
        let enabled = {
            let state = app.state::<AppState>();
            let shared = state.shared.lock().unwrap();
            shared.config.launcher.web_enabled
                && !shared.config.launcher.sites.is_empty()
        };
        if enabled {
            if let Some(web) = web_window_handle() {
                let before = started.elapsed();
                op(4);
                place_web_window(app, &web);
                op(0);
                op(5);
                let _ = web.show();
                op(0);
                WEB_PANEL_OPEN.store(true, Ordering::Relaxed);
                web_ms = started.elapsed().saturating_sub(before).as_millis();
            }
        }
    }
    op(6);
    let _ = window.set_focus();
    op(0);
    // 之前开着网页面板的话，跟着一起回来（页面没重新加载）。
    // 之前开着网页区的话，跟着一起恢复（子 WebView 留在窗口里，不重新加载）。
    let mut sync_ms = 0u128;
    if WEB_PANEL_OPEN.load(Ordering::Relaxed) && web_enabled_at(window.app_handle()) {
        let before = started.elapsed();
        let handle = window.app_handle().clone();
        op(7);
        let _ = sync_web_side(&handle, true);
        op(0);
        sync_ms = started.elapsed().saturating_sub(before).as_millis();
    }
    // Windows 的前台锁会吞掉单纯的 show+focus，必须走 force_foreground，
    // 否则按完快捷键打不了字。
    // show() 只是排进事件循环，此刻窗口可能还没真正绘制出来；延迟一点再抢
    // 一次前台，比在同一帧里连调两次可靠。
    // 抢前台得在窗口自己的线程上做：跨线程 SetFocus 是同步的，主线程正走在键盘
    // 消息里时两边互相等，连续呼出几次就死锁。等待放工作线程，执行排回主线程。
    let handle = window.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(130));
        let task = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            op(17);
            if let Ok(hwnd) = task.hwnd() {
                hexbench_core::sys::force_foreground(hwnd.0 as isize);
            }
            op(0);
        });
    });
    // 网页窗口跟着面板一起回来：页面还在里面，不会重新加载。
    // 内置网页被关掉时不能跟着弹：它是个 always_on_top 的窗口，会压在面板上。
    if web_enabled_at(window.app_handle()) {
        if let Some(site) = window.app_handle().get_webview_window(SITE_WEBVIEW) {
            op(8);
            place_site_window(window, &site);
            let _ = site.show();
            op(0);
        }
    }
    op(9);
    let _ = window.app_handle().emit("launcher-shown", ());
    op(0);
    log::info!(
        "launcher: shown (show={}ms web={web_ms}ms sync={sync_ms}ms total={}ms)",
        after_show.as_millis(),
        started.elapsed().as_millis()
    );
}

fn open_launcher(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("launcher") {
        reveal_launcher(&window, launcher_appearance(app));
        return;
    }
    // 关掉内嵌网页时面板窗口可能被一起带走，但 `launcher` 这个 label 还挂在
    // 旧 WebView 上：不先清掉，重建就会报「label already exists」，
    // 用户看到的就是「按热键没反应」。
    if let Some(stale) = app.get_webview("launcher") {
        log::warn!("launcher: clearing stale webview before rebuild");
        let _ = stale.close();
    }
    if let Some(leftover) = app.get_window("launcher") {
        log::warn!("launcher: destroying leftover window before rebuild");
        // 旧窗口的拖放登记要一起摘掉：句柄可能被新窗口复用，
        // 留着会让新面板的「已登记」判断误以为已经挂过。
        if let Ok(hwnd) = leftover.hwnd() {
            hexbench_core::drop_target::detach(hwnd.0 as isize);
        }
        let _ = leftover.destroy();
    }
    // Creating a WebView2 window re-enters the event loop: doing it on the UI
    // thread froze the whole app for five seconds. Build it on a worker, then
    // reveal it from there.
    let handle = app.clone();
    let appearance = launcher_appearance(app);
    thread::spawn(move || {
        // 销毁是异步的：等它落地再建，否则还会撞 label。
        thread::sleep(Duration::from_millis(120));
        if let Some(window) = build_launcher(&handle) {
            reveal_launcher(&window, appearance);
        }
    });
}


fn show_launcher(app: &AppHandle) {
    // 面板还没出来时，前台窗口才是「用户想输入的地方」；已经显示时别覆盖，
    // 否则选完文件夹重新 reveal 那一下会把目标记成面板自己。
    let already_visible = app
        .get_webview_window("launcher")
        .and_then(|window| panel_visible(&window))
        .unwrap_or(false);
    if !already_visible {
        capture_launcher_return();
    }
    open_launcher(app);
}



fn hide_launcher(app: &AppHandle, reason: &str) {
    let started = Instant::now();
    log::info!(
        "launcher: hide ({reason}, site_open={})",
        app.get_webview_window(SITE_WEBVIEW).is_some()
    );
    if let Some(window) = app.get_webview_window("launcher") {
        // 已经藏起来了就不要再存一次尺寸、再写一遍配置：连续呼出呼入时
        // 这里会被失焦事件连续触发，重复写盘只是白白抖硬盘。
        let was_visible = panel_visible(&window).unwrap_or(false);
        if was_visible {
            op(10);
            store_panel_size(&window);
            op(0);
        }
        let after_store = started.elapsed().as_millis();
        op(11);
        let _ = window.hide();
        op(0);
        let after_hide = started.elapsed().as_millis();
        if panel_visible(&window) == Some(true) {
            log::warn!("launcher: hide() 没生效，直接纠正窗口显示状态");
            if let Ok(hwnd) = window.hwnd() {
                hexbench_core::sys::hide_window(hwnd.0 as isize);
            }
        }
    // 网页窗口是独立窗口：面板收起时一起隐藏
    if let Some(web) = web_window_handle() {
        // 收起前存一次：节流可能把最后一次拖动丢掉，这里以窗口实际状态为准
        op(12);
        store_current_web_geometry(&web);
        op(0);
        let after_web_store = started.elapsed().as_millis();
        op(13);
        let _ = web.hide();
        op(0);
        log::info!(
            "launcher: hide steps store={after_store}ms panel={after_hide}ms webstore={after_web_store}ms webhide={}ms",
            started.elapsed().as_millis()
        );
    }
    }
    // 网页面板跟着面板一起收起：页面继续跑，但不再挡在前台。
    // 用句柄隐藏：这个窗口会从注册表消失，查不到。
    // 网页是面板窗口里的子 WebView，跟着窗口一起隐藏，不用单独处理。
    op(15);
    let keep = app
        .state::<AppState>()
        .shared
        .lock()
        .unwrap()
        .config
        .launcher
        .site_keep_alive_secs;
    op(0);
    if keep == 0 {
        op(14);
        close_site_window(app);
        op(0);
        return;
    }
    let handle = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(keep as u64));
        let visible = handle
            .get_webview_window("launcher")
            .map(|window| panel_visible(&window).unwrap_or(false))
            .unwrap_or(false);
        if visible {
            return;
        }
        log::info!("launcher: page idle for too long, closing the site window");
        op(14);
        close_site_window(&handle);
        op(0);
    });
}



/// 独立网页窗口的标签。
const SITE_WEBVIEW: &str = "site";

/// 网页窗口和面板之间的间距（逻辑像素）。
const SITE_GAP: f64 = 0.0;

/// Windows 给窗口留的不可见外框宽度（实测 8px）。
/// 网页面板要「贴住」面板而不是留缝，就得把这圈隐形边抵掉。
const WEB_FRAME_INSET: f64 = 8.0;

/// 窗口尺寸变化后，把网页子 WebView 重新铺到右栏。
///
/// 不能靠 auto_resize：issue #9611 明确说它会破坏固定横向位置；
/// issue #13318 给的官方临时解法就是自己监听 resize 再 set_size。
fn reflow_web_page(app: &AppHandle) {
    let Some(window) = app.get_webview_window("launcher") else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let width = size.width as f64 / scale;
    let height = size.height as f64 / scale;
    if let Some(page) = app.get_webview(SITE_WEBVIEW) {
        let _ = page.set_position(tauri::LogicalPosition::new(
            (width - WEB_COLUMN_WIDTH).max(0.0),
            WEB_TOOLBAR,
        ));
        let _ = page.set_size(tauri::LogicalSize::new(
            WEB_COLUMN_WIDTH,
            (height - WEB_TOOLBAR).max(80.0),
        ));
    }
}

fn sync_web_side(app: &AppHandle, open: bool) -> Result<(), String> {
    log::info!("web: sync_web_side open={open}");
    // 设置里关掉内置网页之后，任何路径都不该再建出网页窗口/子 WebView。
    if !web_enabled_at(app) {
        WEB_PANEL_OPEN.store(false, Ordering::Relaxed);
        return Ok(());
    }
    if open {
        ensure_web_window(app)?;
    }
    if let Some(web) = web_window_handle() {
        if open {
            place_web_window(app, &web);
            let _ = web.show();
        } else {
            op(16);
            let _ = web.hide();
            op(0);
        }
    }
    WEB_PANEL_OPEN.store(open, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
fn web_panel_show(app: AppHandle) -> Result<bool, String> {
    sync_web_side(&app, true)?;
    Ok(true)
}

#[tauri::command]
fn web_panel_hide(app: AppHandle) -> Result<bool, String> {
    sync_web_side(&app, false)?;
    Ok(false)
}

/// 建独立网页窗口（我们的 web.html：顶栏图标标签 + 下面挂子 WebView）。
fn ensure_web_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    if let Some(existing) = app.get_webview_window(WEB_PANEL) {
        return Ok(existing);
    }
    let (cursor_x, cursor_y) = hexbench_core::sys::cursor_position();
    let (_x, _y, _w, area_h) =
        hexbench_core::sys::work_area_at(cursor_x, cursor_y).unwrap_or((0, 0, 1920, 1080));
    // 工作区高度是物理像素，inner_size 要逻辑值。
    let area_h = area_h as f64 / scale_at_cursor(app);
    let height = (area_h * 0.85).max(320.0);
    let built = tauri::WebviewWindowBuilder::new(
        app,
        WEB_PANEL,
        tauri::WebviewUrl::App("web.html".into()),
    )
    .title("网页")
    .additional_browser_args(WEBVIEW_ARGS)
    .inner_size(WEB_COLUMN_WIDTH, height)
    .decorations(false)
    .resizable(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build()
    .map_err(|error| format!("开不了网页窗口：{error}"))?;
    // 存下句柄：这个窗口实体不会出现在 get_webview_window 的查找结果里
    if let Ok(mut held) = WEB_WINDOW.lock() {
        *held = Some(built.clone());
    }
    // 拖动 / 改大小：重排子网页并记住几何
    {
        let probe = built.clone();
        built.on_window_event(move |event| {
            if matches!(
                event,
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
            ) {
                remember_web_geometry(&probe);
            }
        });
    }
    Ok(built)
}

/// 摆网页窗口：默认固定贴屏幕一侧；勾了「跟随面板」才贴到面板右边。
fn place_web_window(app: &AppHandle, web: &tauri::WebviewWindow) {
    let (width_percent, height_percent, follow, side, saved) = {
        let state = app.state::<AppState>();
        let shared = state.shared.lock().unwrap();
        let launcher = &shared.config.launcher;
        let pct_w = launcher.site_width_percent.clamp(10, 90);
        let pct_h = launcher.site_height_percent.clamp(20, 100);
        // 拖动保存的几何：关掉「记住位置」或改了百分比都不算数
        let saved = (launcher.site_remember_position
            && launcher.site_w > 0
            && launcher.site_pct_w == pct_w
            && launcher.site_pct_h == pct_h)
            .then_some((
                launcher.site_x,
                launcher.site_y,
                launcher.site_w as f64,
                launcher.site_h as f64,
            ));
        (
            pct_w,
            pct_h,
            launcher.site_follow_panel,
            launcher.site_side.clone(),
            saved,
        )
    };
    let (cursor_x, cursor_y) = hexbench_core::sys::cursor_position();
    let (area_x, area_y, area_w, area_h) =
        hexbench_core::sys::work_area_at(cursor_x, cursor_y).unwrap_or((0, 0, 1920, 1080));
    // 工作区是物理像素；下面算出的宽高 / 位置都交给 LogicalSize / LogicalPosition，
    // 所以先按光标所在屏幕的缩放换成逻辑值。不换算的话 150% 下右边贴边放出来的
    // 网页窗口左边缘会落到屏幕外，整个窗口看不见。
    let scale = scale_at_cursor(app);
    let area_x = area_x as f64 / scale;
    let area_y = area_y as f64 / scale;
    let area_w = area_w as f64 / scale;
    let area_h = area_h as f64 / scale;
    let pct_width = (area_w * width_percent as f64 / 100.0).max(240.0);
    let pct_height = (area_h * height_percent as f64 / 100.0).max(240.0);

    // 优先级：跟随面板 > 记住的拖动结果 > 固定贴一侧
    let (width, height, left, top) = if follow {
        let (left, top) = match app.get_webview_window("launcher") {
            Some(panel) => {
                let scale = panel.scale_factor().unwrap_or(1.0);
                match (panel.outer_position(), panel.inner_size()) {
                    (Ok(position), Ok(size)) => (
                        (position.x as f64 / scale + size.width as f64 / scale + 8.0).round(),
                        (position.y as f64 / scale).round(),
                    ),
                    _ => (area_x, area_y),
                }
            }
            None => (area_x, area_y),
        };
        (pct_width, pct_height, left, top)
    } else if let Some((saved_x, saved_y, saved_w, saved_h)) = saved {
        // 用户拖过：位置和大小都按他拖出来的来
        (saved_w, saved_h, saved_x as f64, saved_y as f64)
    } else if side == "left" {
        (
            pct_width,
            pct_height,
            area_x,
            area_y + (area_h - pct_height) / 2.0,
        )
    } else {
        (
            pct_width,
            pct_height,
            area_x + area_w - pct_width,
            area_y + (area_h - pct_height) / 2.0,
        )
    };

    let _ = web.set_size(tauri::LogicalSize::new(width, height));
    let _ = web.set_position(tauri::LogicalPosition::new(left, top));
    reflow_site_page(app, width, height);
    log::info!("web: placed at {left},{top} {width}x{height} follow={follow}");
    store_web_geometry(app, left.round() as i32, top.round() as i32, width.round() as u32, height.round() as u32);
}

/// 子 WebView 按窗口尺寸重新排版。
///
/// 它是按创建时的窗口尺寸摆的，窗口一旦改大小（程序改或用户拖边角），
/// 不重排就会留在旧位置/旧尺寸 —— 表现就是「网页内容空白」。
fn reflow_site_page(app: &AppHandle, width: f64, height: f64) {
    if let Some(page) = app.get_webview(SITE_WEBVIEW) {
        let _ = page.set_position(tauri::LogicalPosition::new(1.0, WEB_TOOLBAR));
        let _ = page.set_size(tauri::LogicalSize::new(
            (width - 2.0).max(80.0),
            (height - WEB_TOOLBAR - 1.0).max(80.0),
        ));
    }
}

/// 把网页窗口的几何写进配置（立即落盘，调用方负责防抖）。
fn store_web_geometry(app: &AppHandle, x: i32, y: i32, w: u32, h: u32) {
    let config = {
        let state = app.state::<AppState>();
        let mut shared = state.shared.lock().unwrap();
        let launcher = &mut shared.config.launcher;
        launcher.site_x = x;
        launcher.site_y = y;
        launcher.site_w = w;
        launcher.site_h = h;
        launcher.site_pct_w = launcher.site_width_percent.clamp(10, 90);
        launcher.site_pct_h = launcher.site_height_percent.clamp(20, 100);
        shared.config.clone()
    };
    let state = app.state::<AppState>();
    if let Err(error) = hexbench_core::config::save(&state.config_path, &config) {
        log::warn!("web: save geometry failed: {error}");
    } else {
        log::info!("web: geometry saved {w}x{h} at {x},{y}");
    }
}

/// 按窗口当前实际位置和大小写一次配置（收起/摆放后调用最可靠）。
fn store_current_web_geometry(window: &tauri::WebviewWindow) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) else {
        return;
    };
    store_web_geometry(
        window.app_handle(),
        (position.x as f64 / scale).round() as i32,
        (position.y as f64 / scale).round() as i32,
        (size.width as f64 / scale).round() as u32,
        (size.height as f64 / scale).round() as u32,
    );
}
/// 用户拖动网页窗口（移动或拖边角改大小）后：重排子网页 + 记住几何。
fn remember_web_geometry(window: &tauri::WebviewWindow) {
    // 隐藏/显示过程中系统会上报一个临时位置（比如 130,130），
    // 那不是用户拖动，存下来会让下次呼出跑到奇怪的地方。
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) else {
        return;
    };
    let x = (position.x as f64 / scale).round() as i32;
    let y = (position.y as f64 / scale).round() as i32;
    let w = (size.width as f64 / scale).round() as u32;
    let h = (size.height as f64 / scale).round() as u32;
    // 拖边角时子网页要跟着变，否则看起来像卡住了
    reflow_site_page(window.app_handle(), w as f64, h as f64);

    // 拖动过程会持续触发，落盘做 800ms 防抖
    static LAST_SAVE: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0);
    if now.saturating_sub(LAST_SAVE.load(Ordering::Relaxed)) < 800 {
        return;
    }
    LAST_SAVE.store(now, Ordering::Relaxed);

    let handle = window.app_handle().clone();
    let config = {
        let state = handle.state::<AppState>();
        let mut shared = state.shared.lock().unwrap();
        let launcher = &mut shared.config.launcher;
        launcher.site_x = x;
        launcher.site_y = y;
        launcher.site_w = w;
        launcher.site_h = h;
        launcher.site_pct_w = launcher.site_width_percent.clamp(10, 90);
        launcher.site_pct_h = launcher.site_height_percent.clamp(20, 100);
        shared.config.clone()
    };
    let state = handle.state::<AppState>();
    if let Err(error) = hexbench_core::config::save(&state.config_path, &config) {
        log::warn!("web: save geometry failed: {error}");
    } else {
        log::info!("web: geometry saved {w}x{h} at {x},{y}");
    }
}



/// 记下用户拖出来的面板尺寸（收起时调用最可靠）。
fn store_panel_size(window: &tauri::WebviewWindow) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let Ok(size) = window.inner_size() else {
        return;
    };
    let width = (size.width as f64 / scale).round() as u32;
    let height = (size.height as f64 / scale).round() as u32;
    if width < 240 || height < 160 {
        return;
    }
    let app = window.app_handle();
    let config = {
        let state = app.state::<AppState>();
        let mut shared = state.shared.lock().unwrap();
        let launcher = &mut shared.config.launcher;
        launcher.panel_w = width;
        launcher.panel_h = height;
        launcher.panel_pct_w = launcher.width_percent.clamp(10, 100);
        launcher.panel_pct_h = launcher.height_percent.clamp(10, 100);
        shared.config.clone()
    };
    let state = app.state::<AppState>();
    if let Err(error) = hexbench_core::config::save(&state.config_path, &config) {
        log::warn!("launcher: save size failed: {error}");
    } else {
        log::info!("launcher: size saved {width}x{height}");
    }
}
/// 网页窗口的句柄。
///
/// 不能用 `get_webview_window("web")` 去找：实测这个窗口的 webview 注册了、
/// 窗口实体却没进注册表（自测输出 `web missing` 就是这么来的）。
static WEB_WINDOW: Mutex<Option<tauri::WebviewWindow>> = Mutex::new(None);

/// 取网页窗口句柄。
fn web_window_handle() -> Option<tauri::WebviewWindow> {
    WEB_WINDOW.lock().ok().and_then(|held| held.clone())
}

/// 设置里的「内置网页」是不是开着（默认开）。
fn web_enabled_at(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .and_then(|state| {
            state
                .shared
                .lock()
                .ok()
                .map(|shared| shared.config.launcher.web_enabled)
        })
        .unwrap_or(false)
}

/// 网页面板当前是不是「打开着」（用户点了 ✕ 才算关）。
/// 面板收起时网页跟着藏，但状态保留——下次呼出面板它一起回来。
static WEB_PANEL_OPEN: AtomicBool = AtomicBool::new(false);

/// 网页当前打开的地址：地址没变就不重新导航（保住滚动位置和未完成的回答）。
static LAST_SITE_URL: Mutex<Option<String>> = Mutex::new(None);

fn close_site_window(app: &AppHandle) {
    // 现在的网页是挂在 `web` 窗口里的子 WebView，get_webview_window 找不到它；
    // 旧的独立窗口还留着兜底。
    if let Some(page) = app.get_webview(SITE_WEBVIEW) {
        let _ = page.close();
    }
    if let Some(window) = app.get_webview_window(SITE_WEBVIEW) {
        let _ = window.close();
    }
    if let Ok(mut last) = LAST_SITE_URL.lock() {
        *last = None;
    }
}

/// 把网页窗口摆到面板右侧、高度对齐面板。
fn place_site_window(panel: &tauri::WebviewWindow, site: &tauri::WebviewWindow) {
    let scale = panel.scale_factor().unwrap_or(1.0);
    let (Ok(position), Ok(size)) = (panel.outer_position(), panel.inner_size()) else {
        return;
    };
    let left = (position.x as f64 / scale + size.width as f64 / scale + SITE_GAP - WEB_FRAME_INSET).round();
    let top = (position.y as f64 / scale).round();
    let height = (size.height as f64 / scale).max(240.0);
    let _ = site.set_position(tauri::LogicalPosition::new(left, top));
    let _ = site.set_size(tauri::LogicalSize::new(WEB_COLUMN_WIDTH, height));
}

/// 建一个独立的网页窗口（不是子 WebView）。
///
/// 独立窗口的好处：面板显示/隐藏/改尺寸都不会连累它，
/// 页面可以真的留在后台（AI 回答不断），也不会出现两边尺寸对不齐的错位。
fn build_site_window(app: &AppHandle, url: tauri::Url) -> Result<(), String> {
    let panel = app.get_webview_window("launcher").ok_or("面板还没建好")?;
    let scale = panel.scale_factor().unwrap_or(1.0);
    let (Ok(position), Ok(size)) = (panel.outer_position(), panel.inner_size()) else {
        return Err("拿不到面板位置".to_string());
    };
    let left = (position.x as f64 / scale + size.width as f64 / scale + SITE_GAP - WEB_FRAME_INSET).round();
    let top = (position.y as f64 / scale).round();
    let height = (size.height as f64 / scale).max(240.0);
    tauri::WebviewWindowBuilder::new(app, SITE_WEBVIEW, tauri::WebviewUrl::External(url))
        .title("网页")
        .additional_browser_args(WEBVIEW_ARGS)
        .inner_size(WEB_COLUMN_WIDTH, height)
        .position(left, top)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .user_agent(SITE_USER_AGENT)
        .on_page_load(|webview, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                let _ = webview.app_handle().emit("launcher-site-loaded", ());
            }
        })
        .build()
        .map_err(|error| format!("开不了网页窗口：{error}"))?;
    Ok(())
}


/// 面板是个小窗口，按 PC 版请求的话很多站点的自适应会挤成一条缝。
/// 统一用安卓 Chrome 的 UA，让站点直接给移动版布局。
const SITE_USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36";

/// 在面板内容区里挂一个真正的 WebView 打开网址。
///
/// 用子 WebView 而不是 iframe：Google / ChatGPT 这类站点会发
/// `X-Frame-Options: DENY`，iframe 直接被拒；子 WebView 是一个独立浏览器，
/// 不受这条限制。
/// 面板当前是不是竖版网页模式。
///
/// 窗口可能被销毁重建（收起久了、或系统回收），前端状态会一起没；
/// 把这个记在这里，重建后仍然知道该用竖版尺寸。
static LAUNCHER_WEB_MODE: AtomicBool = AtomicBool::new(false);

/// 网页模式下内容区的矩形（逻辑像素，相对窗口客户区）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WebRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// 网页列宽：接近手机竖屏的 390–430，移动版布局才会按设计稿呈现。
const WEB_COLUMN_WIDTH: f64 = 420.0;
/// 侧边栏宽度，和 launcher.html 里的 .sidebar 保持一致。
/// 工具栏高度，和 launcher.html 里的 .toolbar 保持一致。


#[tauri::command]
fn launcher_set_web_mode(on: bool) -> Result<WebRect, String> {
    // 网页已经改成独立窗口，面板自己不再改尺寸；命令保留是为了兼容前端旧调用。
    let _ = on;
    Ok(WebRect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    })
}
#[tauri::command]
async fn launcher_open_site(app: AppHandle, url: String) -> Result<(), String> {
    if !web_enabled_at(&app) {
        return Err("内置网页已关闭，先到「设置 → 快捷面板 → 内置网页」打开".to_string());
    }
    let normalized = hexbench_core::net::normalize_url(&url);
    let parsed: tauri::Url = normalized
        .parse()
        .map_err(|_| "网址格式不对，需要以 http:// 或 https:// 开头".to_string())?;
    if let Some(existing) = app.get_webview_window(SITE_WEBVIEW) {
        let _ = existing.navigate(parsed);
        // 不在这里 show()：预建窗口该保持隐藏，否则一开机就自己弹出来。
        return Ok(());
    }
    build_site_window(&app, parsed)?;
    log::info!("launcher: opened site {url}");
    Ok(())
}

/// 独立网页面板的窗口标签。
const WEB_PANEL: &str = "web";
/// 网页面板顶栏高度，必须和 web.html 里 `.bar` 的高度一致。
const WEB_TOOLBAR: f64 = 40.0;





/// 开关右侧网页栏：返回操作后的状态（true = 已打开）。
/// 前端只负责显示，加宽窗口和挂子 WebView 都在这里做完。
#[tauri::command]
fn web_side_toggle(app: AppHandle) -> Result<bool, String> {
    let open = !WEB_PANEL_OPEN.load(Ordering::Relaxed);
    sync_web_side(&app, open)?;
    Ok(open)
}

/// 网页栏当前是否打开（前端每次呼出时问一次，避免前后端状态各走各的）。
#[tauri::command]
fn web_side_state() -> bool {
    WEB_PANEL_OPEN.load(Ordering::Relaxed)
}



#[tauri::command]
async fn web_page_open(app: AppHandle, url: String) -> Result<(), String> {
    if !web_enabled_at(&app) {
        return Err("内置网页已关闭，先到「设置 → 快捷面板 → 内置网页」打开".to_string());
    }
    let normalized = hexbench_core::net::normalize_url(&url);
    let parsed: tauri::Url = normalized
        .parse()
        .map_err(|_| "网址格式不对，需要以 http:// 或 https:// 开头".to_string())?;
    // add_child 定义在 Window 上：用持有句柄换出底层 Window，
    // get_window(WEB_PANEL) 查不到（这个窗口实体不在注册表里）。
    let web_panel = web_window_handle().ok_or_else(|| "网页面板还没建好".to_string())?;
    let window = web_panel.as_ref().window();
    if let Some(existing) = app.get_webview(SITE_WEBVIEW) {
        let same = LAST_SITE_URL
            .lock()
            .map(|last| last.as_deref() == Some(normalized.as_str()))
            .unwrap_or(false);
        if !same {
            let _ = existing.navigate(parsed);
            if let Ok(mut last) = LAST_SITE_URL.lock() {
                *last = Some(normalized);
            }
            log::info!("web: navigated to {url}");
        }
        // 不 show()：预建窗口要保持隐藏，否则一开机就自己弹出来。
        return Ok(());
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    let size = window.inner_size().map_err(|error| error.to_string())?;
    let width = size.width as f64 / scale;
    let height = (size.height as f64 / scale - WEB_TOOLBAR).max(120.0);
    window
        .add_child(
            tauri::WebviewBuilder::new(SITE_WEBVIEW, tauri::WebviewUrl::External(parsed))
                .user_agent(SITE_USER_AGENT)
                .on_page_load(|webview, payload| {
                    if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                        let _ = webview.app_handle().emit("launcher-site-loaded", ());
                    }
                }),
            // 左右各让 1px、底部让 1px：留给 CSS 画的发丝边框。
            tauri::LogicalPosition::new(1.0, WEB_TOOLBAR),
            tauri::LogicalSize::new((width - 2.0).max(80.0), (height - 1.0).max(80.0)),
        )
        .map_err(|error| format!("开不了网页：{error}"))?;
    if let Ok(mut last) = LAST_SITE_URL.lock() {
        *last = Some(normalized);
    }
    log::info!("web: opened {url}");
    Ok(())
}

#[tauri::command]
fn web_page_nav(app: AppHandle, action: String) {
    let Some(webview) = app.get_webview(SITE_WEBVIEW) else {
        return;
    };
    let script = match action.as_str() {
        "back" => "history.back()",
        "forward" => "history.forward()",
        "reload" => "location.reload()",
        _ => return,
    };
    let _ = webview.eval(script);
}

/// 网页面板标签页用的数据。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WebSiteDto {
    name: String,
    url: String,
}

#[tauri::command]
fn launcher_web_sites(app: AppHandle) -> Vec<WebSiteDto> {
    log::info!("web: launcher_web_sites called");
    let state = app.state::<AppState>();
    let shared = state.shared.lock().unwrap();
    shared
        .config
        .launcher
        .sites
        .iter()
        .map(|site| WebSiteDto {
            name: site.name.clone(),
            url: site.url.clone(),
        })
        .collect()
}
/// 抓站点标题，给设置页自动填名字用。
#[tauri::command]
async fn site_title(url: String) -> String {
    tauri::async_runtime::spawn_blocking(move || hexbench_core::net::site_title(&url))
        .await
        .unwrap_or_default()
}

/// 站点图标，给顶部标签页用。
#[tauri::command]
async fn site_icon(url: String) -> String {
    tauri::async_runtime::spawn_blocking(move || hexbench_core::net::favicon_data_uri(&url))
        .await
        .unwrap_or_default()
}

/// 关掉网页窗口。
#[tauri::command]
fn launcher_close_site(app: AppHandle) {
    close_site_window(&app);
}

#[tauri::command]
fn launcher_site_nav(app: AppHandle, action: String) {
    let script = match action.as_str() {
        "back" => "history.back()",
        "forward" => "history.forward()",
        "reload" => "location.reload()",
        _ => return,
    };
    if let Some(page) = app.get_webview(SITE_WEBVIEW) {
        let _ = page.eval(script);
        return;
    }
    if let Some(window) = app.get_webview_window(SITE_WEBVIEW) {
        let _ = window.eval(script);
    }
}

#[tauri::command]
fn launcher_site_bounds(_app: AppHandle, _x: f64, _y: f64, _width: f64, _height: f64) {}

fn toggle_launcher(app: &AppHandle) {
    let enabled = app
        .state::<AppState>()
        .shared
        .lock()
        .unwrap()
        .config
        .launcher
        .enabled;
    if !enabled {
        return;
    }
    // 用真实可见状态判断，不用 tao 那份可能过期的缓存。
    match app.get_webview_window("launcher") {
        Some(window) if panel_visible(&window).unwrap_or(false) => hide_launcher(app, "toggle"),
        _ => show_launcher(app),
    }
}

/// 呼出面板里的一行（文件或文件夹）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherItem {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified: i64,
    icon: Option<String>,
}

fn list_folder(path: &std::path::Path, with_icons: bool) -> Vec<LauncherItem> {
    /// 一次性接口的上限。面板本身走流式接口、不设上限；这个只用于预读和
    /// 其他调用方，留个很大的值防止一次吐出几十万条把 IPC 撑爆。
    const LIST_LIMIT: usize = 20_000;

    let mut items = Vec::new();
    for_each_item(path, with_icons, |item| {
        items.push(item);
        items.len() < LIST_LIMIT
    });
    if items.len() >= LIST_LIMIT {
        log::warn!(
            "launcher: {} 里的条目超过 {LIST_LIMIT}，一次性接口只给前 {LIST_LIMIT} 项",
            path.display()
        );
    }
    sort_items(&mut items);
    items
}

fn sort_items(items: &mut [LauncherItem]) {
    // 文件夹永远在前，其余按名字（和资源管理器的默认排序一致）。
    items.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// 逐条遍历一个目录。`emit` 返回 false 表示不再继续。
fn for_each_item(
    path: &std::path::Path,
    with_icons: bool,
    mut emit: impl FnMut(LauncherItem) -> bool,
) -> usize {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut count = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let item_path = entry.path();
        let metadata = entry.metadata().ok();
        // 和资源管理器的默认视图一致：隐藏文件和系统文件不列出来。
        if metadata.as_ref().is_some_and(|meta| {
            meta.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
        }) {
            continue;
        }
        let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = metadata
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let icon = if with_icons {
            hexbench_core::icons::file_icon_base64(&item_path, is_dir)
        } else {
            None
        };
        count += 1;
        if !emit(LauncherItem {
            name,
            path: item_path.to_string_lossy().to_string(),
            is_dir,
            size,
            modified,
            icon,
        }) {
            break;
        }
    }
    count
}

/// 流式列目录的一页。面板先拿到第一批就能画，剩下的边读边补。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherPage {
    items: Vec<LauncherItem>,
    done: bool,
}

/// 启动器：根目录下的每个子文件夹就是一个分类。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherCategory {
    name: String,
    path: String,
    count: usize,
    /// 面板里是否隐藏这一项。
    hidden: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherRoot {
    root: String,
    categories: Vec<LauncherCategory>,
    /// 「全部」那一项在面板里是否显示。
    all_visible: bool,
    /// 常用文件夹（按用户排好的顺序），面板左侧直接列出来。
    folders: Vec<String>,
    /// 面板里的网址分组。
    sites: Vec<hexbench_core::config::LauncherSite>,
    /// 设置里的「内置网页」开关：开着就默认展开右侧网页栏。
    web_enabled: bool,
    /// 设置里的「收藏」开关：关掉就不显示收藏那一栏。
    notes_enabled: bool,
    /// 已经钉在桌面上的笔记 id（面板据此把图钉按钮显示成「已钉」）。
    notes_pinned: Vec<String>,
    /// 上次用的视图和排序（快速启动 / 分类这一组）。
    view: String,
    sort_key: String,
    sort_dir: i32,
    /// 快速访问（钉住的文件夹）单独记的一套；没单独设过就等于上面那套。
    pinned_view: String,
    pinned_sort_key: String,
    pinned_sort_dir: i32,
    /// 文件区排版缩放的百分比；快速访问那套没单独设过时就是分类那套。
    view_zoom: u32,
    pinned_view_zoom: u32,
    /// 快速访问是否已经单独设过（没设过就跟着分类那套走）。
    pinned_view_set: bool,
    /// 下次呼出要不要回到上次的位置。
    remember_location: bool,
    last_kind: String,
    last_base: String,
    last_label: String,
    last_path: String,
    /// 上次停在「快速访问」那一行（而不是启动分类）。
    last_pinned: bool,
    /// 文件区右键滑动的手势设置。
    gestures: hexbench_core::config::GestureSettings,
}
/// 扫一个目录，把子文件夹当成分类；排序按配置里的顺序，其余按名字。
fn scan_categories(root: &str, order: &[String], hidden: &[String]) -> Vec<LauncherCategory> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut list: Vec<LauncherCategory> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let count = std::fs::read_dir(&path)
            .map(|reader| reader.flatten().count())
            .unwrap_or(0);
        let is_hidden = hidden.iter().any(|entry| entry == &name);
        list.push(LauncherCategory {
            name,
            path: path.to_string_lossy().to_string(),
            count,
            hidden: is_hidden,
        });
    }
    let rank = |name: &str| -> usize {
        order
            .iter()
            .position(|entry| entry == name)
            .unwrap_or(usize::MAX)
    };
    list.sort_by(|a, b| {
        rank(&a.name)
            .cmp(&rank(&b.name))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    list
}

/// 设置页用：直接扫这个路径，不读「当前根目录」。
///
/// 之前是让设置页复用 `launcher_categories`，可它读的是已保存的配置：
/// 刚在对话框里选完文件夹时配置还没落盘，扫到的是旧的空路径，于是显示
/// 「这个目录下还没有子文件夹」。传路径进来就没有这个竞态了。
#[tauri::command]
async fn launcher_categories_at(path: String, app: AppHandle) -> Vec<LauncherCategory> {
    let (order, hidden) = {
        let state = app.state::<AppState>();
        let shared = state.shared.lock().unwrap();
        (
            shared.config.launcher.category_order.clone(),
            shared.config.launcher.hidden_categories.clone(),
        )
    };
    tauri::async_runtime::spawn_blocking(move || {
        if path.trim().is_empty() {
            return Vec::new();
        }
        scan_categories(&path, &order, &hidden)
    })
    .await
    .unwrap_or_default()
}
#[tauri::command]
async fn launcher_categories(app: AppHandle) -> LauncherRoot {
    // 同样要离开主线程：这里会逐个子文件夹 read_dir 数条目。
    tauri::async_runtime::spawn_blocking(move || launcher_root(&app))
        .await
        .unwrap_or_else(|_| LauncherRoot {
            root: String::new(),
            categories: Vec::new(),
            all_visible: false,
            folders: Vec::new(),
            sites: Vec::new(),
            web_enabled: false,
            notes_enabled: false,
            notes_pinned: Vec::new(),
            view: "grid".to_string(),
            sort_key: "name".to_string(),
            sort_dir: 1,
            pinned_view: "grid".to_string(),
            pinned_sort_key: "name".to_string(),
            pinned_sort_dir: 1,
            view_zoom: 100,
            pinned_view_zoom: 100,
            pinned_view_set: false,
            remember_location: false,
            last_kind: String::new(),
            last_base: String::new(),
            last_label: String::new(),
            last_path: String::new(),
            last_pinned: false,
            gestures: hexbench_core::config::GestureSettings::default(),
        })
}

/// 100 是默认也是最小，最多放到 150；越界（旧配置没有这个字段时是 0、
/// 以前存过更小的值）当作 100，界面拿到的永远是能直接用的值。
fn clamp_zoom(value: u32) -> u32 {
    if (100..=150).contains(&value) {
        value
    } else {
        100
    }
}

fn launcher_root(app: &AppHandle) -> LauncherRoot {
    let state = app.state::<AppState>();
    let shared = state.shared.lock().unwrap();
    let root = shared.config.launcher.root.clone();
    let location = shared.config.launcher.last_location.clone();
    let remember_location = shared.config.launcher.remember_location;
    let folders = shared.config.launcher.folders.clone();
    let sites = shared.config.launcher.sites.clone();
    let category_order = shared.config.launcher.category_order.clone();
    let hidden = shared.config.launcher.hidden_categories.clone();
    let view = shared.config.launcher.view.clone();
    let sort_key = shared.config.launcher.sort_key.clone();
    let sort_dir = shared.config.launcher.sort_dir;
    // 快速访问那一组还没单独设过时，先用分类这套，界面拿到的永远是能直接用的值。
    let pinned_view = match shared.config.launcher.pinned_view.as_str() {
        "" => view.clone(),
        other => other.to_string(),
    };
    let pinned_sort_key = match shared.config.launcher.pinned_sort_key.as_str() {
        "" => sort_key.clone(),
        other => other.to_string(),
    };
    let pinned_sort_dir = match shared.config.launcher.pinned_sort_dir {
        0 => sort_dir,
        other => other,
    };
    let view_zoom = clamp_zoom(shared.config.launcher.view_zoom);
    let pinned_view_zoom = match shared.config.launcher.pinned_view_zoom {
        0 => view_zoom,
        other => clamp_zoom(other),
    };
    let pinned_view_set = !shared.config.launcher.pinned_view.is_empty();
    let web_enabled = shared.config.launcher.web_enabled;
    let notes_enabled = shared.config.notes.enabled;
    let notes_pinned: Vec<String> = shared
        .config
        .notes
        .pinned
        .iter()
        .map(|pin| pin.id.clone())
        .collect();
    let gestures = shared.config.launcher.gestures.clone();
    drop(shared);
    let all_visible = !hidden.iter().any(|entry| entry == "__all__");
    let empty = |root: String| LauncherRoot {
        root,
        categories: Vec::new(),
        all_visible,
        folders: folders.clone(),
        sites: sites.clone(),
        web_enabled,
        notes_enabled,
        notes_pinned: notes_pinned.clone(),
        view: view.clone(),
        sort_key: sort_key.clone(),
        sort_dir,
        pinned_view: pinned_view.clone(),
        pinned_sort_key: pinned_sort_key.clone(),
        pinned_sort_dir,
        view_zoom,
        pinned_view_zoom,
        pinned_view_set,
        remember_location,
        last_kind: location.kind.clone(),
        last_base: location.base.clone(),
        last_label: location.label.clone(),
        last_path: location.path.clone(),
        last_pinned: location.pinned,
        gestures: gestures.clone(),
    };
    if root.trim().is_empty() {
        return empty(root);
    }
    if !std::path::Path::new(&root).is_dir() {
        return empty(root);
    }
    let list = scan_categories(&root, &category_order, &hidden);
    LauncherRoot {
        root,
        categories: list,
        all_visible,
        folders,
        sites,
        web_enabled,
        notes_enabled,
        notes_pinned,
        view,
        sort_key,
        sort_dir,
        pinned_view,
        pinned_sort_key,
        pinned_sort_dir,
        view_zoom,
        pinned_view_zoom,
        pinned_view_set,
        remember_location,
        last_kind: location.kind,
        last_base: location.base,
        last_label: location.label,
        last_path: location.path,
        last_pinned: location.pinned,
        gestures,
    }
}

/// 记住面板这次停在哪儿（只有开启「记住上次位置」时才写盘）。
#[tauri::command]
fn launcher_save_location(
    kind: String,
    base: String,
    label: String,
    path: String,
    pinned: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let pinned = pinned.unwrap_or(false);
    let mut shared = state.shared.lock().unwrap();
    let launcher = &mut shared.config.launcher;
    if !launcher.remember_location {
        return Ok(());
    }
    if launcher.last_location.kind == kind
        && launcher.last_location.base == base
        && launcher.last_location.path == path
        && launcher.last_location.pinned == pinned
    {
        return Ok(());
    }
    launcher.last_location = hexbench_core::config::LauncherLocation {
        kind,
        base,
        label,
        path,
        pinned,
    };
    let config = shared.config.clone();
    drop(shared);
    hexbench_core::config::save(&state.config_path, &config).map_err(|error| error.to_string())
}

/// 从面板跳到设置页：把主窗口叫出来，再告诉前端要打开哪一页。
#[tauri::command]
fn open_settings(page: String, tab: Option<String>, app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        MAIN_SHOW_GEN.fetch_add(1, Ordering::SeqCst);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = app.emit("navigate", serde_json::json!({ "page": page, "tab": tab }));
        return;
    }
    // 主窗口被销毁了（关掉一阵子会自动销毁），或者开机自启时根本没建过：
    // 这时 emit 没有页面接。记下目标页，等窗口建好、页面挂载时自己来取
    // （见 `take_pending_page`）。建窗口要走工作线程，不能在命令里直接建。
    if let Ok(mut pending) = app.state::<AppState>().pending_page.lock() {
        *pending = Some((page, tab));
    }
    let handle = app.clone();
    thread::spawn(move || open_main_now(&handle));
}

/// 主窗口页面挂载时来取「本来要去的那一页」；没有就返回 null。
#[tauri::command]
fn take_pending_page(app: AppHandle) -> Option<(String, Option<String>)> {
    app.state::<AppState>()
        .pending_page
        .lock()
        .ok()
        .and_then(|mut pending| pending.take())
}
/// 记住面板的视图和排序方式。`pinned` 表示这份是「快速访问」那一组的，
/// 和「快速启动」（分类）各记各的。
#[tauri::command]
fn launcher_save_view(
    view: String,
    sort_key: String,
    sort_dir: i32,
    zoom: u32,
    pinned: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut shared = state.shared.lock().unwrap();
    let launcher = &mut shared.config.launcher;
    let (slot_view, slot_key, slot_dir, slot_zoom) = if pinned {
        (
            &mut launcher.pinned_view,
            &mut launcher.pinned_sort_key,
            &mut launcher.pinned_sort_dir,
            &mut launcher.pinned_view_zoom,
        )
    } else {
        (
            &mut launcher.view,
            &mut launcher.sort_key,
            &mut launcher.sort_dir,
            &mut launcher.view_zoom,
        )
    };
    let zoom = clamp_zoom(zoom);
    if *slot_view == view && *slot_key == sort_key && *slot_dir == sort_dir && *slot_zoom == zoom {
        return Ok(());
    }
    *slot_view = view;
    *slot_key = sort_key;
    *slot_dir = sort_dir;
    *slot_zoom = zoom;
    let config = shared.config.clone();
    drop(shared);
    hexbench_core::config::save(&state.config_path, &config).map_err(|error| error.to_string())
}
/// 把文件移进某个文件夹（面板内部拖拽用）。
#[tauri::command(async)]
fn launcher_move_into(dest: String, paths: Vec<String>, app: AppHandle) -> Result<usize, String> {
    let dir = PathBuf::from(&dest);
    if !dir.is_dir() {
        return Err("目标文件夹不存在".to_string());
    }
    let sources: Vec<PathBuf> = paths
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect();
    if sources.is_empty() {
        log::warn!("launcher: nothing to move, sources are gone: {paths:?}");
        return Err("文件已经不在了".to_string());
    }
    // 移动前先看目标目录里有没有同名文件：重名时系统会自己改名
    // （「xxx - 副本.txt」），我们不知道它最终叫什么。拿不准的那一项就不记
    // 撤销——总比 Ctrl+Z 把目录里本来就有、同名的那个文件搬回源位置强。
    let expected: Vec<(PathBuf, PathBuf)> = sources
        .iter()
        .filter_map(|source| {
            let name = source.file_name()?;
            let target = dir.join(name);
            (!target.exists()).then(|| (target, source.clone()))
        })
        .collect();
    let moved = move_paths_into(&dir, &sources);
    // 记下「新位置 ← 旧位置」，撤销时搬回去。只记真正移动成功、
    // 而且目标名没有被系统改过的。
    let pairs: Vec<(PathBuf, PathBuf)> = expected
        .into_iter()
        .filter(|(target, source)| target.exists() && !source.exists())
        .collect();
    if !pairs.is_empty() {
        push_undo(UndoOp::Move(pairs));
    }
    log::info!("launcher: moved {moved} item(s) into {}", dir.display());
    let _ = app.emit("launcher-changed", ());
    Ok(moved)
}
/// 把文件复制进某个文件夹（按住 Ctrl 拖进来时用）。
#[tauri::command(async)]
fn launcher_copy_into(dest: String, paths: Vec<String>, app: AppHandle) -> Result<usize, String> {
    let dir = PathBuf::from(&dest);
    if !dir.is_dir() {
        return Err("目标文件夹不存在".to_string());
    }
    let sources: Vec<PathBuf> = paths
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect();
    if sources.is_empty() {
        log::warn!("launcher: nothing to copy, sources are gone: {paths:?}");
        return Err("文件已经不在了".to_string());
    }
    let copied = hexbench_core::shell_ops::copy_into(&sources, &dir)?;
    log::info!("launcher: copied {copied} item(s) into {}", dir.display());
    let _ = app.emit("launcher-changed", ());
    Ok(copied)
}
/// 删进回收站 / 撤销之后把键盘焦点抢回面板：系统处理回收站时会短暂抢走前台，
/// 焦点没回来的话紧接着按 Ctrl+Z 就没反应。
#[tauri::command]
fn launcher_refocus(app: AppHandle) {
    let Some(window) = app.get_webview_window("launcher") else {
        return;
    };
    if !panel_visible(&window).unwrap_or(false) {
        return;
    }
    // 回收站那一下是异步收尾的：立刻抢前台会被它随后再抢走，等一小会儿。
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(180));
        let handle = window.clone();
        let _ = window.run_on_main_thread(move || {
            if let Ok(hwnd) = handle.hwnd() {
                let raw = hwnd.0 as isize;
                hexbench_core::sys::force_foreground(raw);
                // 光抢前台不够：系统操作会把键盘焦点留在顶层窗口上，页面
                // 一个按键都收不到。还要把焦点交回 WebView2 子窗口。
                hexbench_core::sys::focus_webview(raw);
            }
        });
    });
}
/// 面板告诉后端「我现在在哪个目录」，拖进来的文件就放进这里。
#[tauri::command]
fn launcher_set_drop_target(path: Option<String>) {
    log::info!("launcher: drop target = {path:?}");
    if let Ok(mut target) = LAUNCHER_DROP_TARGET.lock() {
        *target = path.map(PathBuf::from);
    }
}

// ---------- 收藏（笔记） ----------

/// 配置里留空就是默认目录；每次调用顺手把目录建好。
fn notes_root(state: &AppState) -> Result<PathBuf, String> {
    let folder = state.shared.lock().unwrap().config.notes.folder.clone();
    hexbench_core::notes::resolve_root(&folder)
}

/// 在主线程上跑一段代码并把结果带回来。`force_foreground` 必须在窗口自己的
/// 线程上调用（跨线程 `SetFocus` 会互相等），所以发送时用它排队。
fn on_main_thread<T: Send + 'static>(
    app: &AppHandle,
    task: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = sender.send(task());
    })
    .map_err(|error| error.to_string())?;
    receiver
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| "主线程没有响应".to_string())
}

/// 把收起来的呼出面板恢复出来（发送失败时用，避免用户以为面板丢了）。
fn reshow_launcher(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("launcher") {
        reveal_launcher(&window, launcher_appearance(app));
    }
}

/// 收藏变化广播。
///
/// `id` 是变化后的笔记（删除时是被删掉的那条），窗口据此决定要不要重读；
/// `source` 是写进磁盘的便签窗口标签 —— 自己保存不该再收一遍自己的回声，
/// 否则正在输入的正文会被重读覆盖、撤销记录也会被清掉。
#[derive(Clone, Serialize)]
struct NotesChanged {
    id: Option<String>,
    source: Option<String>,
}

#[tauri::command(async)]
fn notes_list(state: State<'_, AppState>) -> Result<Vec<hexbench_core::notes::NoteSummary>, String> {
    let root = notes_root(&state)?;
    hexbench_core::notes::list(&root)
}

#[tauri::command(async)]
fn notes_read(
    id: String,
    state: State<'_, AppState>,
) -> Result<hexbench_core::notes::NoteFile, String> {
    let root = notes_root(&state)?;
    hexbench_core::notes::read(&root, &id)
}

#[tauri::command(async)]
fn notes_create(
    title: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<hexbench_core::notes::NoteSummary, String> {
    let root = notes_root(&state)?;
    let note = hexbench_core::notes::create(&root, title.as_deref())?;
    let _ = app.emit(
        "notes-changed",
        NotesChanged {
            id: Some(note.id.clone()),
            source: None,
        },
    );
    Ok(note)
}

#[tauri::command(async)]
fn notes_save(
    id: String,
    title: String,
    body: String,
    source: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<hexbench_core::notes::NoteSummary, String> {
    let root = notes_root(&state)?;
    let note = hexbench_core::notes::save(&root, &id, &title, &body)?;
    // 标题变了文件会改名：钉住的便签要跟着改 id，否则下一次自动保存
    // 会写到旧文件名上（文件已经不在，保存失败）。
    if note.id != id {
        let _ = rename_pin_id(&app, &id, &note.id);
    }
    let _ = app.emit(
        "notes-changed",
        NotesChanged {
            id: Some(note.id.clone()),
            source,
        },
    );
    Ok(note)
}

#[tauri::command(async)]
fn notes_delete(id: String, state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let root = notes_root(&state)?;
    hexbench_core::notes::delete(&root, &id)?;
    // 钉着的时候删掉：窗口跟着关，别在桌面上留一张读不到文件的便签。
    let _ = unpin_id(&app, &id);
    let _ = app.emit(
        "notes-changed",
        NotesChanged {
            id: Some(id),
            source: None,
        },
    );
    Ok(())
}

#[tauri::command(async)]
fn notes_import_images(
    note_id: Option<String>,
    paths: Vec<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<hexbench_core::notes::ImportOutcome, String> {
    let root = notes_root(&state)?;
    let sources: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let outcome =
        hexbench_core::notes::import_images(&root, note_id.as_deref(), &sources)?;
    log::info!(
        "notes: imported {} image(s), skipped {} into {}",
        outcome.imported,
        outcome.skipped,
        outcome.note.id
    );
    let _ = app.emit(
        "notes-changed",
        NotesChanged {
            id: Some(outcome.note.id.clone()),
            source: None,
        },
    );
    Ok(outcome)
}

/// 把一条笔记引用的图片全部放进剪贴板（文件列表，资源管理器 / 聊天工具都能贴）。
#[tauri::command(async)]
fn notes_copy_images(id: String, state: State<'_, AppState>) -> Result<usize, String> {
    let root = notes_root(&state)?;
    let paths = hexbench_core::notes::image_paths(&root, &id)?;
    if paths.is_empty() {
        return Err("这条便签里没有图片".to_string());
    }
    hexbench_core::shell_ops::clipboard_set_files(&paths, false)?;
    Ok(paths.len())
}

/// 面板刷新图钉状态用：只回 id，不重新扫分类（自动保存会频繁触发这个）。
#[tauri::command]
fn notes_pinned(app: AppHandle) -> Vec<String> {
    note_pins(&app).into_iter().map(|pin| pin.id).collect()
}

/// 便签里的链接交给系统浏览器：点在预览里不该把便签窗口本身导航走。
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    hexbench_core::sys::open_url(&url)
}

#[tauri::command(async)]
fn notes_folder(state: State<'_, AppState>) -> Result<String, String> {
    let root = notes_root(&state)?;
    Ok(root.to_string_lossy().to_string())
}

/// 默认收藏目录，和配置里写没写自定义目录无关。设置页拿它显示「留空时用哪个」，
/// 不用在读配置的路径上跟防抖保存抢时间。
#[tauri::command(async)]
fn notes_default_folder() -> Result<String, String> {
    hexbench_core::notes::default_root().map(|root| root.to_string_lossy().to_string())
}

#[tauri::command(async)]
fn notes_open_folder(state: State<'_, AppState>) -> Result<(), String> {
    let root = notes_root(&state)?;
    hexbench_core::sys::open_path(&root)
}

#[tauri::command(async)]
fn notes_clear_all(state: State<'_, AppState>, app: AppHandle) -> Result<usize, String> {
    let root = notes_root(&state)?;
    let count = hexbench_core::notes::clear_all(&root)?;
    let _ = unpin_all(&app);
    let _ = app.emit(
        "notes-changed",
        NotesChanged {
            id: None,
            source: None,
        },
    );
    Ok(count)
}

// ---------- 便签：钉在桌面上的笔记窗口 ----------

const NOTE_LABEL_PREFIX: &str = "note-";
const NOTE_DEFAULT_W: u32 = 340;
const NOTE_DEFAULT_H: u32 = 380;
const NOTE_MIN_W: f64 = 220.0;
const NOTE_MIN_H: f64 = 160.0;
const NOTE_MAX_W: u32 = 2400;
const NOTE_MAX_H: u32 = 1800;
/// 收起后只剩顶栏：note.html 里 `.note` 的上下边框 + 36px 顶栏。
const NOTE_HEADER_H: f64 = 38.0;

/// 收起高度按当前缩放算成物理像素（配置里存的是物理像素）。
fn note_header_px(window: &tauri::WebviewWindow) -> u32 {
    let scale = window.scale_factor().unwrap_or(1.0);
    (NOTE_HEADER_H * scale).round() as u32
}

/// 自定义协议的路径是百分号编码的：图片文件名里有中文 / 空格时先还原。
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn note_pins(app: &AppHandle) -> Vec<NotePin> {
    app.state::<AppState>()
        .shared
        .lock()
        .unwrap()
        .config
        .notes
        .pinned
        .clone()
}

/// 把钉住的列表写回配置。内存先改，磁盘失败也只影响下次启动的恢复。
fn write_note_pins(app: &AppHandle, pins: Vec<NotePin>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let config = {
        let mut shared = state.shared.lock().unwrap();
        shared.config.notes.pinned = pins;
        shared.config.clone()
    };
    config::save(&state.config_path, &config).map_err(|error| error.to_string())
}

/// 笔记改名（文件换名）时，把钉住它的便签指向新文件名。
fn rename_pin_id(app: &AppHandle, old_id: &str, new_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let config = {
        let mut shared = state.shared.lock().unwrap();
        let mut changed = false;
        for pin in &mut shared.config.notes.pinned {
            if pin.id == old_id {
                pin.id = new_id.to_string();
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        shared.config.clone()
    };
    config::save(&state.config_path, &config).map_err(|error| error.to_string())
}

/// 取消某个标签的钉住：只改配置，窗口由调用方关。
fn unpin_label(app: &AppHandle, label: &str) -> Option<String> {
    let removed = {
        let state = app.state::<AppState>();
        let mut shared = state.shared.lock().unwrap();
        let index = shared
            .config
            .notes
            .pinned
            .iter()
            .position(|pin| pin.label == label)?;
        let pin = shared.config.notes.pinned.remove(index);
        let config = shared.config.clone();
        drop(shared);
        if let Err(error) = config::save(&state.config_path, &config) {
            log::warn!("note: save pins after unpin failed: {error}");
        }
        pin
    };
    Some(removed.id)
}

/// 取消某条笔记的所有钉住，并把窗口关掉（删除 / 面板上点取消时用）。
fn unpin_id(app: &AppHandle, id: &str) -> Result<(), String> {
    let labels: Vec<String> = note_pins(app)
        .into_iter()
        .filter(|pin| pin.id == id)
        .map(|pin| pin.label)
        .collect();
    if labels.is_empty() {
        return Ok(());
    }
    let pins: Vec<NotePin> = note_pins(app)
        .into_iter()
        .filter(|pin| pin.id != id)
        .collect();
    write_note_pins(app, pins)?;
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.close();
        }
    }
    Ok(())
}

/// 全部取消（清空收藏时用）。
fn unpin_all(app: &AppHandle) -> Result<(), String> {
    let pins = note_pins(app);
    if pins.is_empty() {
        return Ok(());
    }
    write_note_pins(app, Vec::new())?;
    for pin in pins {
        if let Some(window) = app.get_webview_window(&pin.label) {
            let _ = window.close();
        }
    }
    Ok(())
}

/// 找一个没被占用的窗口标签：配置里的、以及还没关干净的窗口都算占用。
fn next_note_label(app: &AppHandle) -> String {
    let pins = note_pins(app);
    for index in 1..=u32::MAX {
        let label = format!("{NOTE_LABEL_PREFIX}{index}");
        if !pins.iter().any(|pin| pin.label == label)
            && app.get_webview_window(&label).is_none()
        {
            return label;
        }
    }
    format!("{NOTE_LABEL_PREFIX}{}", std::process::id())
}

/// 钉住的矩形：尺寸夹到合理范围，位置拉回可见的工作区（拔掉副屏、
/// 分辨率改了以后，别让便签落在屏幕外面找不回来）。
fn note_rect(pin: &NotePin) -> (i32, i32, u32, u32) {
    let w = pin.w.clamp(NOTE_MIN_W as u32, NOTE_MAX_W);
    let h = pin.h.clamp(NOTE_MIN_H as u32, NOTE_MAX_H);
    let (x, y) = hexbench_core::sys::clamp_to_work_area(pin.x, pin.y, w as i32, h as i32);
    (x, y, w, h)
}

fn rects_overlap(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> bool {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    ax < bx + bw as i32
        && bx < ax + aw as i32
        && ay < by + bh as i32
        && by < ay + ah as i32
}

/// 新便签放哪儿：正常放在光标右下；如果会压住正开着的呼出面板，就挪到
/// 面板旁边——用户刚点完钉子，还要继续用面板，不能被自己的便签挡住。
fn pin_initial_rect(app: &AppHandle, w: u32, h: u32) -> (i32, i32, u32, u32) {
    let (cursor_x, cursor_y) = hexbench_core::sys::cursor_pos().unwrap_or((240, 160));
    let mut x = cursor_x + 40;
    let mut y = cursor_y + 40;
    let panel = app.get_webview_window("launcher").and_then(|window| {
        if !window.is_visible().unwrap_or(false) {
            return None;
        }
        let position = window.outer_position().ok()?;
        let size = window.outer_size().ok()?;
        Some((position.x, position.y, size.width, size.height))
    });
    if let Some((px, py, pw, ph)) = panel {
        if rects_overlap((x, y, w, h), (px, py, pw, ph)) {
            let fits = |fx: i32, fy: i32| {
                hexbench_core::sys::work_area_at(fx, fy)
                    .map(|(wx, wy, ww, wh)| {
                        fx >= wx
                            && fy >= wy
                            && fx + w as i32 <= wx + ww
                            && fy + h as i32 <= wy + wh
                    })
                    .unwrap_or(false)
            };
            let right = px + pw as i32 + 16;
            let left = px - w as i32 - 16;
            let below = py + ph as i32 + 16;
            if fits(right, py) {
                x = right;
                y = py;
            } else if fits(left, py) {
                x = left;
                y = py;
            } else if fits(px, below) {
                x = px;
                y = below;
            } else {
                x = right;
                y = below;
            }
        }
    }
    let (x, y) = hexbench_core::sys::clamp_to_work_area(x, y, w as i32, h as i32);
    (x, y, w, h)
}

/// 建一个便签窗口（先藏着，页面画好了自己叫 `note_ready` 显示）。
fn build_note_window(app: &AppHandle, pin: &NotePin) -> Result<tauri::WebviewWindow, String> {
    let (x, y, w, h) = note_rect(pin);
    let window = WebviewWindowBuilder::new(
        app,
        &pin.label,
        WebviewUrl::App("note.html".into()),
    )
    .title("HexBench 便签")
    .additional_browser_args(WEBVIEW_ARGS)
    .inner_size(w as f64, h as f64)
    .min_inner_size(NOTE_MIN_W, NOTE_MIN_H)
    .decorations(false)
    .resizable(!pin.rolled)
    .zoom_hotkeys_enabled(false)
    .transparent(false)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .build()
    .map_err(|error| format!("打开便签失败：{error}"))?;
    // 配置里存的是物理像素，builder 的 inner_size 收的是逻辑尺寸：缩放不是
    // 100% 的机器上会差一截，这里按物理像素再摆一次（窗口还藏着，看不到跳）。
    let height = if pin.rolled {
        note_header_px(&window)
    } else {
        h
    };
    if pin.rolled {
        let _ = window
            .set_min_size(Some(PhysicalSize::new(NOTE_MIN_W as u32, height)));
    }
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.set_size(PhysicalSize::new(w, height));
    if let Ok(hwnd) = window.hwnd() {
        hexbench_core::sys::round_window_corners(hwnd.0 as isize);
    }
    // 页面万一没跑起来（打包漏文件之类），两秒后也要让窗口露出来，
    // 否则用户只看到「点了钉子，什么都没发生」。
    let probe = window.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        if let Ok(hwnd) = probe.hwnd() {
            if !hexbench_core::sys::window_visible(hwnd.0 as isize) {
                hexbench_core::sys::show_window_no_activate(hwnd.0 as isize);
            }
        }
    });
    Ok(window)
}

/// 启动时把上次钉住的便签重新摆出来。桌面上的窗口全在后台线程建，
/// 不拖慢启动；文件已经没了的钉子直接清掉。
fn restore_note_pins(app: AppHandle) {
    let pins = note_pins(&app);
    if pins.is_empty() {
        return;
    }
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(500));
        let root = match notes_root(&app.state::<AppState>()) {
            Ok(root) => root,
            Err(error) => {
                log::warn!("note: restore skipped: {error}");
                return;
            }
        };
        let mut kept = Vec::new();
        let mut changed = false;
        for pin in pins {
            if hexbench_core::notes::resolve_note(&root, &pin.id).is_err() {
                log::info!("note: {} is gone, dropping pin {}", pin.id, pin.label);
                changed = true;
                continue;
            }
            match build_note_window(&app, &pin) {
                Ok(_) => kept.push(pin),
                Err(error) => {
                    log::warn!("note: restore {} failed: {error}", pin.label);
                    changed = true;
                }
            }
        }
        if changed {
            let _ = write_note_pins(&app, kept);
        }
    });
}

/// 便签页面加载完读一次：标签 → 当前笔记。改名之后页面靠它重新对上 id。
#[tauri::command(async)]
fn note_state(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<hexbench_core::notes::NoteFile, String> {
    let label = window.label().to_string();
    let pin = note_pins(&app)
        .into_iter()
        .find(|pin| pin.label == label)
        .ok_or_else(|| "这条便签已经取消了".to_string())?;
    let root = notes_root(&app.state::<AppState>())?;
    hexbench_core::notes::read(&root, &pin.id)
}

/// 页面画好了：显示窗口，但不抢焦点（钉的时候用户多半还在面板上打字）。
#[tauri::command]
fn note_ready(window: tauri::WebviewWindow) {
    if !window.label().starts_with(NOTE_LABEL_PREFIX) {
        return;
    }
    if let Ok(hwnd) = window.hwnd() {
        hexbench_core::sys::show_window_no_activate(hwnd.0 as isize);
    }
}

/// 钉住一条笔记。已经钉着就把它拿到前面来，不再建第二个窗口。
#[tauri::command]
async fn notes_pin(id: String, app: AppHandle) -> Result<NotePin, String> {
    let root = notes_root(&app.state::<AppState>())?;
    // 文件得真的在：钉一个读不到的便签没有意义。
    hexbench_core::notes::read(&root, &id)?;
    if let Some(existing) = note_pins(&app).into_iter().find(|pin| pin.id == id) {
        if let Some(window) = app.get_webview_window(&existing.label) {
            let _ = window.show();
        }
        return Ok(existing);
    }
    let label = next_note_label(&app);
    let (x, y, w, h) = pin_initial_rect(&app, NOTE_DEFAULT_W, NOTE_DEFAULT_H);
    let pin = NotePin {
        id,
        label,
        x,
        y,
        w,
        h,
        rolled: false,
    };
    let mut pins = note_pins(&app);
    pins.push(pin.clone());
    write_note_pins(&app, pins)?;
    let handle = app.clone();
    let build_pin = pin.clone();
    let built = tauri::async_runtime::spawn_blocking(move || {
        build_note_window(&handle, &build_pin)
    })
    .await
    .map_err(|error| error.to_string())?;
    if let Err(error) = built {
        // 建不出来就把刚写进配置的钉子撤掉，别留下一个永远不出现的窗口。
        let pins: Vec<NotePin> = note_pins(&app)
            .into_iter()
            .filter(|entry| entry.label != pin.label)
            .collect();
        let _ = write_note_pins(&app, pins);
        return Err(error);
    }
    log::info!("note: pinned {} as {}", pin.id, pin.label);
    Ok(pin)
}

/// 面板上取消钉住。
#[tauri::command(async)]
fn notes_unpin(id: String, app: AppHandle) -> Result<(), String> {
    unpin_id(&app, &id)?;
    let _ = app.emit(
        "notes-changed",
        NotesChanged {
            id: Some(id),
            source: None,
        },
    );
    Ok(())
}

/// 便签窗口上的「取消钉住」。
#[tauri::command]
fn note_unpin(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    let label = window.label().to_string();
    let id = unpin_label(&app, &label);
    let _ = app.emit(
        "notes-changed",
        NotesChanged {
            id,
            source: None,
        },
    );
    let _ = window.close();
    Ok(())
}

/// 移动 / 改大小后记下几何（页面那边做了防抖）。
#[tauri::command(async)]
fn note_geometry(
    window: tauri::WebviewWindow,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    app: AppHandle,
) -> Result<(), String> {
    let label = window.label().to_string();
    let mut pins = note_pins(&app);
    let Some(pin) = pins.iter_mut().find(|pin| pin.label == label) else {
        return Ok(());
    };
    // 收起时窗口只有标题栏高：这时收到的尺寸是「收起后」的，不能拿来覆盖
    // 展开后的高度，只记位置；展开以后 h 会重新报一次。
    let next_w = if pin.rolled { pin.w } else { w };
    let next_h = if pin.rolled { pin.h } else { h };
    if pin.x == x && pin.y == y && pin.w == next_w && pin.h == next_h {
        return Ok(());
    }
    pin.x = x;
    pin.y = y;
    pin.w = next_w;
    pin.h = next_h;
    write_note_pins(&app, pins)
}

/// 便签窗收起到只剩标题栏 / 再展开。
#[tauri::command]
fn note_roll(window: tauri::WebviewWindow, rolled: bool, app: AppHandle) -> Result<(), String> {
    let label = window.label().to_string();
    let mut pins = note_pins(&app);
    let Some(pin) = pins.iter_mut().find(|pin| pin.label == label) else {
        return Ok(());
    };
    if pin.rolled == rolled {
        return Ok(());
    }
    pin.rolled = rolled;
    let (w, h) = (pin.w, pin.h);
    write_note_pins(&app, pins)?;

    // 收起的高度比最小高度还小，先放宽下限再改尺寸；展开时按相反顺序。
    let header = note_header_px(&window);
    let result = if rolled {
        let _ = window.set_min_size(Some(PhysicalSize::new(NOTE_MIN_W as u32, header)));
        let _ = window.set_resizable(false);
        window
            .set_size(PhysicalSize::new(w, header))
            .map_err(|error| error.to_string())
    } else {
        let _ = window.set_resizable(true);
        let _ = window.set_min_size(Some(PhysicalSize::new(
            NOTE_MIN_W as u32,
            (NOTE_MIN_H * window.scale_factor().unwrap_or(1.0)).round() as u32,
        )));
        window
            .set_size(PhysicalSize::new(w, h))
            .map_err(|error| error.to_string())
    };
    // 便签窗是用 ShowWindow(SW_SHOWNOACTIVATE) 显示的，tao 内部仍当它是隐藏的；
    // 一改窗口样式（set_resizable 会整份重设样式位），WS_VISIBLE 就被抹掉了。
    // 所以改完再按不抢焦点的方式显示一次。
    if let Ok(hwnd) = window.hwnd() {
        let raw = hwnd.0 as isize;
        hexbench_core::sys::show_window_no_activate(raw);
        // 样式是在 tao 的事件线程上排队的，可能比这一下还晚；稍后再露一次。
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            hexbench_core::sys::show_window_no_activate(raw);
        });
    }
    result
}

/// 便签窗打开时问一下自己是不是收起的（配置里记着）。
#[tauri::command]
fn note_rolled(window: tauri::WebviewWindow, app: AppHandle) -> bool {
    let label = window.label();
    note_pins(&app)
        .into_iter()
        .find(|pin| pin.label == label)
        .map(|pin| pin.rolled)
        .unwrap_or(false)
}

/// 发送一条笔记：收起面板 → 把焦点还给呼出前的窗口 → 逐字输入。
///
/// 这是一次性的「代打」：先确认目标窗口还在、前台确实切回去了，再开始输入。
/// 目标程序以管理员权限运行时 `SendInput` 会被 UIPI 静默丢掉，所以失败要把
/// 面板重新叫回来，并给一句人话，而不是让用户对着空输入框发呆。
#[tauri::command]
async fn notes_send(
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (root, send_enter) = {
        let shared = state.shared.lock().unwrap();
        (
            hexbench_core::notes::resolve_root(&shared.config.notes.folder)?,
            shared.config.notes.send_enter,
        )
    };
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let note = hexbench_core::notes::read(&root, &id)?;
        let text = hexbench_core::notes::send_text(&note.body);
        if text.is_empty() {
            return Err("这条便签没有可发送的文字".to_string());
        }
        let target = LAUNCHER_RETURN
            .lock()
            .ok()
            .and_then(|slot| *slot)
            .filter(|hwnd| hexbench_core::sys::window_valid(*hwnd));
        let Some(target) = target else {
            return Err("先切到要输入的窗口，再呼出面板".to_string());
        };
        hide_launcher(&handle, "note-send");
        thread::sleep(Duration::from_millis(120));
        on_main_thread(&handle, move || {
            hexbench_core::sys::force_foreground(target)
        })?;
        thread::sleep(Duration::from_millis(80));
        let mut front = hexbench_core::sys::foreground_hwnd();
        if front != Some(target) {
            on_main_thread(&handle, move || {
                hexbench_core::sys::force_foreground(target)
            })?;
            thread::sleep(Duration::from_millis(80));
            front = hexbench_core::sys::foreground_hwnd();
        }
        if front != Some(target) {
            reshow_launcher(&handle);
            return Err("无法切回目标窗口，可能已最小化，或被系统拦住".to_string());
        }
        if !hexbench_core::sys::type_text(&text) {
            reshow_launcher(&handle);
            return Err("发送失败：目标程序可能以管理员权限运行".to_string());
        }
        if send_enter && !hexbench_core::sys::press_enter() {
            reshow_launcher(&handle);
            return Err("文字已输入，但回车没有发出去".to_string());
        }
        log::info!("notes: sent {} char(s) to {target}", text.chars().count());
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 在中转站的当前目录里新建文件夹（重名自动加序号）。
/// 在某个目录下新建一个子文件夹（面板右键「新建分类」用）。
#[tauri::command]
fn launcher_create_dir(parent: String, name: String) -> Result<String, String> {
    let name = name.trim().trim_end_matches(['.', ' ']);
    if name.is_empty() {
        return Err("名字不能为空".to_string());
    }
    if name.chars().any(|c| "\\/:*?\"<>|".contains(c)) {
        return Err("名字里不能有 \\ / : * ? \" < > |".to_string());
    }
    let dir = PathBuf::from(&parent).join(name);
    std::fs::create_dir_all(&dir).map_err(|error| format!("建不了这个文件夹：{error}"))?;
    log::info!("launcher: created category {}", dir.display());
    Ok(dir.to_string_lossy().to_string())
}
#[tauri::command]
fn launcher_new_folder(parent: String) -> Result<String, String> {
    let target = hexbench_core::sys::create_folder(std::path::Path::new(&parent), "新建文件夹")?;
    let created = std::fs::metadata(&target)
        .ok()
        .and_then(|meta| meta.created().ok());
    push_undo(UndoOp::NewFolder {
        path: target.clone(),
        created,
    });
    Ok(target.to_string_lossy().to_string())
}
/// 某个分类（也就是某个子文件夹）里的条目。
#[tauri::command]
async fn launcher_items(path: String, with_icons: Option<bool>) -> Vec<LauncherItem> {
    // 图标要问 Shell（每张几十毫秒），必须离开界面线程，否则一屏图标就能把
    // 面板卡住一秒以上。也可以先只要名字，图标后面再补。
    let with_icons = with_icons.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        list_folder(std::path::Path::new(&path), with_icons)
    })
    .await
    .unwrap_or_default()
}

/// 流式列目录：一页一页推给面板，大目录也能立刻出前几屏。
///
/// 这里不排序——面板按自己的排序规则排，而且排序需要拿到全部条目，
/// 那就又退化成一次性加载了。
#[tauri::command]
async fn launcher_items_stream(
    path: String,
    on_page: tauri::ipc::Channel<LauncherPage>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        const PAGE: usize = 400;
        let mut batch: Vec<LauncherItem> = Vec::with_capacity(PAGE);
        let mut send_error: Option<String> = None;
        for_each_item(std::path::Path::new(&path), false, |item| {
            batch.push(item);
            if batch.len() < PAGE {
                return true;
            }
            let page = LauncherPage {
                items: std::mem::take(&mut batch),
                done: false,
            };
            if let Err(error) = on_page.send(page) {
                send_error = Some(error.to_string());
                return false;
            }
            true
        });
        if send_error.is_none() {
            let _ = on_page.send(LauncherPage {
                items: batch,
                done: true,
            });
        }
        send_error.map_or(Ok(()), Err)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 一次最多同时抽几张图。缩略图是「读盘 + 解码」，几路并行能把整屏的等待
/// 摊薄；再多就开始抢磁盘和 Shell 的缩略图服务，收益很小。
const ICON_WORKERS: usize = 6;

/// 后台抽好一张图标后推给前端的载荷。
#[derive(Clone, Serialize)]
struct LauncherIcon {
    path: String,
    key: Option<String>,
}

/// 抽取队列：两档优先级。屏幕上看得见的那一屏先抽，往后预取的排在后面——
/// 快速滚动时新一屏的请求不能被前面预取的几百张堵住。
/// （任务和唤醒信号分开：信号走 mpsc，任务留在队列里好重新排序。）
#[derive(Default)]
struct IconQueues {
    visible: std::collections::VecDeque<(AppHandle, String)>,
    ahead: std::collections::VecDeque<(AppHandle, String)>,
}

static ICON_QUEUE: Mutex<Option<IconQueues>> = Mutex::new(None);
static ICON_WAKE: std::sync::OnceLock<std::sync::mpsc::Sender<()>> = std::sync::OnceLock::new();

/// 固定 6 个长驻线程碰 Shell：滚得再快也不会一下子开几十个线程抢磁盘。
/// 线程里第一次取图时各自初始化 COM 公寓，之后一直复用。
fn icon_wake() -> &'static std::sync::mpsc::Sender<()> {
    ICON_WAKE.get_or_init(|| {
        let (wake, sleep) = std::sync::mpsc::channel::<()>();
        let sleep = Arc::new(Mutex::new(sleep));
        for _ in 0..ICON_WORKERS {
            let sleep = Arc::clone(&sleep);
            thread::spawn(move || loop {
                let woken = sleep
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .recv();
                if woken.is_err() {
                    return;
                }
                let task = {
                    let mut guard = ICON_QUEUE.lock().unwrap_or_else(|error| error.into_inner());
                    let queues = guard.get_or_insert_with(IconQueues::default);
                    queues
                        .visible
                        .pop_front()
                        .or_else(|| queues.ahead.pop_front())
                };
                if let Some((app, raw)) = task {
                    extract_icon(&app, &raw);
                }
            });
        }
        wake
    })
}

fn extract_icon(app: &AppHandle, raw: &str) {
    let path = PathBuf::from(raw);
    // 可见区、预取和滚动会重复请求同一批路径，先来先抽。
    let Some(_claim) = hexbench_core::icons::claim_extraction(&path) else {
        return;
    };
    let is_dir = path.is_dir();
    let mut key = hexbench_core::icons::icon_key(&path, is_dir);
    if key.is_none() {
        // Shell 偶发抽风一次就被记 5 分钟，这里立刻抹掉重试，
        // 别让一次失败变成「这个文件永远只有图标」。
        hexbench_core::icons::forget_miss(&path);
        thread::sleep(Duration::from_millis(150));
        key = hexbench_core::icons::icon_key(&path, is_dir);
    }
    let _ = app.emit(
        "launcher-icon",
        LauncherIcon {
            path: raw.to_string(),
            key,
        },
    );
}

/// 批量补图标：`(路径, 缓存键)`，前端用 `hbicon://<键>` 直接取图。
///
/// 缓存命中的随返回值立刻给；没命中的丢到抽取队列，**抽完一张发一条
/// `launcher-icon`**——不能等整批，一批里最慢的那张（大视频、网络盘）
/// 会把整批已经抽好的图标一起扣住，看起来就是「缩略图加载得很慢」。
#[tauri::command]
async fn launcher_icons(
    app: AppHandle,
    paths: Vec<String>,
    ahead: Option<Vec<String>>,
) -> Vec<(String, Option<String>)> {
    let mut ready = Vec::new();
    let mut visible_todo = Vec::new();
    let mut later_todo = Vec::new();
    for raw in paths {
        let path = PathBuf::from(&raw);
        match hexbench_core::icons::cached_icon_key(&path) {
            Some(key) => ready.push((raw, key)),
            None => visible_todo.push(raw),
        }
    }
    for raw in ahead.unwrap_or_default() {
        let path = PathBuf::from(&raw);
        match hexbench_core::icons::cached_icon_key(&path) {
            Some(key) => ready.push((raw, key)),
            None => later_todo.push(raw),
        }
    }
    if visible_todo.is_empty() && later_todo.is_empty() {
        return ready;
    }

    let queued = visible_todo.len() + later_todo.len();
    {
        let mut guard = ICON_QUEUE.lock().unwrap_or_else(|error| error.into_inner());
        let queues = guard.get_or_insert_with(IconQueues::default);
        for raw in visible_todo {
            // 之前作为预取排进队的，现在滚到眼前了：从后面挪到前面。
            queues.ahead.retain(|(_, path)| *path != raw);
            queues.visible.push_back((app.clone(), raw));
        }
        for raw in later_todo {
            queues.ahead.push_back((app.clone(), raw));
        }
    }
    let wake = icon_wake();
    for _ in 0..queued {
        let _ = wake.send(());
    }

    ready
}

/// 面板里按住一个条目拖出去：交给 OLE，资源管理器 / 浏览器都认。
///
/// `DoDragDrop` 必须留在主线程上：它靠 `SetCapture` 拿鼠标，而只有前台线程拿得到，
/// 换到别的线程上松手时那段模态循环收不到消息，会一直挂着不返回。
/// 代价是拖拽期间主线程不理 WebView2 的消息——`hbicon://` 的图片请求和取图标的命令
/// 都排着队。所以那期间才显示的缩略图得提前备在渲染进程里（前端的 `warmPreview`）。
#[tauri::command]
fn launcher_begin_drag(paths: Vec<String>, app: AppHandle) -> Result<u32, String> {
    let targets: Vec<PathBuf> = paths
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect();
    if targets.is_empty() {
        log::warn!("launcher: drag targets all missing: {paths:?}");
        return Err("文件已经不在了".to_string());
    }
    log::info!("launcher: drag requested for {} item(s)", targets.len());
    run_file_drag(&app, "launcher", &targets)
}

/// 拖拽本体：建数据对象 → OLE 模态循环 → 松手时把落点交回界面。
///
/// `label` 是发起拖拽的窗口（快捷面板或某个桌面格子）：框选判断、客户区坐标
/// 和事件都按它来，事件名前缀也是 `launcher-*` / `fence-*`。
fn run_file_drag(app: &AppHandle, label: &str, targets: &[PathBuf]) -> Result<u32, String> {
    let prefix = if label == "launcher" { "launcher" } else { "fence" };
    // 面板在屏幕上的位置：松手时用它判断「还在面板里」。
    let frame = app.get_webview_window(label).and_then(|window| {
        let position = window.outer_position().ok()?;
        let size = window.inner_size().ok()?;
        Some((
            position.x,
            position.y,
            size.width as i32,
            size.height as i32,
        ))
    });

    // 拖拽期间轮询光标：面板自己的拖放目标收不到 OLE 事件，只能自己看光标在哪。
    // 坐标要换成面板的客户区坐标——页面里的 `window.screenX/screenY` 在窗口
    // 挪过位置以后不更新，交给前端去减会整体偏掉（拖着文件指哪都不亮）。
    let hwnd = app
        .get_webview_window(label)
        .and_then(|window| window.hwnd().ok())
        .map(|handle| handle.0 as isize);
    let stop = Arc::new(AtomicBool::new(false));
    let pos_event = format!("{prefix}-drag-pos");
    let drop_event = format!("{prefix}-drag-drop");
    let poller = {
        let app = app.clone();
        let stop = stop.clone();
        let pos_event = pos_event.clone();
        let label = label.to_string();
        thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let (x, y) = hexbench_core::sys::cursor_position();
                let (x, y) = match hwnd {
                    Some(hwnd) => hexbench_core::sys::screen_to_client(hwnd, x, y),
                    None => (x, y),
                };
                let _ = app.emit_to(
                    label.as_str(),
                    pos_event.as_str(),
                    serde_json::json!({ "x": x, "y": y }),
                );
                thread::sleep(Duration::from_millis(25));
            }
        })
    };

    hexbench_core::drop_target::set_drag_source(hwnd.unwrap_or(-1));
    let result = hexbench_core::sys::start_file_drag(targets);
    hexbench_core::drop_target::clear_drag_source();

    stop.store(true, Ordering::SeqCst);
    let _ = poller.join();

    // 松手时如果还在面板里、而且左键已经松开（不是 Esc 取消），就把落点交给界面：
    // 落在文件夹上 = 移进去，落在空白 = 放进当前目录。
    if !hexbench_core::sys::left_button_down() {
        if let Some((left, top, width, height)) = frame {
            let (x, y) = hexbench_core::sys::cursor_position();
            if x >= left && y >= top && x < left + width && y < top + height {
                let (x, y) = match hwnd {
                    Some(hwnd) => hexbench_core::sys::screen_to_client(hwnd, x, y),
                    None => (x, y),
                };
                let list: Vec<String> = targets
                    .iter()
                    .map(|path| path.to_string_lossy().to_string())
                    .collect();
                let _ = app.emit_to(
                    label,
                    drop_event.as_str(),
                    serde_json::json!({ "x": x, "y": y, "paths": list }),
                );
            }
        }
    }

    match result {
        Ok(effect) => {
            log::info!("launcher: dragged out {} item(s) effect={effect}", targets.len());
            Ok(effect)
        }
        Err(error) => {
            log::warn!("launcher: drag failed: {error}");
            Err(error)
        }
    }
}
/// 撤销栈：记录最近能反悔的操作。
enum UndoOp {
    /// 删进回收站的文件（撤销 = 从回收站还原）
    Trash(Vec<PathBuf>),
    /// 重命名：（新路径, 旧路径）
    Rename { path: PathBuf, previous: PathBuf },
    /// 移动：（新路径, 旧路径）成对
    Move(Vec<(PathBuf, PathBuf)>),
    /// 新建文件夹：（路径, 建的时候的创建时间，用来确认撤销时还是同一个）
    NewFolder {
        path: PathBuf,
        created: Option<std::time::SystemTime>,
    },
}

static UNDO_STACK: Mutex<Vec<UndoOp>> = Mutex::new(Vec::new());

fn push_undo(op: UndoOp) {
    if let Ok(mut stack) = UNDO_STACK.lock() {
        stack.push(op);
        // 只留最近 30 步，够用又不占内存
        while stack.len() > 30 {
            stack.remove(0);
        }
    }
}

/// Ctrl+Z：撤销上一步。
#[tauri::command(async)]
fn launcher_undo(app: AppHandle) -> Result<String, String> {
    let op = UNDO_STACK.lock().ok().and_then(|mut stack| stack.pop());
    let Some(op) = op else {
        return Err("没有可撤销的操作".to_string());
    };
    let message = match op {
        UndoOp::Trash(paths) => {
            let mut restored = 0usize;
            let mut last_error = String::new();
            for path in &paths {
                match hexbench_core::shell_ops::restore_trashed(path) {
                    Ok(()) => restored += 1,
                    Err(error) => last_error = error,
                }
            }
            if restored == 0 {
                return Err(last_error);
            }
            format!("已从回收站还原 {restored} 个文件")
        }
        UndoOp::Rename { path, previous } => {
            // Windows 的 rename 会直接覆盖目标：先确认改过名的那个还在、
            // 原来的位置已经腾出来了。否则可能把用户后来新建的同名文件盖掉。
            if !path.exists() {
                return Err("改过名的文件已经不在了".to_string());
            }
            if previous.exists() {
                return Err(format!("{} 已经有同名文件了", previous.display()));
            }
            std::fs::rename(&path, &previous).map_err(|error| error.to_string())?;
            format!("已还原名称：{}", previous.display())
        }
        UndoOp::Move(entries) => {
            let mut moved = 0usize;
            for (from, to) in &entries {
                if from.exists() && !to.exists() {
                    if let Err(error) = std::fs::rename(from, to) {
                        log::warn!("undo move: {error}");
                        continue;
                    }
                    moved += 1;
                }
            }
            if moved == 0 {
                return Err("这些文件已经不在原来的位置了".to_string());
            }
            format!("已移回 {moved} 个文件")
        }
        UndoOp::NewFolder { path, created } => {
            let meta = std::fs::metadata(&path)
                .map_err(|_| "刚新建的文件夹已经不在了".to_string())?;
            if !meta.is_dir() {
                return Err("这个位置已经不是文件夹了".to_string());
            }
            // 同名文件夹可能已经被删掉又建了一个：创建时间对不上就别动它。
            if let (Some(before), Ok(now)) = (created, meta.created()) {
                if before != now {
                    return Err("这个位置现在换成了另一个文件夹".to_string());
                }
            }
            // 建完之后又往里面放了东西：撤销新建不该顺带把内容也删掉。
            let empty = std::fs::read_dir(&path)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
            if !empty {
                return Err("文件夹里已经有东西了，确认后再手动删除".to_string());
            }
            hexbench_core::shell_ops::trash(&[path]).map_err(|error| error.to_string())?;
            "已删除刚新建的文件夹".to_string()
        }
    };
    let _ = app.emit("launcher-changed", ());
    log::info!("undo: {message}");
    Ok(message)
}
/// 删除到回收站（系统会弹确认框）。
#[tauri::command(async)]
fn launcher_trash(paths: Vec<String>, app: AppHandle) -> Result<usize, String> {
    log::info!("trash: {} items", paths.len());
    // 多选里可能混着刚被别处删掉 / 移走的路径：跳过它们，别让一个失效的
    // 条目把整组操作顶掉（IFileOperation 只要有一项建不出来就整批不做）。
    let total = paths.len();
    let targets: Vec<PathBuf> = paths
        .into_iter()
        .map(PathBuf::from)
        // symlink_metadata：断掉的快捷方式 / 链接本身也算存在，得能删掉。
        .filter(|path| std::fs::symlink_metadata(path).is_ok())
        .collect();
    if targets.is_empty() {
        log::warn!("trash: nothing to delete, all {total} path(s) are gone");
        return Err("文件已经不在了".to_string());
    }
    if targets.len() < total {
        log::warn!("trash: skipped {} of {total} gone path(s)", total - targets.len());
    }
    let removed = hexbench_core::shell_ops::trash(&targets)?;
    if removed > 0 {
        push_undo(UndoOp::Trash(targets));
        // 别的窗口 / 面板的钉住文件夹列表也要知道目录变了。
        let _ = app.emit("launcher-changed", ());
    }
    Ok(removed)
}

/// 把选中项放上剪贴板（cut=true 表示剪切）。
#[tauri::command]
fn launcher_clipboard_copy(paths: Vec<String>, cut: bool) -> Result<(), String> {
    log::info!("clipboard: copy {} items cut={cut}", paths.len());
    // 同上：失效的路径不放进剪贴板，剩下的照常复制。
    let targets: Vec<PathBuf> = paths
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| std::fs::symlink_metadata(path).is_ok())
        .collect();
    if targets.is_empty() {
        return Err("文件已经不在了".to_string());
    }
    hexbench_core::shell_ops::clipboard_set_files(&targets, cut)
}

/// 粘贴剪贴板里的文件到目标目录。
#[tauri::command(async)]
fn launcher_paste(dest: String, app: AppHandle) -> Result<usize, String> {
    log::info!("clipboard: paste into {dest}");
    let pasted = hexbench_core::shell_ops::paste_into(&PathBuf::from(dest))?;
    if pasted > 0 {
        let _ = app.emit("launcher-changed", ());
    }
    Ok(pasted)
}
/// 面板菜单里调起的 Shell 命令（压缩、复制、发送到…）跑完之前，别因为前台被
/// 它的进度窗口拿走就收起面板。
///
/// 值为「至少要守到什么时候」（毫秒时间戳）。这些命令都是异步的：
/// `InvokeCommand` 立刻返回，进度窗口过一会儿才出现；目标目录还在变就说明活儿
/// 没干完，截止时间顺延。
static SHELL_OP_UNTIL: AtomicU64 = AtomicU64::new(0);
/// 已经有线程在等这次命令结束、准备收面板了，别重复起。
static SHELL_OP_WATCHING: AtomicBool = AtomicBool::new(false);

const SHELL_OP_GRACE_MS: u64 = 2500;

fn shell_op_active() -> bool {
    now_millis() < SHELL_OP_UNTIL.load(Ordering::Relaxed)
}

/// 目录的「动静」：条目数、最新修改时间、总大小。压缩 / 复制这类操作会持续改它。
///
/// 只看前 4000 项：这是给「命令还在不在跑」用的粗略信号，不是清单，
/// 大目录每 250ms 全量 stat 一遍不值得。
fn dir_activity(path: &std::path::Path) -> (usize, u128, u64) {
    let mut count = 0usize;
    let mut newest = 0u128;
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if count >= 4000 {
                break;
            }
            count += 1;
            if let Ok(meta) = entry.metadata() {
                total = total.saturating_add(meta.len());
                if let Ok(modified) = meta.modified() {
                    if let Ok(delta) = modified.duration_since(std::time::UNIX_EPOCH) {
                        newest = newest.max(delta.as_millis());
                    }
                }
            }
        }
    }
    (count, newest, total)
}

/// 记下「刚从面板菜单发了一条 Shell 命令」，并盯住目录给它续期。
fn watch_shell_op(dir: PathBuf) {
    SHELL_OP_UNTIL.store(now_millis() + SHELL_OP_GRACE_MS, Ordering::Relaxed);
    thread::spawn(move || {
        let mut last = dir_activity(&dir);
        let mut idle = 0;
        // 最多盯 5 分钟：真有超长的活儿，用户自己点回来也不会被收。
        for _ in 0..1200 {
            thread::sleep(Duration::from_millis(250));
            let now = dir_activity(&dir);
            if now != last {
                last = now;
                idle = 0;
                SHELL_OP_UNTIL.store(now_millis() + SHELL_OP_GRACE_MS, Ordering::Relaxed);
            } else {
                idle += 1;
                if idle >= 10 {
                    break;
                }
            }
        }
    });
}

/// Shell 命令期间没收面板：命令结束、前台也不再是它了，再补一次「该收了」。
fn hide_after_shell_op(app: AppHandle) {
    if SHELL_OP_WATCHING.swap(true, Ordering::Relaxed) {
        return;
    }
    thread::spawn(move || {
        while shell_op_active() {
            thread::sleep(Duration::from_millis(250));
            let visible = app
                .get_webview_window("launcher")
                .map(|window| panel_visible(&window).unwrap_or(false))
                .unwrap_or(false);
            if !visible {
                SHELL_OP_WATCHING.store(false, Ordering::Relaxed);
                return;
            }
        }
        SHELL_OP_WATCHING.store(false, Ordering::Relaxed);
        // 命令刚结束那一会儿前台可能还在切换，稍等一下再看。
        thread::sleep(Duration::from_millis(400));
        if shell_op_active() || hexbench_core::sys::foreground_is_current_process() {
            return;
        }
        if let Some(window) = app.get_webview_window("launcher") {
            if panel_visible(&window) == Some(true) {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    hide_launcher(&handle, "focus-lost (shell done)");
                });
            }
        }
    });
}
/// 文件夹空白处的原生右键菜单（新建 / 粘贴 / 属性 / 刷新）。
#[tauri::command]
fn launcher_background_menu(
    path: String,
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<(), String> {
    log::info!("bg menu: invoked for {path}");
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err("目录已经不在了".to_string());
    }
    let (x, y) = hexbench_core::sys::cursor_position();
    // 菜单挂在调用它的窗口上：快捷面板和桌面格子共用一个命令。
    let owner = window.hwnd().ok().map(|hwnd| hwnd.0 as isize).unwrap_or(0);
    // 剪贴板里有文件时才让「粘贴」可用
    let can_paste = hexbench_core::shell_ops::clipboard_get_files().is_some();
    LAUNCHER_MENU_OPEN.store(true, Ordering::Relaxed);
    let result = hexbench_core::sys::show_folder_background_menu(&dir, x, y, owner, can_paste);
    LAUNCHER_MENU_OPEN.store(false, Ordering::Relaxed);
    // 「新建」是我们自己实现的，同步建好就直接刷新并进改名——资源管理器的手感。
    let choice = result.as_ref().ok().and_then(|value| *value);
    match choice {
        Some("paste") => {
            if let Err(error) = hexbench_core::shell_ops::paste_into(&dir) {
                log::warn!("background menu: paste failed: {error}");
            }
        }
        Some("new-folder") | Some("new-text") => {
            let created = if choice == Some("new-folder") {
                hexbench_core::sys::create_folder(&dir, "新建文件夹")
            } else {
                hexbench_core::sys::create_text_file(&dir, "新建文本文档")
            };
            return match created {
                Ok(path) => {
                    let _ = app.emit("launcher-changed", ());
                    let _ = app.emit("launcher-rename", path.to_string_lossy().to_string());
                    Ok(())
                }
                Err(error) => Err(error),
            };
        }
        _ => {}
    }

    // Explorer 的「新建 ▸ 文件夹」是先建出来再进入重命名的异步操作：
    // 菜单一关就刷新往往太早，新条目还没落盘。这里轮询几秒，
    // 目录一变就通知面板刷新；就算没变，最后也补一次。
    let watch_dir = dir.clone();
    let handle = app.clone();
    thread::spawn(move || {
        let snapshot = |path: &std::path::Path| -> (usize, u128) {
            let mut count = 0usize;
            let mut newest = 0u128;
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    count += 1;
                    if let Ok(meta) = entry.metadata() {
                        if let Ok(modified) = meta.modified() {
                            if let Ok(delta) = modified.duration_since(std::time::UNIX_EPOCH) {
                                newest = newest.max(delta.as_millis());
                            }
                        }
                    }
                }
            }
            (count, newest)
        };
        let before = snapshot(&watch_dir);
        for _ in 0..15 {
            thread::sleep(Duration::from_millis(200));
            if snapshot(&watch_dir) != before {
                let _ = handle.emit("launcher-changed", ());
                return;
            }
        }
        let _ = handle.emit("launcher-changed", ());
    });

    result.map(|_| ())
}
/// 面板里右键：系统原生菜单 + 自己补的重命名。
///
/// `paths` 是多选时的整组文件，菜单里的剪切 / 复制 / 删除会一起作用到这一组；
/// 第一项必须是右键点中的那个（其它目录的条目会被后端过滤掉）。
///
/// 返回 `"rename"` 表示用户选了重命名（改名要用户输入，交给界面做），
/// `"deleted"` 表示删到回收站了，`None` 表示没选或执行了系统命令。
#[tauri::command]
fn launcher_context_menu(
    paths: Vec<String>,
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<Option<String>, String> {
    let targets: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let Some(target) = targets.first().cloned() else {
        return Err("没有选中文件".to_string());
    };
    if !target.exists() {
        log::warn!(
            "launcher: context menu skipped, file is gone: {}",
            target.display()
        );
        return Err("文件已经不在了".to_string());
    }
    let (x, y) = hexbench_core::sys::cursor_position();
    let owner = window.hwnd().ok().map(|hwnd| hwnd.0 as isize).unwrap_or(0);
    log::info!(
        "launcher: context menu for {} item(s) at {x},{y}",
        targets.len()
    );
    LAUNCHER_MENU_OPEN.store(true, Ordering::Relaxed);
    let result = hexbench_core::sys::show_shell_context_menu_for(&targets, x, y, owner, || {
        // 先把「命令可能还在跑」的标记戴上：InvokeCommand 本身就可能等进度
        // 窗口，这段时间主线程已经会收到失焦了。菜单里选的可能不是系统命令
        // （改名），那也最多多守几秒。
        SHELL_OP_UNTIL.store(now_millis() + SHELL_OP_GRACE_MS, Ordering::Relaxed);
    });
    LAUNCHER_MENU_OPEN.store(false, Ordering::Relaxed);
    match result {
        Ok(choice) => {
            // 菜单关掉之后窗口不一定还拿着键盘焦点；要改名就必须先把焦点抢回来，
            // 否则输入框看得见却收不到按键。

            let _ = app.emit("launcher-changed", ());
            match choice {
                Some("shell") => {
                    // 系统命令（压缩 / 复制 / 发送到…）大多还在后台跑，会弹自己的
                    // 进度窗口：这段时间别把面板收起，否则用户刚点的东西看不到了。
                    if let Some(dir) = target.parent() {
                        watch_shell_op(dir.to_path_buf());
                    }
                    // 菜单开着时收到的失焦会被忽略，那一刻面板就已经不在前台了；
                    // 之后不一定再有失焦事件，所以这里直接把「活儿干完再收」排上。
                    hide_after_shell_op(app.clone());
                    Ok(None)
                }
                other => Ok(other.map(|value| value.to_string())),
            }
        }
        Err(error) => {
            log::warn!("launcher: context menu failed: {error}");
            Err(error)
        }
    }
}

/// 在面板里给文件改名（同一目录内）。
#[tauri::command(async)]
fn launcher_rename(path: String, name: String, app: AppHandle) -> Result<String, String> {
    log::info!("launcher: rename {path} -> {name}");
    let target = PathBuf::from(&path);
    let renamed = match hexbench_core::sys::rename_path(&target, &name) {
        Ok(value) => value,
        Err(error) => {
            log::warn!("launcher: rename failed: {error}");
            return Err(error);
        }
    };
    push_undo(UndoOp::Rename {
        path: renamed.clone(),
        previous: target.clone(),
    });
    let _ = app.emit("launcher-changed", ());
    Ok(renamed.to_string_lossy().to_string())
}
/// 设置页「缓存」：每一项缓存的占用。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CacheInfo {
    key: String,
    label: String,
    hint: String,
    path: String,
    bytes: u64,
    files: usize,
}

#[tauri::command]
fn cache_stats() -> Vec<CacheInfo> {
    let mut list = Vec::new();
    if let Some(dir) = hexbench_core::icons::cache_dir() {
        let (bytes, files) = hexbench_core::icons::cache_usage();
        list.push(CacheInfo {
            key: "thumbs".to_string(),
            label: "缩略图缓存".to_string(),
            hint: "面板里的文件缩略图和图标。删掉不影响使用，下次打开时会重新生成。".to_string(),
            path: dir.to_string_lossy().to_string(),
            bytes,
            files,
        });
    }
    list
}

/// 清空所有缓存，返回释放的字节数。
#[tauri::command]
fn cache_clear(app: AppHandle) -> Result<u64, String> {
    let freed = hexbench_core::icons::clear_cache();
    log::info!("cache: cleared {freed} bytes");
    let _ = app.emit("launcher-changed", ());
    Ok(freed)
}
/// 使用频率表：第一次用到时从盘上读，之后常驻内存。
static USAGE: Mutex<Option<hexbench_core::usage::UsageData>> = Mutex::new(None);

/// 拿使用频率表做点什么（读计数 / 记一次打开）。第一次调用时才读盘。
fn with_usage<R>(app: &AppHandle, work: impl FnOnce(&mut hexbench_core::usage::UsageData) -> R) -> R {
    let path = hexbench_core::usage::file_of(&app.state::<AppState>().config_path);
    let mut guard = USAGE.lock().unwrap_or_else(|poison| poison.into_inner());
    let data = guard.get_or_insert_with(|| hexbench_core::usage::load(&path));
    work(data)
}

/// 打开成功后记一次：面板里「使用频率」排序就是按它来的。
fn record_use(app: &AppHandle, path: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let file = hexbench_core::usage::file_of(&app.state::<AppState>().config_path);
    let result = with_usage(app, |data| {
        data.record(path, now);
        data.trim();
        hexbench_core::usage::save(&file, data)
    });
    if let Err(error) = result {
        log::warn!("usage: 写不进 {}（{error}）", file.display());
    }
}

/// 面板要的「每个文件打开过几次」，路径和列表里的写法一致。
#[tauri::command]
fn launcher_usage(app: AppHandle) -> std::collections::HashMap<String, u32> {
    with_usage(&app, |data| {
        data.items
            .iter()
            .map(|(path, usage)| (path.clone(), usage.count))
            .collect()
    })
}
/// 面板里双击：交给系统默认程序。
#[tauri::command]
fn launcher_open(path: String, app: AppHandle) -> Result<(), String> {
    hexbench_core::sys::open_path(PathBuf::from(&path).as_path())?;
    record_use(&app, &path);
    Ok(())
}
/// 选一个文件夹作为面板的文件区。
/// 选文件夹。`target` 为 `"root"` 时设置启动器根目录，否则设置文件区。
#[tauri::command]
fn launcher_pick_folder(target: String, app: AppHandle) {
    use tauri_plugin_dialog::DialogExt;
    let handle = app.clone();
    let is_root = target == "root";
    app.dialog().file().pick_folder(move |picked| {
        let Some(picked) = picked else {
            return;
        };
        let Some(path) = picked.as_path() else {
            return;
        };
        let folder = path.to_string_lossy().to_string();
        let state = handle.state::<AppState>();
        let config = {
            let mut shared = state.shared.lock().unwrap();
            if is_root {
                shared.config.launcher.root = folder.clone();
            } else if !shared.config.launcher.folders.contains(&folder) {
                shared.config.launcher.folders.push(folder.clone());
            }
            shared.config.clone()
        };
        if let Err(error) = config::save(&state.config_path, &config) {
            log::warn!("launcher: save folder failed: {error}");
            return;
        }
        log::info!("launcher: folder set to {folder}");
        // 选文件夹时面板会失焦收起，选完再把它叫回来，别让用户以为操作丢了。
        if let Some(window) = handle.get_webview_window("launcher") {
            reveal_launcher(&window, launcher_appearance(&handle));
        }
        let _ = handle.emit("launcher-changed", ());
    });
}

#[tauri::command]
fn launcher_clear_folder(app: AppHandle, state: State<'_, AppState>) {
    let config = {
        let mut shared = state.shared.lock().unwrap();
        shared.config.launcher.folders.clear();
        shared.config.clone()
    };
    let _ = config::save(&state.config_path, &config);
    let _ = app.emit("launcher-changed", ());
}

/// 保存「常用文件夹」的完整列表（增删和拖动排序都走这里）。
#[tauri::command]
fn launcher_save_folders(folders: Vec<String>, app: AppHandle, state: State<'_, AppState>) {
    let config = {
        let mut shared = state.shared.lock().unwrap();
        let cleaned: Vec<String> = folders
            .into_iter()
            .map(|entry| entry.trim().to_string())
            .filter(|entry| !entry.is_empty())
            .collect();
        if shared.config.launcher.folders == cleaned {
            return;
        }
        shared.config.launcher.folders = cleaned;
        shared.config.clone()
    };
    if let Err(error) = config::save(&state.config_path, &config) {
        log::warn!("launcher: save folders failed: {error}");
        return;
    }
    let _ = app.emit("launcher-changed", ());
}

/// 保存分类的显示顺序（设置页拖动排序）。
#[tauri::command]
fn launcher_save_category_order(order: Vec<String>, app: AppHandle, state: State<'_, AppState>) {
    let config = {
        let mut shared = state.shared.lock().unwrap();
        if shared.config.launcher.category_order == order {
            return;
        }
        shared.config.launcher.category_order = order;
        shared.config.clone()
    };
    if let Err(error) = config::save(&state.config_path, &config) {
        log::warn!("launcher: save category order failed: {error}");
        return;
    }
    let _ = app.emit("launcher-changed", ());
}

/// 监听映射文件夹的变化：文件系统事件驱动，空闲时零开销。
/// 格子当前浏览的子目录（id → 路径）。watcher 除了根目录，也要跟着这一层，
/// 不然进了子文件夹之后拖进去的文件不会立刻出现。
static FENCE_WATCH: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// 格子进入 / 退出子目录时更新。传空路径 = 只看根目录。
/// 列目录对「不存在」和「空目录」都给空列表；界面靠这个区分该显示
/// 「这里是空的」还是「文件夹不在了」。
#[tauri::command]
fn fence_path_exists(path: String) -> bool {
    !path.trim().is_empty() && PathBuf::from(path).is_dir()
}

#[tauri::command]
fn fence_watch_path(id: String, path: String) {
    if let Ok(mut slots) = FENCE_WATCH.lock() {
        slots.retain(|(slot_id, _)| *slot_id != id);
        if !path.trim().is_empty() {
            slots.push((id, path));
        }
    }
}

fn spawn_launcher_watcher(app: AppHandle) {
    thread::spawn(move || {
        use notify::{RecursiveMode, Watcher};
        use std::sync::mpsc::RecvTimeoutError;

        type Session = (
            notify::RecommendedWatcher,
            std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
        );
        // 监听「根目录 + 所有常用文件夹」：任意一处变化都让面板刷新。
        let mut current: Vec<PathBuf> = Vec::new();
        // 监听器和它的接收端必须一起活着：丢了监听器就等于丢了监听。
        let mut session: Option<Session> = None;

        loop {
            let wanted = {
                let state = app.state::<AppState>();
                let shared = state.shared.lock().unwrap();
                let mut list: Vec<PathBuf> = Vec::new();
                let root = shared.config.launcher.root.clone();
                if !root.trim().is_empty() {
                    list.push(PathBuf::from(root));
                }
                for folder in &shared.config.launcher.folders {
                    if !folder.trim().is_empty() {
                        list.push(PathBuf::from(folder));
                    }
                }
                // 桌面格子绑定的文件夹也一起看：拖进去的文件要立刻出现在格子里。
                for fence in &shared.config.desktop.fences {
                    if !fence.path.trim().is_empty() {
                        list.push(PathBuf::from(&fence.path));
                    }
                    // 整理规则的来源目录（桌面 / 下载…）：一有变化就自动收。
                    for rule in &fence.rules {
                        if !rule.enabled {
                            continue;
                        }
                        for source in &rule.sources {
                            if !source.trim().is_empty() {
                                list.push(PathBuf::from(source.trim()));
                            }
                        }
                    }
                }
                // 格子当前进到的子目录。
                if let Ok(slots) = FENCE_WATCH.lock() {
                    for (_, path) in slots.iter() {
                        if !path.trim().is_empty() {
                            list.push(PathBuf::from(path));
                        }
                    }
                }
                list
            };

            if wanted != current {
                session = None;
                let (sender, rx) = std::sync::mpsc::channel();
                let mut watched = 0usize;
                if let Ok(mut created) = notify::recommended_watcher(move |event| {
                    let _ = sender.send(event);
                }) {
                    for directory in &wanted {
                        if directory.is_dir()
                            && created.watch(directory, RecursiveMode::NonRecursive).is_ok()
                        {
                            watched += 1;
                        }
                    }
                    if watched > 0 {
                        log::info!("launcher: watching {watched} directory(ies)");
                        session = Some((created, rx));
                    }
                }
                // 一个都没挂上（磁盘 / 网络盘暂时不在）就不要记成「已监视」：
                // 留到下一轮再试，盘回来之后才能自动刷新。
                if watched > 0 || wanted.is_empty() {
                    current = wanted;
                }
            }

            match session.as_ref() {
                Some((_watcher, rx)) => match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(Ok(first)) => {
                        // 解压 / 批量复制会连发几百个事件，合并成一次刷新。
                        let mut changed: Vec<PathBuf> = first.paths.clone();
                        thread::sleep(Duration::from_millis(250));
                        while let Ok(Ok(next)) = rx.try_recv() {
                            changed.extend(next.paths);
                        }
                        // 落进某个格子的根目录（或它下面的路径）就只通知那一格。
                        let fences = {
                            let state = app.state::<AppState>();
                            let shared = state.shared.lock().unwrap();
                            shared.config.desktop.fences.clone()
                        };
                        for fence in fences {
                            let root = fence.path.replace('\\', "/").to_lowercase();
                            let hit = changed.iter().any(|path| {
                                let text = path.to_string_lossy().replace('\\', "/").to_lowercase();
                                text == root || text.starts_with(&format!("{root}/"))
                            });
                            if hit {
                                let _ = app.emit_to(
                                    format!("fence-{}", fence.id),
                                    "fence-changed",
                                    serde_json::json!({ "path": fence.path }),
                                );
                                // 原生格子没有页面自己刷新，watcher 直接推新内容。
                                native_refresh(&app, &fence.id);
                            }
                        }
                        // 格子正在看的子目录：变化就让它自己刷新（前端只认事件，
                        // 再按当前路径重新列一遍）。
                        let slots = FENCE_WATCH
                            .lock()
                            .map(|slots| slots.clone())
                            .unwrap_or_default();
                        for (id, watched) in slots {
                            let watched = watched.replace('\\', "/").to_lowercase();
                            if watched.is_empty() {
                                continue;
                            }
                            let hit = changed.iter().any(|path| {
                                let text =
                                    path.to_string_lossy().replace('\\', "/").to_lowercase();
                                text == watched || text.starts_with(&format!("{watched}/"))
                            });
                            if hit {
                                let _ = app.emit_to(
                                    format!("fence-{id}"),
                                    "fence-changed",
                                    serde_json::json!({ "path": watched }),
                                );
                                native_refresh(&app, &id);
                            }
                        }
                        // 规则盒的来源目录有变化：等安静下来再按规则收。
                        mark_rules_dirty(&app, &changed);
                        let _ = app.emit("launcher-changed", ());
                    }
                    Ok(Err(_)) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        session = None;
                        current.clear();
                    }
                },
                None => thread::sleep(Duration::from_millis(500)),
            }
        }
    });
}

/// 面板里按 Esc：前端喊一声，这里收起来。
#[tauri::command]
fn dismiss_launcher(app: AppHandle) {
    hide_launcher(&app, "dismiss");
}

// ---------- 桌面格子 ----------
//
// 每格一个独立的无边框窗口，锚在桌面图标宿主（SHELLDLL_DefView）正上方、
// 普通窗口之下（做法参考 PecoFence：不 re-parent，只插 z-order 然后看门狗盯着）。
// 窗口建在一个长期存活的 worker 线程上：WebView2 在 UI 线程建会重入消息循环，
// 而且拖放目标必须挂在建窗的线程上（见 build_launcher 的注释）。
//
// 几何两份：按显示器工作区归一化的 `x/y/w/h`（分辨率 / DPI / 任务栏变化后按比例
// 恢复），以及上次的物理矩形（显示器找不到时兜底）。`h` 永远是展开高度，卷起只是
// 把窗口实际高度收到标题条。

/// 最小尺寸（逻辑像素）。
const FENCE_MIN_W_LOGICAL: f64 = 240.0;
const FENCE_MIN_H_LOGICAL: f64 = 160.0;
/// 格子数量软上限。
const FENCE_MAX: usize = 8;

static FENCE_SEQ: AtomicU64 = AtomicU64::new(1);
/// 命令改完配置后踢一下管理器，不用等下一轮 1s 轮询。
static FENCE_WAKE: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);

/// 收纳盒的幕后目录放在「文档」下：用户看得见、能备份，清理软件也不会
/// 当成缓存删掉（放 %LOCALAPPDATA% 里迟早会丢用户文件）。
fn fence_store_root() -> Result<PathBuf, String> {
    let base = hexbench_core::sys::documents_dir()
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .ok_or("找不到用户目录")?;
    Ok(base.join("HexBench 收纳"))
}

/// 目录名里不能出现的字符换成 `_`。标题之后随时能改，目录建好就不再动。
fn sanitize_dir_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "收纳盒".to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
}

/// `child` 是否在 `root` 里面（含 root 自己）。规则引擎用它跳过盒子自己
/// 和所有幕后目录，避免「收进去又被当成新文件收一次」。
fn is_under_path(child: &Path, root: &Path) -> bool {
    let child = child.to_string_lossy().replace('\\', "/").to_lowercase();
    let root = root.to_string_lossy().replace('\\', "/").to_lowercase();
    let root = root.trim_end_matches('/');
    !root.is_empty() && (child == root || child.starts_with(&format!("{root}/")))
}

fn desktop_snapshot(app: &AppHandle) -> DesktopSettings {
    app.state::<AppState>()
        .shared
        .lock()
        .unwrap()
        .config
        .desktop
        .clone()
}

fn wake_fence_manager() {
    if let Ok(slot) = FENCE_WAKE.lock() {
        if let Some(tx) = slot.as_ref() {
            let _ = tx.send(());
        }
    }
}

/// 改完 `desktop` 之后统一落盘：写配置、通知设置页、叫醒管理器。
fn save_fence_config(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let config = state.shared.lock().unwrap().config.clone();
    config::save(&state.config_path, &config).map_err(|error| error.to_string())?;
    let _ = app.emit("config-changed", ());
    wake_fence_manager();
    Ok(())
}

fn fence_work_area(fence: &FenceSettings) -> Option<Rect> {
    let monitors = hexbench_core::sys::enumerate_monitors();
    if let Some(monitor) = monitors
        .iter()
        .find(|monitor| !fence.monitor.is_empty() && monitor.device == fence.monitor)
    {
        return Some(monitor.work);
    }
    // 记下的显示器不在了（拔掉副屏）：按上次的物理位置找最近的显示器。
    let center_x = fence.pixel_x + fence.pixel_w / 2;
    let center_y = fence.pixel_y + fence.pixel_h / 2;
    if let Some(monitor) = hexbench_core::sys::monitor_at(center_x, center_y) {
        return Some(monitor.work);
    }
    monitors.first().map(|monitor| monitor.work)
}

/// 展开状态下的物理外框。
fn fence_expanded_rect(fence: &FenceSettings) -> Rect {
    if fence.has_valid_geometry() {
        if let Some(work) = fence_work_area(fence) {
            if work.width() > 0 && work.height() > 0 {
                let width = (fence.w * work.width() as f64).round() as i32;
                let height = (fence.h * work.height() as f64).round() as i32;
                let left = work.left + (fence.x * work.width() as f64).round() as i32;
                let top = work.top + (fence.y * work.height() as f64).round() as i32;
                return Rect::new(left, top, left + width.max(1), top + height.max(1));
            }
        }
    }
    let width = fence.pixel_w.max(320);
    let height = fence.pixel_h.max(200);
    let (left, top) = hexbench_core::sys::clamp_to_work_area(
        fence.pixel_x,
        fence.pixel_y,
        width,
        height,
    );
    Rect::new(left, top, left + width, top + height)
}

// ---------- 格子 ----------
//
// 渲染层在 `hexbench-fence`：一条自己的消息循环线程 + 一份 D3D11/D2D/DComp
// 设备。这里只负责把数据整理成 `Frame`：目录枚举复用 `for_each_item`，
// 排序按设置，图标走 `hexbench_core::icons` 的同一份磁盘缓存。

/// 当前活着的原生宿主。只有管理器线程会建 / 拆；watcher 线程只读克隆。
static NATIVE_FENCES: Mutex<Option<Arc<hexbench_fence::NativeFences>>> = Mutex::new(None);
/// 每次推送内容都递增，用来丢弃晚到的图标批次。
static NATIVE_GEN: AtomicU64 = AtomicU64::new(1);
/// 原生格子的浏览历史：走过的目录（含绑定目录）+ 当前停在第几层。
/// 放这里而不是渲染层，拖动、排序、watcher 重推内容都不会把导航冲掉；
/// 右键手势的「后退 / 前进」按这份记录走，换绑目录时整段作废。
struct NativeNav {
    id: String,
    entries: Vec<String>,
    index: usize,
}
static NATIVE_NAV: Mutex<Vec<NativeNav>> = Mutex::new(Vec::new());

/// 「跟系统」时读注册表；AppsUseLightTheme = 0 表示深色。
fn system_uses_dark_theme() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|value| value == 0)
        .unwrap_or(false)
}

fn ui_uses_dark_theme(app: &AppHandle) -> bool {
    match app.state::<AppState>().shared.lock().unwrap().config.ui.theme {
        hexbench_core::config::Theme::Dark => true,
        hexbench_core::config::Theme::Light => false,
        hexbench_core::config::Theme::System => system_uses_dark_theme(),
    }
}

fn native_host(app: &AppHandle) -> Option<Arc<hexbench_fence::NativeFences>> {
    if let Ok(slot) = NATIVE_FENCES.lock() {
        if let Some(host) = slot.as_ref() {
            return Some(Arc::clone(host));
        }
    }
    let handle = app.clone();
    let sink: hexbench_fence::EventSink = Arc::new(move |event| match event {
        hexbench_fence::Event::Moved { id, rect } => {
            fence_persist_rect(
                &handle,
                &id,
                Rect::new(rect[0], rect[1], rect[0] + rect[2], rect[1] + rect[3]),
            );
        }
        hexbench_fence::Event::Rolled { id, rolled } => {
            native_edit_fence(&handle, &id, |fence| fence.rolled = rolled);
        }
        hexbench_fence::Event::Sort { id, key } => {
            native_edit_fence(&handle, &id, |fence| {
                // 和网页版一致：点同一列反转方向，换一列从升序开始。
                if fence.sort_key == key {
                    fence.sort_dir = -fence.sort_dir;
                } else {
                    fence.sort_key = key;
                    fence.sort_dir = 1;
                }
            });
        }
        hexbench_fence::Event::Lock { id, locked } => {
            native_edit_fence(&handle, &id, |fence| fence.locked = locked);
        }
        hexbench_fence::Event::View { id, view } => {
            let view = if view == hexbench_core::config::FENCE_VIEW_LIST {
                hexbench_core::config::FENCE_VIEW_LIST
            } else {
                hexbench_core::config::FENCE_VIEW_GRID
            };
            native_edit_fence(&handle, &id, |fence| fence.view = view.to_string());
        }
        hexbench_fence::Event::Rename { id, title } => {
            let title = title.trim().to_string();
            if !title.is_empty() {
                native_edit_fence(&handle, &id, |fence| fence.title = title);
            }
        }
        hexbench_fence::Event::RenameItem { id, path, name } => {
            // 和网页版的内联改名同一个入口：改名失败在格子里提示，不弹系统报错框。
            match launcher_rename(path, name, handle.clone()) {
                Ok(_) => native_refresh(&handle, &id),
                Err(error) => native_toast(&handle, &id, error),
            }
        }
        hexbench_fence::Event::Hide { id } => {
            native_edit_fence(&handle, &id, |fence| fence.visible = false);
        }
        hexbench_fence::Event::Delete { id } => {
            if let Ok(mut nav) = NATIVE_NAV.lock() {
                nav.retain(|state| state.id != id);
            }
            // 和网页版「删除格子」同一个入口；收纳盒里的文件按 keep 处理，不删。
            if let Err(error) = fence_delete(id.clone(), None, handle.clone()) {
                log::warn!("fence {id}: 原生菜单删除失败: {error}");
            }
        }
        hexbench_fence::Event::Refresh { id } => {
            // 右键菜单（在渲染线程上弹，返回时已经关掉）之后重读目录。
            native_refresh(&handle, &id);
        }
        hexbench_fence::Event::Navigate { id, path } => {
            // 双击子文件夹：停在格子里看，不弹资源管理器。
            native_navigate(&handle, &id, path);
        }
        hexbench_fence::Event::Back { id } => {
            // 右键手势往回划：退到历史里的上一层。
            native_history_step(&handle, &id, true);
        }
        hexbench_fence::Event::Forward { id } => {
            // 右键手势往前划：回到刚退出去的那一层。
            native_history_step(&handle, &id, false);
        }
        hexbench_fence::Event::BeginMove { id, hwnd, scale } => {
            // 原生格子自己发起：标题条拖动交给和网页共用的吸附 / 参考线循环。
            fence_run_move(handle.clone(), id, hwnd, scale as f64);
        }
        hexbench_fence::Event::BeginResize {
            id,
            hwnd,
            edge,
            scale,
        } => {
            if let Err(error) = fence_run_resize(handle.clone(), id.clone(), hwnd, &edge, scale as f64)
            {
                log::warn!("native fence: 缩放失败: {error}");
                native_toast(&handle, &id, error);
            }
        }
        hexbench_fence::Event::ChangeFolder { id } => {
            native_change_folder(&handle, &id);
        }
        hexbench_fence::Event::RunRules { id } => {
            let dto = run_rule_box(&handle, &id);
            log::info!(
                "native fence {id}: 立即整理 matched={} moved={}",
                dto.matched,
                dto.moved
            );
            let text = if dto.moved > 0 {
                format!("已收进 {} 项", dto.moved)
            } else {
                "没有可整理的项目".to_string()
            };
            native_toast(&handle, &id, text);
            native_refresh(&handle, &id);
        }
        hexbench_fence::Event::Drop {
            id,
            dest,
            paths,
            copy,
        } => {
            // 和网页格子拖进去是同一条路：撞名自动编号、记撤销、广播刷新。
            if dest.trim().is_empty() || paths.is_empty() {
                return;
            }
            // 格子里把文件拖到它自己所在的目录：系统仍会送一个 Drop，
            // 真搬的话就是「自己移进自己」，直接忽略。
            let dest_dir = Path::new(&dest)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&dest));
            if !copy
                && paths.iter().all(|path| {
                    Path::new(path)
                        .parent()
                        .map(|parent| {
                            parent
                                .canonicalize()
                                .unwrap_or_else(|_| parent.to_path_buf())
                                == dest_dir
                        })
                        .unwrap_or(false)
                })
            {
                return;
            }
            // 这里还在 OLE 的 Drop 回调里（渲染线程）：Shell 操作和整格重读
            // 都可能要几百毫秒，同步做会把所有格子卡住、也拖着资源管理器的
            // 拖拽不放。放到工作线程，回调立刻返回。
            let worker = handle.clone();
            let fence_id = id.clone();
            thread::spawn(move || {
                let result = if copy {
                    launcher_copy_into(dest, paths, worker.clone())
                } else {
                    launcher_move_into(dest, paths, worker.clone())
                };
                match result {
                    Ok(count) => {
                        log::info!("native fence {fence_id}: 拖入 {count} 项 copy={copy}");
                        native_toast(
                            &worker,
                            &fence_id,
                            match (count, copy) {
                                (0, true) => "没有复制".to_string(),
                                (0, false) => "没有移动".to_string(),
                                (count, true) => format!("已复制 {count} 项"),
                                (count, false) => format!("已移入 {count} 项"),
                            },
                        );
                    }
                    Err(error) => {
                        log::warn!("native fence {fence_id}: 拖入失败: {error}");
                        native_toast(&worker, &fence_id, error);
                    }
                }
                native_refresh(&worker, &fence_id);
            });
        }
    });
    match hexbench_fence::NativeFences::start(sink) {
        Ok(host) => {
            log::info!("fence: 原生渲染层已启动");
            if let Ok(mut slot) = NATIVE_FENCES.lock() {
                *slot = Some(Arc::clone(&host));
            }
            Some(host)
        }
        Err(error) => {
            log::warn!("fence: 原生渲染层启动失败，回退到 WebView：{error}");
            None
        }
    }
}

fn native_shutdown() {
    let host = NATIVE_FENCES
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    if let Some(host) = host {
        host.shutdown();
        log::info!("fence: 原生渲染层已关闭");
    }
}

/// 原生菜单 / 标题双击只改配置；`save_desktop` 会唤醒管理器线程重新落位，
/// 和设置页改同一份数据。
fn native_edit_fence(app: &AppHandle, id: &str, edit: impl FnOnce(&mut FenceSettings)) {
    let mut desktop = desktop_snapshot(app);
    let Some(fence) = desktop.fences.iter_mut().find(|fence| fence.id == id) else {
        return;
    };
    edit(fence);
    if let Err(error) = save_desktop(app, desktop) {
        log::warn!("fence {id}: 保存原生操作失败: {error}");
    }
}

/// 原生格子里的短提示（网页版 `showToast` 的对应物）：画在格子底部。
fn native_toast(_app: &AppHandle, id: &str, text: impl AsRef<str>) {
    let text = text.as_ref();
    if let Some(host) = NATIVE_FENCES.lock().ok().and_then(|slot| slot.clone()) {
        host.toast(id, text);
    }
    log::info!("native fence {id}: toast {text}");
}

/// 原生格子标题菜单「更换文件夹…」：弹系统目录选择，选完改绑定并重读。
fn native_change_folder(app: &AppHandle, id: &str) {
    use tauri_plugin_dialog::DialogExt;
    let handle = app.clone();
    let fence_id = id.to_string();
    handle
        .dialog()
        .file()
        .set_title("选择这个格子要显示的文件夹")
        .pick_folder(move |picked| {
            let Some(folder) = picked.and_then(|file| {
                file.as_path()
                    .map(|path| path.to_string_lossy().to_string())
            }) else {
                return;
            };
            let mut desktop = desktop_snapshot(&handle);
            let Some(fence) = desktop
                .fences
                .iter_mut()
                .find(|fence| fence.id == fence_id)
            else {
                return;
            };
            if fence.path.eq_ignore_ascii_case(&folder) {
                return;
            }
            fence.path = folder;
            fence.kind = FenceKind::Folder;
            // 换目录后旧目录的浏览历史没有意义了。
            if let Ok(mut nav) = NATIVE_NAV.lock() {
                nav.retain(|state| state.id != fence_id);
            }
            if let Err(error) = save_desktop(&handle, desktop) {
                log::warn!("native fence {fence_id}: 更换文件夹失败: {error}");
                native_toast(&handle, &fence_id, error);
                return;
            }
            native_refresh(&handle, &fence_id);
        });
}

/// 和网页版 `displayName` 一致：快捷方式不显示后缀。
fn fence_display_name(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower.ends_with(".lnk") || lower.ends_with(".url") {
        name[..name.len() - 4].to_string()
    } else {
        name.to_string()
    }
}

/// 资源管理器的排序：数字按数值比（`图2 < 图10`）。网页版用 Intl.Collator
/// 的 `numeric: true`，这里给一个等价的小实现。
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let si = i;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let na: String = a[si..i].iter().collect();
            let nb: String = b[sj..j].iter().collect();
            let ta = na.trim_start_matches('0');
            let tb = nb.trim_start_matches('0');
            let order = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let order = a[i].cmp(&b[j]);
            if order != Ordering::Equal {
                return order;
            }
            i += 1;
            j += 1;
        }
    }
    a.len().cmp(&b.len())
}

fn fence_extension(name: &str) -> String {
    match name.rfind('.') {
        Some(index) if index > 0 => name[index + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// 和 `src/fileview.ts` 的 `sortItems` 同一套规则（「使用频率」没有使用记录，
/// 按网页版的兜底走名字排序）。
fn sort_fence_items(items: &mut [LauncherItem], key: &str, direction: i32) {
    use std::cmp::Ordering;
    let dir = if direction < 0 { -1 } else { 1 };
    let flipped = |order: Ordering| if dir > 0 { order } else { order.reverse() };
    items.sort_by(|a, b| match key {
        "modified" => {
            let time = a.modified.cmp(&b.modified);
            if time == Ordering::Equal {
                natural_cmp(&a.name, &b.name)
            } else {
                flipped(time)
            }
        }
        _ => {
            if a.is_dir != b.is_dir {
                return if a.is_dir { Ordering::Less } else { Ordering::Greater };
            }
            let order = match key {
                "size" => a.size.cmp(&b.size),
                "type" => fence_extension(&a.name)
                    .cmp(&fence_extension(&b.name))
                    .then_with(|| natural_cmp(&a.name, &b.name)),
                _ => natural_cmp(&a.name, &b.name),
            };
            flipped(order)
        }
    });
}

/// 统一的路径比较形式：反斜杠、去掉末尾分隔符、不区分大小写。
fn native_norm_path(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// `path` 是不是 `root` 本身或它下面的一层（不允许借 `..` 跳出去）。
fn native_inside(path: &str, root: &str) -> bool {
    let child = native_norm_path(path);
    let parent = native_norm_path(root);
    if child.is_empty() || parent.is_empty() {
        return false;
    }
    child == parent || child.starts_with(&(parent + "\\"))
}

/// 这一格现在该读哪个目录：浏览历史停的那一层（仍然有效时），否则绑定目录。
fn native_nav_dir(fence: &FenceSettings) -> String {
    let root = fence.path.replace('/', "\\");
    if let Ok(mut nav) = NATIVE_NAV.lock() {
        if let Some(state) = nav.iter_mut().find(|state| state.id == fence.id) {
            if let Some(current) = state.entries.get(state.index).cloned() {
                if native_inside(&current, &root) && Path::new(&current).is_dir() {
                    return current;
                }
            }
            // 换绑了目录（或者旧目录被删）：整段历史作废，回到根。
            state.entries = vec![root.clone()];
            state.index = 0;
        }
    }
    root
}

/// 手势提示用：历史里还能往回 / 往前走的层数。
fn native_history_counts(fence: &FenceSettings) -> (usize, usize) {
    let root = fence.path.replace('/', "\\");
    if let Ok(nav) = NATIVE_NAV.lock() {
        if let Some(state) = nav.iter().find(|state| state.id == fence.id) {
            if state
                .entries
                .get(state.index)
                .map(|entry| native_inside(entry, &root))
                .unwrap_or(false)
            {
                return (state.index, state.entries.len().saturating_sub(state.index + 1));
            }
        }
    }
    (0, 0)
}

/// 从绑定目录到当前目录的每一层（不含根），和网页版的 crumbs 一致。
fn native_crumbs(root: &str, current: &str) -> Vec<hexbench_fence::Crumb> {
    let base = root.replace('/', "\\");
    let base = base.trim_end_matches('\\');
    let full = current.replace('/', "\\");
    let full = full.trim_end_matches('\\');
    if base.is_empty() || !native_inside(full, base) || native_norm_path(full) == native_norm_path(base)
    {
        return Vec::new();
    }
    let Some(rest) = full.get(base.len() + 1..) else {
        return Vec::new();
    };
    let mut acc = base.to_string();
    rest.split(['\\', '/'])
        .filter(|name| !name.is_empty())
        .map(|name| {
            acc.push('\\');
            acc.push_str(name);
            hexbench_fence::Crumb {
                name: name.to_string(),
                path: acc.clone(),
            }
        })
        .collect()
}

/// 让文件监听跟着格子当前看的目录走。根目录的监听是非递归的，进了子目录
/// 之后下面那一层的新建 / 删除得单独看，不然要等下次重开才会出现。
fn native_watch_dir(id: &str, dir: &str, root: &str) {
    let current = if native_norm_path(dir) == native_norm_path(root) {
        String::new()
    } else {
        dir.to_string()
    };
    if let Ok(mut slots) = FENCE_WATCH.lock() {
        let existing = slots
            .iter()
            .find(|(slot_id, _)| slot_id == id)
            .map(|(_, path)| path.clone());
        if existing.as_deref() != Some(current.as_str()) {
            slots.retain(|(slot_id, _)| slot_id != id);
            if !current.is_empty() {
                slots.push((id.to_string(), current));
            }
        }
    }
}

fn native_items(dir: &Path, sort_key: &str, sort_dir: i32) -> Vec<hexbench_fence::Item> {
    if dir.as_os_str().is_empty() {
        return Vec::new();
    }
    let mut raw = Vec::new();
    for_each_item(dir, false, |item| {
        raw.push(item);
        true
    });
    sort_fence_items(&mut raw, sort_key, sort_dir);
    raw.into_iter()
        .map(|item| hexbench_fence::Item {
            name: fence_display_name(&item.name),
            // Shell 的 SHParseDisplayName 只认反斜杠；老配置里可能存的是
            // `D:/xxx`，混着的路径会让右键菜单直接 E_INVALIDARG。
            path: item.path.replace('/', "\\"),
            is_dir: item.is_dir,
            modified: item.modified,
            size: if item.is_dir { 0 } else { item.size },
        })
        .collect()
}

/// 双击子文件夹：记下导航位置并立刻重读这一格。
fn native_navigate(app: &AppHandle, id: &str, path: String) {
    let desktop = desktop_snapshot(app);
    let Some(fence) = desktop.fences.iter().find(|fence| fence.id == id) else {
        return;
    };
    let root = fence.path.replace('/', "\\");
    let target = path.replace('/', "\\");
    if !native_inside(&target, &root) || !Path::new(&target).is_dir() {
        return;
    }
    if let Ok(mut nav) = NATIVE_NAV.lock() {
        if !nav.iter().any(|state| state.id == id) {
            nav.push(NativeNav {
                id: id.to_string(),
                entries: vec![root.clone()],
                index: 0,
            });
        }
        if let Some(state) = nav.iter_mut().find(|state| state.id == id) {
            if state.entries.is_empty() {
                state.entries.push(root.clone());
                state.index = 0;
            }
            let same = state
                .entries
                .get(state.index)
                .map(|entry| native_norm_path(entry) == native_norm_path(&target))
                .unwrap_or(false);
            if !same {
                // 从中间退回去之后又进新目录：后面的「前进」记录作废（和浏览器一致）。
                state.entries.truncate(state.index + 1);
                state.entries.push(target);
                // 最多记 50 层，太老的从头上丢掉。
                if state.entries.len() > 50 {
                    let drop = state.entries.len() - 50;
                    state.entries.drain(0..drop);
                }
                state.index = state.entries.len() - 1;
            }
        }
    }
    native_refresh(app, id);
}

/// 右键手势的「后退 / 前进」：在历史里挪一层再重读目录。
fn native_history_step(app: &AppHandle, id: &str, back: bool) {
    {
        let Ok(mut nav) = NATIVE_NAV.lock() else {
            return;
        };
        let Some(state) = nav.iter_mut().find(|state| state.id == id) else {
            return;
        };
        if back {
            if state.index == 0 {
                return;
            }
            state.index -= 1;
        } else {
            if state.index + 1 >= state.entries.len() {
                return;
            }
            state.index += 1;
        }
    }
    native_refresh(app, id);
}

fn native_frame(
    app: &AppHandle,
    desktop: &DesktopSettings,
    fence: &FenceSettings,
    generation: u64,
) -> hexbench_fence::Frame {
    let expanded = fence_expanded_rect(fence);
    let title = fence.title.trim();
    let root = fence.path.replace('/', "\\");
    let dir = native_nav_dir(fence);
    let (back_count, forward_count) = native_history_counts(fence);
    native_watch_dir(&fence.id, &dir, &root);
    let crumbs = native_crumbs(&root, &dir);
    hexbench_fence::Frame {
        id: fence.id.clone(),
        generation,
        title: if title.is_empty() {
            "格子".to_string()
        } else {
            title.to_string()
        },
        path: dir.clone(),
        root: root.clone(),
        crumbs,
        rect: [
            expanded.left,
            expanded.top,
            expanded.width(),
            expanded.height(),
        ],
        rolled: fence.rolled,
        zoom: fence.zoom,
        opacity: desktop.opacity,
        frosted: desktop.frosted,
        dark: ui_uses_dark_theme(app),
        items: native_items(Path::new(&dir), &fence.sort_key, fence.sort_dir),
        back_count,
        forward_count,
        is_bucket: fence.kind.is_bucket(),
        rule_count: fence.rules.len(),
        sort_key: fence.sort_key.clone(),
        sort_dir: fence.sort_dir,
        locked: fence.locked,
        list: fence.is_list(),
    }
}

/// 后台抽图标，按小批推给渲染线程。缓存命中的（网页版已经抽过）先到，
/// 没命中的才去问 Shell——两侧共用 `hexbench_core::icons` 的同一份缓存。
fn native_load_icons(
    host: Arc<hexbench_fence::NativeFences>,
    id: String,
    generation: u64,
    items: Vec<(String, bool)>,
) {
    thread::spawn(move || {
        let mut batch: Vec<(String, Option<PathBuf>)> = Vec::new();
        for (raw, is_dir) in items {
            let path = PathBuf::from(&raw);
            let mut key = hexbench_core::icons::cached_icon_key(&path).flatten();
            if key.is_none() {
                if let Some(_claim) = hexbench_core::icons::claim_extraction(&path) {
                    key = hexbench_core::icons::icon_key(&path, is_dir);
                } else {
                    // 别的线程正在抽同一张图：等它落盘，不重复问 Shell。
                    thread::sleep(Duration::from_millis(120));
                    key = hexbench_core::icons::cached_icon_key(&path).flatten();
                }
            }
            let file = key.as_deref().and_then(hexbench_core::icons::cache_file);
            batch.push((raw, file));
            if batch.len() >= 8 {
                host.set_icons(&id, generation, std::mem::take(&mut batch));
            }
        }
        if !batch.is_empty() {
            host.set_icons(&id, generation, batch);
        }
    });
}

fn native_sync(
    app: &AppHandle,
    desktop: &DesktopSettings,
    fences: &[FenceSettings],
    host: &Arc<hexbench_fence::NativeFences>,
) {
    let generation = NATIVE_GEN.fetch_add(1, Ordering::Relaxed);
    let frames: Vec<hexbench_fence::Frame> = fences
        .iter()
        .map(|fence| native_frame(app, desktop, fence, generation))
        .collect();
    let icon_jobs: Vec<(String, Vec<(String, bool)>)> = frames
        .iter()
        .map(|frame| {
            (
                frame.id.clone(),
                frame
                    .items
                    .iter()
                    .map(|item| (item.path.clone(), item.is_dir))
                    .collect(),
            )
        })
        .collect();
    host.sync(frames);
    for (id, icons) in icon_jobs {
        if !icons.is_empty() {
            native_load_icons(Arc::clone(host), id, generation, icons);
        }
    }
}

/// 文件变化 / 设置变化后只刷新一格。
fn native_refresh(app: &AppHandle, id: &str) {
    let Some(host) = NATIVE_FENCES.lock().ok().and_then(|slot| slot.clone()) else {
        return;
    };
    let desktop = desktop_snapshot(app);
    let Some(fence) = desktop.fences.iter().find(|fence| fence.id == id).cloned() else {
        return;
    };
    let generation = NATIVE_GEN.fetch_add(1, Ordering::Relaxed);
    let frame = native_frame(app, &desktop, &fence, generation);
    let icons: Vec<(String, bool)> = frame
        .items
        .iter()
        .map(|item| (item.path.clone(), item.is_dir))
        .collect();
    host.upsert(frame);
    if !icons.is_empty() {
        native_load_icons(host, id.to_string(), generation, icons);
    }
}

/// 让实际窗口和配置对上：开关关了 / 隐藏 / 删除的收起，其余的原样留着。
fn apply_fence_windows(app: &AppHandle) {
    let desktop = desktop_snapshot(app);
    let desired: Vec<FenceSettings> = if desktop.enabled {
        desktop
            .fences
            .iter()
            .filter(|fence| fence.visible)
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    // 原生宿主起不来（没有 D3D11 / 合成器）时桌面只能空着，日志里说清楚。
    let Some(host) = native_host(app) else {
        log::warn!("fence: 原生渲染不可用");
        return;
    };
    native_sync(app, &desktop, &desired, &host);
    // 建窗时用默认值，这里补上用户当前的手势设置。
    host.set_gestures(
        app.state::<AppState>()
            .shared
            .lock()
            .unwrap()
            .config
            .launcher
            .gestures
            .clone(),
    );
}

/// 管理器关心的变化：开关、增删、显隐、卷起、几何。内容（标题 / 路径 / 排序）
/// 由页面自己听 `config-changed` 刷新，不必重建窗口；原生层没有页面，
/// 所以把会影响画面的项也放进签名，变了就重新推一帧。
fn fence_signature(app: &AppHandle, desktop: &DesktopSettings) -> String {
    let mut signature = format!(
        "{}:{}:{}:{}",
        if desktop.enabled { '1' } else { '0' },
        desktop.opacity,
        desktop.frosted,
        if ui_uses_dark_theme(app) { 'D' } else { 'L' }
    );
    for fence in &desktop.fences {
        signature.push_str(&format!(
            "|{}:{}:{}:{}:{}:{:.5}:{:.5}:{:.5}:{:.5}:{}:{}:{}:{}:{}",
            fence.id,
            fence.visible,
            fence.rolled,
            fence.monitor,
            fence.title,
            fence.x,
            fence.y,
            fence.w,
            fence.h,
            fence.zoom,
            fence.sort_key,
            fence.sort_dir,
            if fence.locked { 'L' } else { '-' },
            fence.view
        ));
    }
    signature
}

/// Win+D 会把桌面窗口压到下面 / 最小化；Explorer 重启后 z-order 也会乱。
/// 每秒比一次，顺序正确时什么都不做。
/// 前台窗口的类名，日志里说明是谁把桌面抬起来 / 落回去的。
fn describe_foreground() -> String {
    let class = hexbench_core::sys::window_class_name(hexbench_core::sys::foreground_root());
    if class.is_empty() {
        "无".to_string()
    } else {
        class
    }
}

fn fence_watchdog(_app: &AppHandle, desktop: &DesktopSettings, show_desktop: &mut bool) {
    let hwnds: Vec<isize> = NATIVE_FENCES
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|host| host.hwnds()))
        .unwrap_or_default();

    // Win+D / 「显示桌面」不是把窗口最小化，而是资源管理器把桌面窗口
    // （24H2 之后的 `Progman`，更早的 `WorkerW`）抬到所有窗口上面并拿走
    // 前台——格子被压在它下面。判据和参考实现 PecoFence 一致：格子确实掉到
    // 桌面宿主下面，并且前台是 shell 自己的窗口（桌面、任务栏、桌面右键菜单）。
    // 只看类名不够，别的程序也能注册 WorkerW，所以比的是 Explorer 的进程。
    // 抬桌面和换前台之间还有极短的空档，这期间宿主已经在所有普通窗口前面了，
    // 也算「显示桌面」——漏掉这一拍，格子会被拉回桌面上面，之后再也认不出来。
    // 「保持可见」照常把格子插回桌面上方；关掉这一项就让它待在后面——被盖住
    // 即「跟着桌面一起藏起来」。别的程序回到前台后桌面落回底层，格子自然又
    // 露出来，不需要单独记一个「结束」时刻。
    let covering = hwnds.iter().copied().any(hexbench_core::sys::behind_desktop)
        && (hexbench_core::sys::foreground_is_shell()
            || hexbench_core::sys::desktop_raised_over_windows());
    if covering != *show_desktop {
        *show_desktop = covering;
        if covering {
            log::info!(
                "fence: 桌面盖住格子（{}，前台 {}）",
                if desktop.keep_on_show_desktop {
                    "保持可见"
                } else {
                    "跟随桌面隐藏"
                },
                describe_foreground()
            );
        } else {
            log::info!("fence: 桌面不再盖住格子（前台 {}）", describe_foreground());
        }
    }
    if covering && !desktop.keep_on_show_desktop {
        // 桌面还盖在格子上：什么都不做。下面那套重新锚定会把格子拉回桌面上方，
        // 那正是不该发生的事。
        return;
    }

    for (index, hwnd) in hwnds.iter().copied().enumerate() {
        if hexbench_core::sys::minimized(hwnd) || !hexbench_core::sys::window_visible(hwnd) {
            if desktop.keep_on_show_desktop {
                hexbench_core::sys::restore_window(hwnd);
            }
            continue;
        }
        // 两种不对的 z-order 都要拉回来：被桌面盖住（Win+D），以及反过来
        // 浮在普通窗口上面（置顶，或者和桌面之间夹了别的窗口）。
        let siblings: Vec<isize> = hwnds
            .iter()
            .copied()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, other)| other)
            .collect();
        if hexbench_core::sys::behind_desktop(hwnd)
            || hexbench_core::sys::floats_above_windows(hwnd, &siblings)
        {
            hexbench_core::sys::anchor_above_desktop(hwnd);
        }
    }
}

/// 显示器拓扑 / 分辨率 / 任务栏高度：变了就按归一化坐标重新落位。
fn monitor_signature() -> String {
    let mut monitors = hexbench_core::sys::enumerate_monitors();
    monitors.sort_by(|a, b| a.device.cmp(&b.device));
    monitors
        .iter()
        .map(|monitor| {
            format!(
                "{}:{},{},{}x{}",
                monitor.device,
                monitor.work.left,
                monitor.work.top,
                monitor.work.width(),
                monitor.work.height()
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn spawn_fence_manager(app: AppHandle) {
    thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        if let Ok(mut slot) = FENCE_WAKE.lock() {
            *slot = Some(tx);
        }
        // 启动时别和主窗口抢：格子晚半秒出来没人注意，主界面卡一下很明显。
        thread::sleep(Duration::from_millis(900));
        apply_fence_windows(&app);
        let mut signature = fence_signature(&app, &desktop_snapshot(&app));
        let mut monitors = monitor_signature();
        // Win+D 是否还按着：跨轮询保持，不然每一秒都会「重新发现」一次。
        let mut show_desktop = false;
        loop {
            let _ = rx.recv_timeout(Duration::from_millis(1000));
            let desktop = desktop_snapshot(&app);
            let next = fence_signature(&app, &desktop);
            let next_monitors = monitor_signature();
            if next != signature || next_monitors != monitors {
                signature = next;
                monitors = next_monitors;
                apply_fence_windows(&app);
            }
            if desktop.enabled {
                fence_watchdog(&app, &desktop, &mut show_desktop);
            }
        }
    });
}

// ---------------------------------------------------------------- 吸附参考线（跨屏虚线）

/// 每个格子都是独立窗口，自己画不出边界以外的线。要像设计工具那样让虚线横跨整块
/// 桌面、连到被对齐的那一格，就得有一块「谁都能画」的画布：一块盖住虚拟屏幕的透明
/// 窗口，点击穿透、置顶、不进任务栏。启动时先建好藏起来（拖动期间显示、松手藏回去）——
/// 运行中再建窗口只会得到「有外壳、没 webview」的空窗，页面根本不会加载。
const GUIDES_LABEL: &str = "snap-guides";

struct GuidesState {
    window: Option<tauri::WebviewWindow>,
    visible: bool,
    /// 建窗口失败过：同一次拖动里别再每帧都试，等下一次拖动再给一次机会。
    failed: bool,
    origin: (i32, i32),
    scale: f64,
    /// 上一次发出去的是哪组线。
    last: Option<[Option<hexbench_core::snap::Guide>; 2]>,
    last_emit: Instant,
}

fn guides_state() -> &'static Mutex<GuidesState> {
    static GUIDES: std::sync::OnceLock<Mutex<GuidesState>> = std::sync::OnceLock::new();
    GUIDES.get_or_init(|| {
        Mutex::new(GuidesState {
            window: None,
            visible: false,
            failed: false,
            origin: (0, 0),
            scale: 1.0,
            last: None,
            last_emit: Instant::now(),
        })
    })
}

/// 虚拟屏幕的物理范围（副屏在主屏左边 / 上边时坐标是负的）。
fn guides_bounds(app: &AppHandle) -> Option<(i32, i32, u32, u32)> {
    let state = app.state::<AppState>();
    let shared = state.shared.lock().unwrap();
    let monitors = &shared.monitors;
    let left = monitors.iter().map(|m| m.x).min()?;
    let top = monitors.iter().map(|m| m.y).min()?;
    let right = monitors.iter().map(|m| m.x + m.w).max()?;
    let bottom = monitors.iter().map(|m| m.y + m.h).max()?;
    Some((
        left,
        top,
        (right - left).max(1) as u32,
        (bottom - top).max(1) as u32,
    ))
}

/// 建画布窗口本身（不显示）。只在启动阶段调用。
fn build_guides_window(
    app: &AppHandle,
    bounds: (i32, i32, u32, u32),
) -> Option<tauri::WebviewWindow> {
    let (x, y, width, height) = bounds;
    let window = match WebviewWindowBuilder::new(
        app,
        GUIDES_LABEL,
        WebviewUrl::App("snap-guides.html".into()),
    )
    .title("HexBench")
    .additional_browser_args(WEBVIEW_ARGS)
    .decorations(false)
    .zoom_hotkeys_enabled(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .focused(false)
    .visible(false)
    .build()
    {
        Ok(window) => window,
        Err(error) => {
            log::warn!("snap-guides: build failed: {error}");
            return None;
        }
    };
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.set_size(PhysicalSize::new(width, height));
    let _ = window.set_ignore_cursor_events(true);
    // 拖动过程中弹出来的置顶窗口绝不能抢焦点，否则鼠标还按着、拖拽就被它打断了。
    if let Ok(hwnd) = window.hwnd() {
        hexbench_core::sys::set_no_activate(hwnd.0 as isize, true);
    }
    log::info!("snap-guides: canvas pre-built at {x},{y} {width}x{height}");
    Some(window)
}

/// 启动时把画布建好藏着。运行中新建的窗口没有 webview，页面加载不了。
fn prebuild_guides(app: &AppHandle) {
    let Some(bounds) = guides_bounds(app) else {
        return;
    };
    let mut state = guides_state().lock().unwrap();
    if state.window.is_some() || app.get_webview_window(GUIDES_LABEL).is_some() {
        return;
    }
    if let Some(window) = build_guides_window(app, bounds) {
        let _ = window.hide();
        state.window = Some(window);
        state.visible = false;
    } else {
        state.failed = true;
    }
}

fn guides_window(app: &AppHandle, state: &mut GuidesState) -> Option<tauri::WebviewWindow> {
    let (x, y, width, height) = guides_bounds(app)?;
    let window = match state
        .window
        .clone()
        .or_else(|| app.get_webview_window(GUIDES_LABEL))
    {
        Some(window) => window,
        None => match build_guides_window(app, (x, y, width, height)) {
            Some(window) => window,
            None => {
                state.failed = true;
                return None;
            }
        },
    };
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.set_size(PhysicalSize::new(width, height));
    let _ = window.set_ignore_cursor_events(true);
    if let Ok(hwnd) = window.hwnd() {
        hexbench_core::sys::set_no_activate(hwnd.0 as isize, true);
    }
    state.window = Some(window.clone());
    state.origin = (x, y);
    state.scale = window.scale_factor().unwrap_or(1.0);
    if !state.visible {
        if let Err(error) = window.show() {
            log::warn!("snap-guides: show failed: {error}");
            state.failed = true;
            return None;
        }
        state.visible = true;
        log::info!("snap-guides: shown at {x},{y} {width}x{height}");
    }
    Some(window)
}

/// 换算成那块画布里的 CSS 坐标（画布原点是虚拟屏幕左上角，按窗口缩放除一次）。
fn guides_payload(
    state: &GuidesState,
    guides: [Option<hexbench_core::snap::Guide>; 2],
) -> serde_json::Value {
    let (origin_x, origin_y) = state.origin;
    let scale = if state.scale > 0.0 { state.scale } else { 1.0 };
    let css = |value: i32| value as f64 / scale;
    let vertical = guides[0].map(|guide| {
        serde_json::json!({
            "x": css(guide.at - origin_x),
            "from": css(guide.from - origin_y),
            "to": css(guide.to - origin_y),
        })
    });
    let horizontal = guides[1].map(|guide| {
        serde_json::json!({
            "y": css(guide.at - origin_y),
            "from": css(guide.from - origin_x),
            "to": css(guide.to - origin_x),
        })
    });
    serde_json::json!({ "v": vertical, "h": horizontal })
}

/// 拖动 / 缩放过程中调用：有参考线就把线交给那层透明窗口去画。
fn guides_update(app: &AppHandle, guides: [Option<hexbench_core::snap::Guide>; 2]) {
    let mut state = guides_state().lock().unwrap();
    if state.failed {
        return;
    }
    let empty = guides.iter().all(Option::is_none);
    // 没有线、画布也收着：没什么要画的。
    if empty && !state.visible {
        state.last = Some(guides);
        return;
    }
    // 变了立刻发；没变每 250ms 也补发一次。画布是第一次吸附时才建窗的，
    // 页面挂载前发出的那次事件会丢，靠这个心跳把当前这根线补过去。
    let changed = state.last != Some(guides);
    let due = state.last_emit.elapsed() >= Duration::from_millis(250);
    if !changed && !due {
        return;
    }
    let window = if state.visible {
        state.window.clone()
    } else {
        guides_window(app, &mut state)
    };
    let Some(window) = window else {
        return;
    };
    // 透明置顶窗口可能被别的东西藏起来（切桌面、全屏程序），每次命中都核一下真状态。
    if let Ok(hwnd) = window.hwnd() {
        if !hexbench_core::sys::is_window_visible(hwnd.0 as isize) {
            let _ = window.show();
            log::info!("snap-guides: was hidden, shown again");
        }
    }
    // 和格子一样走 emit_to(label)：JS 那边 listen 的 target 是字符串，对上的就是
    // AnyLabel 这一路，窗口自己的 emit 到不了。
    let _ = window.emit_to(GUIDES_LABEL, "snap-guides", guides_payload(&state, guides));
    state.last = Some(guides);
    state.last_emit = Instant::now();
}

/// 拖动 / 缩放结束：清线、收起画布；下一次拖动再显示。
fn guides_clear() {
    let mut state = guides_state().lock().unwrap();
    state.failed = false;
    if let Some(window) = state.window.clone() {
        let _ = window.emit_to(GUIDES_LABEL, "snap-guides", serde_json::json!({ "v": null, "h": null }));
        if state.visible {
            let _ = window.hide();
        }
    }
    state.visible = false;
    // 下一轮拖动从「没发过线」开始，第一帧一定发得出去。
    state.last = None;
}

struct FenceSnap {
    cfg: hexbench_core::snap::SnapConfig,
    work: Rect,
    others: Vec<Rect>,
}

fn fence_snap_setup(app: &AppHandle, id: &str, start: Rect, scale: f64) -> FenceSnap {
    let desktop = desktop_snapshot(app);
    // 其他格子现在在哪：参考线要能跨过所有格子。
    let native: std::collections::HashMap<String, isize> = NATIVE_FENCES
        .lock()
        .ok()
        .and_then(|slot| {
            slot.as_ref()
                .map(|host| host.hwnd_map().into_iter().collect())
        })
        .unwrap_or_default();
    let mut others = Vec::new();
    for fence in desktop
        .fences
        .iter()
        .filter(|fence| fence.id != id && fence.visible)
    {
        let rect = native
            .get(&fence.id)
            .and_then(|hwnd| hexbench_core::sys::window_rect(*hwnd));
        if let Some(rect) = rect {
            others.push(rect);
        }
    }
    let center_x = (start.left + start.right) / 2;
    let center_y = (start.top + start.bottom) / 2;
    let work = hexbench_core::sys::monitor_at(center_x, center_y)
        .map(|monitor| monitor.work)
        .unwrap_or(start);
    FenceSnap {
        cfg: hexbench_core::snap::SnapConfig {
            enabled: desktop.snap,
            // 阈值按 DIP 定，换算成窗口所在显示器的物理像素。
            distance: (8.0 * scale).round() as i32,
            grid: if desktop.snap_grid {
                (8.0 * scale).round() as i32
            } else {
                0
            },
        },
        work,
        others,
    }
}

/// 松手后把物理矩形写回配置（归一化 + 像素两份）。卷起时不动 `h`。
fn fence_persist_geometry(app: &AppHandle, id: &str, hwnd: isize) {
    let Some(rect) = hexbench_core::sys::window_rect(hwnd) else {
        return;
    };
    fence_persist_rect(app, id, rect);
}

/// 两种窗口共用：把一个物理矩形写回归一化坐标 + 落盘。
fn fence_persist_rect(app: &AppHandle, id: &str, rect: Rect) {
    let center_x = rect.left + rect.width() / 2;
    let center_y = rect.top + rect.height() / 2;
    let monitor = hexbench_core::sys::monitor_at(center_x, center_y);
    let state = app.state::<AppState>();
    let config = {
        let mut shared = state.shared.lock().unwrap();
        let Some(fence) = shared
            .config
            .desktop
            .fences
            .iter_mut()
            .find(|fence| fence.id == id)
        else {
            return;
        };
        fence.pixel_x = rect.left;
        fence.pixel_y = rect.top;
        fence.pixel_w = rect.width();
        if !fence.rolled {
            fence.pixel_h = rect.height();
        }
        if let Some(monitor) = monitor {
            fence.monitor = monitor.device;
            let work = monitor.work;
            if work.width() > 0 && work.height() > 0 {
                fence.x = ((rect.left - work.left) as f64 / work.width() as f64).clamp(0.0, 1.0);
                fence.y = ((rect.top - work.top) as f64 / work.height() as f64).clamp(0.0, 1.0);
                fence.w = (rect.width() as f64 / work.width() as f64).clamp(0.02, 1.0);
                if !fence.rolled {
                    fence.h = (rect.height() as f64 / work.height() as f64).clamp(0.02, 1.0);
                }
            }
        }
        shared.config.clone()
    };
    if let Err(error) = config::save(&state.config_path, &config) {
        log::warn!("fence {id}: geometry save failed: {error}");
    }
}

/// 新格子的 id。`now_millis` 撞上（同一毫秒建两个）就再补一个序号。
fn fence_next_id(desktop: &DesktopSettings) -> String {
    let mut id = format!("f{:x}", now_millis());
    while desktop.fences.iter().any(|fence| fence.id == id) {
        id = format!(
            "f{:x}{:x}",
            now_millis(),
            FENCE_SEQ.fetch_add(1, Ordering::Relaxed)
        );
    }
    id
}

/// 在光标所在显示器上找个位置摆一格：同屏已有的依次往后错开，别叠在一起。
fn fence_place(
    desktop: &mut DesktopSettings,
    id: String,
    title: String,
    path: String,
    kind: FenceKind,
) -> Result<FenceSettings, String> {
    if desktop.fences.len() >= FENCE_MAX {
        return Err(format!("最多放 {FENCE_MAX} 个格子"));
    }
    let (cursor_x, cursor_y) = hexbench_core::sys::cursor_position();
    let monitor = hexbench_core::sys::monitor_at(cursor_x, cursor_y)
        .or_else(|| hexbench_core::sys::enumerate_monitors().into_iter().next())
        .ok_or("找不到显示器")?;
    let work = monitor.work;
    if work.width() <= 0 || work.height() <= 0 {
        return Err("显示器工作区无效".to_string());
    }
    let slot = desktop
        .fences
        .iter()
        .filter(|fence| fence.monitor == monitor.device)
        .count() as i32;
    let width = (work.width() as f64 * 0.30).clamp(360.0, 800.0) as i32;
    let height = (work.height() as f64 * 0.36).clamp(260.0, 720.0) as i32;
    let offset = (36 * slot).min((work.width() - width).max(0));
    let left = work.left + ((work.width() - width) as f64 * 0.18) as i32 + offset;
    let top = work.top + ((work.height() - height) as f64 * 0.14) as i32 + offset / 2;
    let (left, top) = hexbench_core::sys::clamp_to_work_area(left, top, width, height);

    let fence = FenceSettings {
        id,
        title,
        path,
        kind,
        monitor: monitor.device.clone(),
        x: ((left - work.left) as f64 / work.width() as f64).clamp(0.0, 1.0),
        y: ((top - work.top) as f64 / work.height() as f64).clamp(0.0, 1.0),
        w: (width as f64 / work.width() as f64).clamp(0.02, 1.0),
        h: (height as f64 / work.height() as f64).clamp(0.02, 1.0),
        pixel_x: left,
        pixel_y: top,
        pixel_w: width,
        pixel_h: height,
        ..Default::default()
    };
    desktop.enabled = true;
    desktop.fences.push(fence.clone());
    Ok(fence)
}

fn save_desktop(app: &AppHandle, desktop: DesktopSettings) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        state.shared.lock().unwrap().config.desktop = desktop;
    }
    save_fence_config(app)
}

#[tauri::command]
fn fence_create(path: String, app: AppHandle) -> Result<FenceSettings, String> {
    let path = path.trim().replace('/', "\\");
    if path.is_empty() {
        return Err("先选一个文件夹".to_string());
    }
    let folder = PathBuf::from(&path);
    if !folder.is_dir() {
        return Err("文件夹不存在".to_string());
    }
    let mut desktop = desktop_snapshot(&app);
    let title = folder
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("格子")
        .to_string();
    let id = fence_next_id(&desktop);
    let fence = fence_place(&mut desktop, id, title, path.clone(), FenceKind::Folder)?;
    save_desktop(&app, desktop)?;
    log::info!("fence {}: created for {path}", fence.id);
    Ok(fence)
}

/// 新建一个收纳盒：幕后目录建在「文档\HexBench 收纳」下面，格子绑定它。
/// 目录名用标题 + id，标题之后改了也不会把目录搬来搬去。
#[tauri::command]
fn fence_create_box(title: String, app: AppHandle) -> Result<FenceSettings, String> {
    let mut desktop = desktop_snapshot(&app);
    if desktop.fences.len() >= FENCE_MAX {
        return Err(format!("最多放 {FENCE_MAX} 个格子"));
    }
    let title = {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            "收纳盒".to_string()
        } else {
            trimmed.chars().take(24).collect()
        }
    };
    let id = fence_next_id(&desktop);
    let store = fence_store_root()?
        .join(format!("{}-{}", sanitize_dir_name(&title), &id[1..]));
    std::fs::create_dir_all(&store).map_err(|error| format!("建收纳目录失败：{error}"))?;
    let fence = fence_place(
        &mut desktop,
        id,
        title,
        store.to_string_lossy().to_string(),
        FenceKind::Bucket,
    )?;
    save_desktop(&app, desktop)?;
    log::info!("fence {}: created bucket at {}", fence.id, store.display());
    Ok(fence)
}

/// 设置页里「来源：桌面 / 下载」这两个快捷按钮用的真实路径
/// （OneDrive 重定向过的机器上跟 `%USERPROFILE%` 拼出来的不一样）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct KnownFoldersDto {
    desktop: Option<String>,
    downloads: Option<String>,
    documents: Option<String>,
}

#[tauri::command]
fn fence_known_folders() -> KnownFoldersDto {
    let path = |dir: Option<PathBuf>| dir.map(|dir| dir.to_string_lossy().to_string());
    KnownFoldersDto {
        desktop: path(hexbench_core::sys::desktop_dir()),
        downloads: path(hexbench_core::sys::downloads_dir()),
        documents: path(hexbench_core::sys::documents_dir()),
    }
}

/// 收纳盒里还有多少东西：删除前给设置页看，好决定文件怎么处理。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BoxInfoDto {
    store: String,
    items: usize,
    bytes: u64,
}

/// `path` 是收纳盒的幕后目录；不是收纳盒就返回空（设置页据此不弹三选一）。
fn box_contents(store: &Path) -> (Vec<PathBuf>, u64) {
    let mut paths = Vec::new();
    let mut bytes = 0u64;
    if let Ok(entries) = std::fs::read_dir(store) {
        for entry in entries.flatten() {
            paths.push(entry.path());
            if let Ok(meta) = entry.metadata() {
                bytes += meta.len();
            }
        }
    }
    (paths, bytes)
}

#[tauri::command]
fn fence_box_info(id: String, app: AppHandle) -> Option<BoxInfoDto> {
    let fence = desktop_snapshot(&app)
        .fences
        .into_iter()
        .find(|fence| fence.id == id)?;
    if !fence.is_bucket() {
        return None;
    }
    let (paths, bytes) = box_contents(&PathBuf::from(&fence.path));
    Some(BoxInfoDto {
        store: fence.path,
        items: paths.len(),
        bytes,
    })
}

/// 删除格子。收纳盒里的文件怎么处理由设置页传 `mode`：
/// `release` 搬到桌面、`purge` 删到回收站、其余（`keep` / 空）留在目录里。
/// 绑文件夹的格子本来就不动文件，`mode` 无所谓。
#[tauri::command]
fn fence_delete(id: String, mode: Option<String>, app: AppHandle) -> Result<(), String> {
    let mut desktop = desktop_snapshot(&app);
    let Some(fence) = desktop.fences.iter().find(|fence| fence.id == id).cloned() else {
        return Ok(());
    };
    if fence.is_bucket() {
        let store = PathBuf::from(&fence.path);
        let (entries, _) = box_contents(&store);
        if !entries.is_empty() {
            match mode.as_deref() {
                Some("release") => {
                    let dest = hexbench_core::sys::desktop_dir()
                        .ok_or("找不到桌面目录")?;
                    hexbench_core::shell_ops::move_into(&entries, &dest)
                        .map_err(|error| format!("把文件移到桌面失败：{error}"))?;
                }
                Some("purge") => {
                    hexbench_core::shell_ops::trash(&entries)
                        .map_err(|error| format!("删除文件失败：{error}"))?;
                }
                _ => {}
            }
        }
        // 空了就顺手删掉；还有东西（keep）就留着。
        let _ = std::fs::remove_dir(&store);
    }
    desktop.fences.retain(|fence| fence.id != id);
    save_desktop(&app, desktop)?;
    log::info!("fence {id}: deleted (mode={:?})", mode);
    Ok(())
}

// ---------- 整理规则 ----------
//
// 规则盒 = 收纳盒 + 一组「什么文件、从哪儿收、收进哪个子目录」。匹配只看
// 源目录的第一层；先命中的规则决定去向。盒子自己和所有幕后目录永远跳过，
// 否则「收进去的文件」会再次被当成候选。

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RuleMatchDto {
    path: String,
    name: String,
    /// 命中的规则名，预览时告诉用户「按哪条收」。
    rule: String,
    /// 收进盒子的哪个子目录（空 = 盒子根）。
    subdir: String,
    size: u64,
}

#[derive(Serialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
struct RuleRunDto {
    matched: usize,
    moved: usize,
}

/// 隐藏 / 系统文件（`desktop.ini`、卷标这类）不参与整理。
fn is_hidden_entry(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
        meta.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        false
    }
}

fn rule_matches(rule: &FenceRule, path: &Path, meta: &std::fs::Metadata) -> bool {
    let is_dir = meta.is_dir();
    if !rule.exts.is_empty() {
        // 按扩展名挑的时候只认文件：目录没有扩展名，收进去也说不清。
        if is_dir {
            return false;
        }
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !rule.exts.iter().any(|wanted| wanted == &ext) {
            return false;
        }
    }
    let needle = rule.name_contains.trim().to_lowercase();
    if !needle.is_empty() {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !name.contains(&needle) {
            return false;
        }
    }
    let megabytes = meta.len() as f64 / (1024.0 * 1024.0);
    if rule.min_mb > 0 && megabytes < rule.min_mb as f64 {
        return false;
    }
    if rule.max_mb > 0 && megabytes > rule.max_mb as f64 {
        return false;
    }
    if rule.older_than_days > 0 {
        let Ok(modified) = meta.modified() else {
            return false;
        };
        let age = std::time::SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        if age < Duration::from_secs(rule.older_than_days as u64 * 86_400) {
            return false;
        }
    }
    true
}

fn collect_rule_matches(fence: &FenceSettings, stores: &[PathBuf]) -> Vec<RuleMatchDto> {
    let box_path = PathBuf::from(&fence.path);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut hits = Vec::new();
    for rule in fence.rules.iter().filter(|rule| rule.enabled) {
        for source in &rule.sources {
            let dir = PathBuf::from(source.trim());
            if !dir.is_dir() {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let key = path.to_string_lossy().to_lowercase();
                if seen.contains(&key) {
                    continue;
                }
                // 已经在盒子里的、或者落在任何一个幕后目录里的，都不算候选。
                if is_under_path(&path, &box_path)
                    || stores.iter().any(|store| is_under_path(&path, store))
                {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if is_hidden_entry(&meta) || !rule_matches(rule, &path, &meta) {
                    continue;
                }
                seen.insert(key);
                hits.push(RuleMatchDto {
                    path: path.to_string_lossy().to_string(),
                    name: path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    rule: rule.name.clone(),
                    subdir: rule.subdir.clone(),
                    size: meta.len(),
                });
                // 一次别收太多：真出错了（规则写宽了）也来得及停手。
                if hits.len() >= 5000 {
                    return hits;
                }
            }
        }
    }
    hits
}

fn rule_box_stores(app: &AppHandle) -> Vec<PathBuf> {
    desktop_snapshot(app)
        .fences
        .iter()
        .filter(|fence| fence.is_bucket() && !fence.path.trim().is_empty())
        .map(|fence| PathBuf::from(&fence.path))
        .collect()
}

/// 按规则整理一格。移动走 `launcher_move_into`：撞名自动编号、记撤销、
/// 广播刷新，和手动拖进去是同一条路。
fn run_rule_box(app: &AppHandle, id: &str) -> RuleRunDto {
    let Some(fence) = desktop_snapshot(app)
        .fences
        .into_iter()
        .find(|fence| fence.id == id && fence.is_bucket())
    else {
        return RuleRunDto::default();
    };
    let stores = rule_box_stores(app);
    let hits = collect_rule_matches(&fence, &stores);
    if hits.is_empty() {
        return RuleRunDto::default();
    }
    let matched = hits.len();
    let mut groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for hit in hits {
        groups.entry(hit.subdir).or_default().push(hit.path);
    }
    let mut moved = 0;
    for (subdir, paths) in groups {
        let mut dest = PathBuf::from(&fence.path);
        if !subdir.trim().is_empty() {
            dest.push(subdir.trim());
        }
        if std::fs::create_dir_all(&dest).is_err() {
            continue;
        }
        match launcher_move_into(dest.to_string_lossy().to_string(), paths, app.clone()) {
            Ok(count) => moved += count,
            Err(error) => log::warn!("fence {id}: rule move failed: {error}"),
        }
    }
    if moved > 0 {
        log::info!("fence {id}: rules collected {moved} item(s)");
    }
    RuleRunDto { matched, moved }
}

#[tauri::command]
fn fence_rule_preview(id: String, app: AppHandle) -> Vec<RuleMatchDto> {
    let Some(fence) = desktop_snapshot(&app)
        .fences
        .into_iter()
        .find(|fence| fence.id == id && fence.is_bucket())
    else {
        return Vec::new();
    };
    collect_rule_matches(&fence, &rule_box_stores(&app))
}

#[tauri::command]
fn fence_rule_run(id: String, app: AppHandle) -> RuleRunDto {
    run_rule_box(&app, &id)
}

/// 等源目录安静下来再整理：解压 / 批量复制会连发几百个事件，边收边发容易撞名。
static RULE_DIRTY: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
const RULE_DEBOUNCE: Duration = Duration::from_millis(1500);

/// watcher 看到某处变化：把「源目录命中这次变化」的规则盒排进队列。
fn mark_rules_dirty(app: &AppHandle, changed: &[PathBuf]) {
    let desktop = desktop_snapshot(app);
    if !desktop.enabled {
        return;
    }
    let now = Instant::now();
    let mut dirty = RULE_DIRTY.lock().unwrap();
    for fence in desktop
        .fences
        .iter()
        .filter(|fence| fence.is_bucket() && fence.rules_auto && !fence.rules.is_empty())
    {
        let hit = fence
            .rules
            .iter()
            .filter(|rule| rule.enabled)
            .flat_map(|rule| rule.sources.iter())
            .any(|source| {
                let source = PathBuf::from(source.trim());
                changed.iter().any(|path| is_under_path(path, &source))
            });
        if !hit {
            continue;
        }
        match dirty.iter_mut().find(|(slot, _)| *slot == fence.id) {
            // 还在变化就往后推，等到安静了再整理。
            Some((_, at)) => *at = now,
            None => dirty.push((fence.id.clone(), now)),
        }
    }
}

/// 规则盒的后台整理线程：启动时把现有的规则盒排一次（关掉应用期间新增的
/// 文件也要收），之后只按 watcher 的信号跑。
fn spawn_rule_engine(app: AppHandle) {
    thread::spawn(move || {
        {
            let desktop = desktop_snapshot(&app);
            if desktop.enabled {
                let now = Instant::now();
                let mut dirty = RULE_DIRTY.lock().unwrap();
                for fence in desktop
                    .fences
                    .iter()
                    .filter(|fence| fence.is_bucket() && fence.rules_auto && !fence.rules.is_empty())
                {
                    dirty.push((fence.id.clone(), now));
                }
            }
        }
        loop {
            thread::sleep(Duration::from_millis(600));
            let due: Vec<String> = {
                let mut dirty = RULE_DIRTY.lock().unwrap();
                let now = Instant::now();
                let ready = dirty
                    .iter()
                    .filter(|(_, at)| now.duration_since(*at) >= RULE_DEBOUNCE)
                    .map(|(id, _)| id.clone())
                    .collect();
                dirty.retain(|(_, at)| now.duration_since(*at) < RULE_DEBOUNCE);
                ready
            };
            for id in due {
                // 没开桌面格子 / 格子被删了就跳过（下次启动会重新排）。
                let desktop = desktop_snapshot(&app);
                let exists = desktop.enabled
                    && desktop
                        .fences
                        .iter()
                        .any(|fence| fence.id == id && fence.is_bucket());
                if exists {
                    let _ = run_rule_box(&app, &id);
                }
            }
        }
    });
}

#[tauri::command]
fn fence_set_enabled(enabled: bool, app: AppHandle) -> Result<(), String> {
    let mut desktop = desktop_snapshot(&app);
    desktop.enabled = enabled;
    {
        let state = app.state::<AppState>();
        state.shared.lock().unwrap().config.desktop = desktop;
    }
    save_fence_config(&app)
}

/// 设置页里的两个滑块 / 开关：写进配置后由管理器重推一帧，不等重启。
#[tauri::command]
fn fence_set_appearance(opacity: u32, frosted: bool, app: AppHandle) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let mut shared = state.shared.lock().unwrap();
        shared.config.desktop.opacity = opacity;
        shared.config.desktop.frosted = frosted;
        shared.config.desktop.clamp_opacity();
    }
    save_fence_config(&app)
}

/// 桌面右键菜单跟着 `desktop.contextMenu` 走：开着就写注册表（每次启动对一遍，
/// 应用升级 / 换目录之后菜单指向的仍是当前 exe），关掉就删掉自己那一项。
///
/// 界面上没有这个开关，`contextMenu: false` 只是配置里的逃生口。
fn apply_desktop_menu(enabled: bool) -> Result<(), String> {
    if !enabled {
        return hexbench_core::desktop_menu::uninstall();
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    hexbench_core::desktop_menu::install(&exe)
}

/// 右键菜单「新建格子…」：选一个文件夹，选中就放一个格子上去。
fn fence_pick_and_create(app: &AppHandle) {
    use tauri_plugin_dialog::DialogExt;
    // 冷启动时这个线程比事件循环还早：稍等一下再弹对话框。
    thread::sleep(Duration::from_millis(500));
    let handle = app.clone();
    handle
        .dialog()
        .file()
        .set_title("选择要放进桌面格子的文件夹")
        .pick_folder(move |picked| {
            let Some(folder) = picked.and_then(|file| {
                file.as_path()
                    .map(|path| path.to_string_lossy().to_string())
            }) else {
                return;
            };
            if let Err(error) = fence_create(folder, handle.clone()) {
                log::warn!("fence: create from menu failed: {error}");
            }
        });
}

/// 桌面右键菜单转过来的命令行参数；接住了返回 true。
///
/// 菜单项只带一个参数（见 `hexbench_core::desktop_menu`），真正的动作在
/// 已经在跑的实例里做：单实例插件会把参数转交给它。
fn menu_args_handled(app: &AppHandle, args: &[String]) -> bool {
    if args.iter().any(|arg| arg == "--fence-create-pick") {
        let handle = app.clone();
        thread::spawn(move || fence_pick_and_create(&handle));
        return true;
    }
    // 右键菜单「新建收纳盒」：没有文件夹要选，直接建一个幕后目录的盒子。
    if args.iter().any(|arg| arg == "--box-create") {
        let handle = app.clone();
        thread::spawn(move || {
            // 冷启动时事件循环还没跑起来，稍等一下再建窗口（同 fence_pick_and_create）。
            thread::sleep(Duration::from_millis(700));
            match fence_create_box(String::new(), handle) {
                Ok(fence) => log::info!("fence: menu created box {}", fence.id),
                Err(error) => log::warn!("fence: menu create box failed: {error}"),
            }
        });
        return true;
    }
    if args.iter().any(|arg| arg == "--fence-toggle") {
        let handle = app.clone();
        thread::spawn(move || {
            let enabled = !desktop_snapshot(&handle).enabled;
            match fence_set_enabled(enabled, handle) {
                Ok(()) => log::info!("fence: menu toggle -> enabled={enabled}"),
                Err(error) => log::warn!("fence: menu toggle failed: {error}"),
            }
        });
        return true;
    }
    if args.iter().any(|arg| arg == "--desktop-settings") {
        let handle = app.clone();
        thread::spawn(move || open_settings("desktop".to_string(), None, handle));
        return true;
    }
    false
}

#[tauri::command]
// 前端一次提交改动项，参数形状就是 invoke 的对象字段；拆成 struct 反而要改前端。
#[allow(clippy::too_many_arguments)]
fn fence_update(
    id: String,
    title: Option<String>,
    path: Option<String>,
    rolled: Option<bool>,
    visible: Option<bool>,
    zoom: Option<u32>,
    sort_key: Option<String>,
    sort_dir: Option<i32>,
    locked: Option<bool>,
    view: Option<String>,
    rules: Option<Vec<FenceRule>>,
    rules_auto: Option<bool>,
    app: AppHandle,
) -> Result<(), String> {
    let mut desktop = desktop_snapshot(&app);
    {
        let Some(fence) = desktop
            .fences
            .iter_mut()
            .find(|fence| fence.id == id)
        else {
            return Err("格子不在了".to_string());
        };
        if let Some(title) = title {
            fence.title = title.trim().to_string();
        }
        if let Some(path) = path {
            // 收纳盒的「文件夹」是应用自己建的幕后目录，不能换成别的。
            if fence.is_bucket() {
                return Err("收纳盒不能更换文件夹".to_string());
            }
            let path = path.trim().replace('/', "\\");
            if !path.is_empty() && !PathBuf::from(&path).is_dir() {
                return Err("文件夹不存在".to_string());
            }
            fence.path = path;
        }
        if let Some(mut rules) = rules {
            // 设置页新加的规则没有 id：这里补一个，之后增删按 id 配对。
            for rule in &mut rules {
                if rule.id.trim().is_empty() {
                    rule.id = format!(
                        "r{:x}{:x}",
                        now_millis(),
                        FENCE_SEQ.fetch_add(1, Ordering::Relaxed)
                    );
                }
            }
            fence.rules = rules;
            fence.clamp_rules();
        }
        if let Some(rules_auto) = rules_auto {
            fence.rules_auto = rules_auto;
        }
        if let Some(rolled) = rolled {
            fence.rolled = rolled;
        }
        if let Some(visible) = visible {
            fence.visible = visible;
        }
        if let Some(zoom) = zoom {
            fence.zoom = zoom;
        }
        if let Some(sort_key) = sort_key {
            fence.sort_key = sort_key;
        }
        if let Some(sort_dir) = sort_dir {
            fence.sort_dir = sort_dir;
        }
        if let Some(locked) = locked {
            fence.locked = locked;
        }
        if let Some(view) = view {
            fence.view = if view == hexbench_core::config::FENCE_VIEW_LIST {
                hexbench_core::config::FENCE_VIEW_LIST.to_string()
            } else {
                hexbench_core::config::FENCE_VIEW_GRID.to_string()
            };
        }
        fence.clamp_zoom();
    }
    {
        let state = app.state::<AppState>();
        state.shared.lock().unwrap().config.desktop = desktop;
    }
    save_fence_config(&app)?;
    Ok(())
}

/// 拖动循环：网页格子走命令、原生格子走 `Event::BeginMove`，共用这一份。
fn fence_run_move(app: AppHandle, id: String, hwnd: isize, scale: f64) {
    let Some(start) = hexbench_core::sys::window_rect(hwnd) else {
        return;
    };
    let origin = hexbench_core::sys::cursor_position();
    let snap = fence_snap_setup(&app, &id, start, scale);
    let handle = app.clone();
    let moved_id = id.clone();
    thread::spawn(move || {
        let mut last: Option<Rect> = None;
        loop {
            if !hexbench_core::sys::left_button_down() {
                break;
            }
            let (cursor_x, cursor_y) = hexbench_core::sys::cursor_position();
            let dx = cursor_x - origin.0;
            let dy = cursor_y - origin.1;
            let raw = Rect::new(
                start.left + dx,
                start.top + dy,
                start.right + dx,
                start.bottom + dy,
            );
            // 按住 Alt 临时关掉吸附（也关网格），和成熟实现的习惯一致。
            let cfg = if hexbench_core::sys::alt_down() {
                hexbench_core::snap::SnapConfig {
                    enabled: false,
                    grid: 0,
                    ..snap.cfg
                }
            } else {
                snap.cfg
            };
            let snapped = hexbench_core::snap::snap_move(raw, snap.work, &snap.others, cfg);
            // 拖到屏幕边上就顶住：拖动过程中不夹的话，窗口可以跟着光标跑出工作区，
            // 松手时再夹回来，用户看到的就是卡片「跳」一下。
            let (left, top) = hexbench_core::sys::clamp_to_work_area(
                snapped.rect.left,
                snapped.rect.top,
                snapped.rect.width(),
                snapped.rect.height(),
            );
            let rect = Rect::new(
                left,
                top,
                left + snapped.rect.width(),
                top + snapped.rect.height(),
            );
            if last != Some(rect) {
                hexbench_core::sys::set_window_rect(hwnd, rect);
                last = Some(rect);
            }
            // 每帧都交过去：画布是懒建的，页面挂载前的事件会丢，由 guides_update
            // 里的心跳去补；线消失时也要靠它把旧线擦掉。
            guides_update(&handle, snapped.guides);
            thread::sleep(Duration::from_millis(8));
        }
        guides_clear();
        // 松手后夹进显示器工作区：标题条不能被拖到屏幕外面找不回来。
        if let Some(rect) = hexbench_core::sys::window_rect(hwnd) {
            let (left, top) = hexbench_core::sys::clamp_to_work_area(
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
            );
            let clamped = Rect::new(left, top, left + rect.width(), top + rect.height());
            if clamped != rect {
                hexbench_core::sys::set_window_rect(hwnd, clamped);
            }
        }
        hexbench_core::sys::anchor_above_desktop(hwnd);
        fence_persist_geometry(&handle, &moved_id, hwnd);
    });
}

/// 缩放循环：同上，两种窗口共用。
fn fence_run_resize(
    app: AppHandle,
    id: String,
    hwnd: isize,
    edge: &str,
    scale: f64,
) -> Result<(), String> {
    let edge = hexbench_core::snap::Edge::parse(edge).ok_or("未知的缩放边")?;
    let start = hexbench_core::sys::window_rect(hwnd).ok_or("拿不到窗口位置")?;
    let origin = hexbench_core::sys::cursor_position();
    let rolled = desktop_snapshot(&app)
        .fences
        .iter()
        .find(|fence| fence.id == id)
        .map(|fence| fence.rolled)
        .unwrap_or(false);
    let snap = fence_snap_setup(&app, &id, start, scale);
    let min_width = (FENCE_MIN_W_LOGICAL * scale).round() as i32;
    let min_height = (FENCE_MIN_H_LOGICAL * scale).round() as i32;
    let handle = app.clone();
    let resized_id = id.clone();
    thread::spawn(move || {
        let mut last: Option<Rect> = None;
        loop {
            if !hexbench_core::sys::left_button_down() {
                break;
            }
            let (cursor_x, cursor_y) = hexbench_core::sys::cursor_position();
            let dx = cursor_x - origin.0;
            let dy = cursor_y - origin.1;
            let mut left = start.left;
            let mut top = start.top;
            let mut right = start.right;
            let mut bottom = start.bottom;
            if edge.moves_west() {
                left = (start.left + dx).min(start.right - min_width);
            }
            if edge.moves_east() {
                right = (start.right + dx).max(start.left + min_width);
            }
            // 卷起状态固定高度，只允许改宽（PecoFence 的同一规则）。
            if !rolled {
                if edge.moves_north() {
                    top = (start.top + dy).min(start.bottom - min_height);
                }
                if edge.moves_south() {
                    bottom = (start.bottom + dy).max(start.top + min_height);
                }
            }
            let raw = Rect::new(left, top, right, bottom);
            let cfg = if hexbench_core::sys::alt_down() {
                hexbench_core::snap::SnapConfig {
                    enabled: false,
                    grid: 0,
                    ..snap.cfg
                }
            } else {
                snap.cfg
            };
            let snapped = hexbench_core::snap::snap_resize(
                raw,
                edge,
                snap.work,
                &snap.others,
                cfg,
                min_width,
                if rolled { raw.height() } else { min_height },
            );
            let mut rect = snapped.rect;
            if rolled {
                rect.top = start.top;
                rect.bottom = start.bottom;
            }
            if last != Some(rect) {
                hexbench_core::sys::set_window_rect(hwnd, rect);
                last = Some(rect);
            }
            // 每帧都交过去，靠 guides_update 的心跳补上早丢的事件。
            guides_update(&handle, snapped.guides);
            thread::sleep(Duration::from_millis(8));
        }
        guides_clear();
        hexbench_core::sys::anchor_above_desktop(hwnd);
        fence_persist_geometry(&handle, &resized_id, hwnd);
    });
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let paused = {
        let state = app.state::<AppState>();
        let shared = state.shared.lock().unwrap();
        shared.paused
    };
    let open = MenuItem::with_id(app, "open", "打开主界面", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(
        app,
        "pause",
        "暂停边缘切换",
        true,
        paused,
        None::<&str>,
    )?;
    let show_all = MenuItem::with_id(app, "show-all", "显示全部格子", true, None::<&str>)?;
    let separator_a = PredefinedMenuItem::separator(app)?;
    let separator_b = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &separator_a,
            &pause,
            &show_all,
            &separator_b,
            &quit,
        ],
    )?;
    *app.state::<AppState>().tray_menu.lock().unwrap() = Some(menu.clone());

    let icon = match app.default_window_icon() {
        Some(icon) => icon.clone(),
        None => tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?,
    };

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("HexBench")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => open_main(app),
            "quit" => request_quit(app),
            "pause" => toggle_pause(app),
            "show-all" => show_all_fences(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_main(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// 托盘勾选跟着状态走：热键和设置页也能改暂停开关。
fn sync_tray_checks(app: &AppHandle) {
    let menu = app.state::<AppState>().tray_menu.lock().unwrap().clone();
    let Some(menu) = menu else {
        return;
    };
    let paused = {
        let state = app.state::<AppState>();
        let shared = state.shared.lock().unwrap();
        shared.paused
    };
    if let Some(MenuItemKind::Check(item)) = menu.get("pause") {
        let _ = item.set_checked(paused);
    }
}

fn spawn_engine(shared: Arc<Mutex<Shared>>) {
    thread::spawn(move || {
        let initial = shared.lock().unwrap().config.clone();
        let mut engine = Engine::new(initial, false);
        let mut tick_count: u64 = 0;
        let mut last_guard: Option<String> = None;

        loop {
            tick_count += 1;
            let (desired, paused, refresh_requested, poll_ms) = {
                let mut state = shared.lock().unwrap();
                let refresh = std::mem::take(&mut state.refresh_requested);
                (
                    state.config.clone(),
                    state.paused,
                    refresh,
                    state.config.trigger.poll_ms,
                )
            };

            if refresh_requested || engine.config() != &desired {
                engine.set_config(desired.clone());
            }

            let tick = if paused {
                engine.reset();
                None
            } else {
                Some(engine.tick())
            };

            {
                let mut state = shared.lock().unwrap();
                match tick {
                    Some(tick) => {
                        state.cursor = tick.cursor;
                        state.hit = tick.hit;
                        state.guard = tick.guard.map(str::to_string);
                    }
                    None => {
                        state.cursor = hexbench_core::sys::cursor_pos();
                        state.hit = None;
                        state.guard = Some("paused".to_string());
                    }
                }

                state.engine_phase = engine.phase().to_string();
                if let Some(tick) = &tick {
                    if let Some(fire) = tick.fire {
                        log::info!(
                            "fire {:?} at {}ms (desktops={:?})",
                            fire.hit.side,
                            fire.at_ms,
                            state.desktop_count
                        );
                    }
                }
                if tick.is_some() {
                    let guard_now = state.guard.clone();
                    if guard_now != last_guard {
                        log::info!("guard {:?} -> {:?}", last_guard, guard_now);
                        if guard_now.as_deref() == Some("fullscreen")
                            || guard_now.as_deref() == Some("fullscreen-rect")
                        {
                            if let Some(info) = hexbench_core::sys::foreground_window_info() {
                                log::info!(
                                    "  fullscreen window: {} | {} | {}",
                                    info.process,
                                    info.class,
                                    info.title
                                );
                            }
                        }
                        last_guard = guard_now;
                    }
                }
                state.monitors = engine
                    .monitors()
                    .iter()
                    .enumerate()
                    .map(|(index, monitor)| monitor_dto(index, monitor))
                    .collect();
                state.regions = engine.regions().iter().map(region_dto).collect();

                let stats = engine.stats();
                state.stats = StatsDto {
                    ticks: stats.ticks,
                    fires: stats.fires,
                    failures: stats.send_failures,
                };

                // Explorer updates the registry when desktops are created or
                // removed; twice a second is plenty and avoids doing it per tick.
                if tick_count % 40 == 1 {
                    state.desktop_count = hexbench_core::sys::virtual_desktop_count();
                }
            }

            thread::sleep(Duration::from_millis(poll_ms.max(10)));
        }
    });
}

fn monitor_dto(index: usize, monitor: &Monitor) -> MonitorDto {
    MonitorDto {
        index,
        x: monitor.rect.left,
        y: monitor.rect.top,
        w: monitor.rect.width(),
        h: monitor.rect.height(),
        work_x: monitor.work.left,
        work_y: monitor.work.top,
        work_w: monitor.work.width(),
        work_h: monitor.work.height(),
        primary: monitor.primary,
        device: monitor.device.clone(),
    }
}

fn region_dto(region: &Region) -> RegionDto {
    RegionDto {
        monitor: region.monitor,
        side: region.side,
        edge_x: region.edge_x,
        y0: region.y0,
        y1: region.y1,
    }
}

/// 文件区自检（仅 debug）：把面板真正调用的那组命令跑一遍，结果按行写进
/// `hexbench-selftest-out.txt`。用 `%TEMP%` 下的沙箱，不碰配置里的目录。
#[cfg(debug_assertions)]
fn selftest_launcher_files(app: &AppHandle) -> String {
    let mut out = String::new();
    let mut pass = 0usize;
    let mut fail = 0usize;
    macro_rules! check {
        ($name:expr, $ok:expr, $($detail:tt)*) => {{
            let ok = $ok;
            if ok { pass += 1; } else { fail += 1; }
            out += &format!(
                "files: {} {} — {}\n",
                if ok { "PASS" } else { "FAIL" },
                $name,
                format!($($detail)*)
            );
        }};
    }

    let raw = std::env::temp_dir().join("hexbench-fileset");
    let _ = std::fs::remove_dir_all(&raw);
    if let Err(error) = std::fs::create_dir_all(&raw) {
        return format!("files: FAIL setup — {error}\n");
    }
    // `%TEMP%` 可能是 8.3 短路径，回收站里记的是长路径，统一成长路径。
    let base = std::fs::canonicalize(&raw).unwrap_or(raw);
    let base = match base.to_string_lossy().strip_prefix(r"\\?\") {
        Some(rest) => std::path::PathBuf::from(rest),
        None => base,
    };
    let sub = base.join("子目录");
    let cut_dest = base.join("剪切目标");
    let _ = std::fs::create_dir_all(&sub);
    let a = base.join("a.txt");
    let b = base.join("b.txt");
    let _ = std::fs::write(&a, "aaa");
    let _ = std::fs::write(&b, "bbb");
    let sub_b = sub.join("b.txt");
    let text = |path: &std::path::Path| std::fs::read_to_string(path).unwrap_or_default();
    let base_s = base.to_string_lossy().to_string();
    let sub_s = sub.to_string_lossy().to_string();
    let cut_s = cut_dest.to_string_lossy().to_string();

    let items = tauri::async_runtime::block_on(launcher_items(base_s.clone(), Some(false)));
    let names: Vec<String> = items.iter().map(|item| item.name.clone()).collect();
    check!(
        "listing",
        items.len() == 3 && items.first().is_some_and(|item| item.is_dir),
        "{names:?}（文件夹应排在前面）"
    );

    let made = launcher_new_folder(base_s.clone());
    check!(
        "new-folder",
        made.as_ref()
            .is_ok_and(|path| std::path::Path::new(path).is_dir()),
        "{made:?}"
    );
    let _ = launcher_undo(app.clone());
    check!(
        "undo-new-folder",
        made.as_ref()
            .is_ok_and(|path| !std::path::Path::new(path).exists()),
        "撤销后刚建的文件夹应消失"
    );

    let renamed = launcher_rename(a.to_string_lossy().to_string(), "改名.txt".into(), app.clone());
    let renamed_ok = renamed
        .as_ref()
        .map(|path| text(std::path::Path::new(path)) == "aaa")
        .unwrap_or(false);
    check!("rename", renamed_ok, "{renamed:?}");
    let _ = launcher_undo(app.clone());
    check!("undo-rename", a.exists() && !base.join("改名.txt").exists(), "应改回 a.txt");

    let moved = launcher_move_into(sub_s.clone(), vec![b.to_string_lossy().to_string()], app.clone());
    check!(
        "move",
        moved.as_ref().is_ok_and(|count| *count == 1) && sub_b.exists() && !b.exists(),
        "{moved:?}"
    );
    let _ = launcher_undo(app.clone());
    check!("undo-move", b.exists() && !sub_b.exists(), "应搬回原处");

    let _ = launcher_clipboard_copy(vec![b.to_string_lossy().to_string()], false);
    let pasted = launcher_paste(sub_s.clone(), app.clone());
    check!(
        "copy-paste",
        pasted.as_ref().is_ok_and(|count| *count == 1) && b.exists() && sub_b.exists(),
        "{pasted:?}"
    );

    let _ = std::fs::create_dir_all(&cut_dest);
    let cut_write = launcher_clipboard_copy(vec![b.to_string_lossy().to_string()], true);
    let readback = hexbench_core::shell_ops::clipboard_get_files();
    out += &format!(
        "files: INFO cut-write — {cut_write:?} readback={:?}\n",
        readback
            .as_ref()
            .map(|(paths, cut)| (paths.len(), *cut))
    );
    let cut = launcher_paste(cut_s.clone(), app.clone());
    check!(
        "cut-paste",
        cut.as_ref().is_ok_and(|count| *count == 1)
            && !b.exists()
            && cut_dest.join("b.txt").exists(),
        "{cut:?}"
    );
    check!(
        "clipboard-cleared",
        hexbench_core::shell_ops::clipboard_get_files().is_none(),
        "剪切粘贴后剪贴板应清空"
    );

    let icon_path = cut_dest.join("b.txt").to_string_lossy().to_string();
    // 命令只把**缓存命中**的随返回值给出，没命中的在后台线程里抽、抽好发事件。
    // 所以这里叫一次，然后看后台有没有把图真的写进缓存（面板靠这个落盘的
    // PNG 走 hbicon:// 取图）。
    let _ = tauri::async_runtime::block_on(launcher_icons(
        app.clone(),
        vec![icon_path.clone()],
        None,
    ));
    let mut icon_ok = false;
    for _ in 0..20 {
        if let Some(Some(key)) =
            hexbench_core::icons::cached_icon_key(std::path::Path::new(&icon_path))
        {
            if hexbench_core::icons::cache_file(&key)
                .is_some_and(|file| file.metadata().map(|meta| meta.len() > 0).unwrap_or(false))
            {
                icon_ok = true;
                break;
            }
        }
        thread::sleep(Duration::from_millis(150));
    }
    check!("icon", icon_ok, "文本文件应能从 Shell 拿到图标并写进缓存");

    // 资源管理器会隐藏隐藏/系统属性的项，面板跟它一致。
    let hidden_dir = base.join("隐藏检查");
    let _ = std::fs::create_dir_all(&hidden_dir);
    let _ = std::fs::write(hidden_dir.join("看得见.txt"), "x");
    let hidden_file = hidden_dir.join("藏起来.txt");
    let _ = std::fs::write(&hidden_file, "x");
    let _ = std::process::Command::new("attrib")
        .arg("+h")
        .arg(&hidden_file)
        .status();
    let hidden_items = tauri::async_runtime::block_on(launcher_items(
        hidden_dir.to_string_lossy().to_string(),
        Some(false),
    ));
    let hidden_names: Vec<String> = hidden_items.iter().map(|item| item.name.clone()).collect();
    check!(
        "hidden-filtered",
        hidden_items.len() == 1 && hidden_names[0] == "看得见.txt",
        "{hidden_names:?}（隐藏文件不应出现）"
    );
    let _ = std::process::Command::new("attrib")
        .arg("-h")
        .arg(&hidden_file)
        .status();

    // 撞名时自动编号（和资源管理器一样变成「xxx - 副本.txt」），不能覆盖。
    let clash_dest = base.join("撞名目标");
    let _ = std::fs::create_dir_all(&clash_dest);
    let _ = std::fs::write(clash_dest.join("同名.txt"), "原有的");
    let clash_src = base.join("同名.txt");
    let _ = std::fs::write(&clash_src, "新来的");
    let clash = launcher_move_into(
        clash_dest.to_string_lossy().to_string(),
        vec![clash_src.to_string_lossy().to_string()],
        app.clone(),
    );
    let clash_names: Vec<String> = std::fs::read_dir(&clash_dest)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    check!(
        "collision-rename",
        clash.as_ref().is_ok_and(|count| *count == 1)
            && clash_names.len() == 2
            && std::fs::read_to_string(clash_dest.join("同名.txt")).unwrap_or_default() == "原有的",
        "{clash_names:?}"
    );

    // 大目录不再截断（面板那边是虚拟滚动 + 流式分页）。
    let many = base.join("很多");
    let _ = std::fs::create_dir_all(&many);
    for index in 0..1050 {
        let _ = std::fs::write(many.join(format!("f-{index:04}.txt")), "x");
    }
    let started = std::time::Instant::now();
    let many_items = tauri::async_runtime::block_on(launcher_items(
        many.to_string_lossy().to_string(),
        Some(false),
    ));
    check!(
        "listing-no-truncation",
        many_items.len() == 1050,
        "1050 项应全部返回，实际 {} 项，耗时 {:?}",
        many_items.len(),
        started.elapsed()
    );

    let _ = std::fs::remove_dir_all(&base);
    out += &format!("files: SUMMARY pass={pass} fail={fail}\n");
    out
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// 调试自测通道（仅 debug 构建）：文件触发、结果写文件，不依赖屏幕坐标。
#[cfg(debug_assertions)]
fn spawn_selftest(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(6));
        if !std::env::temp_dir().join("hexbench-selftest.txt").exists() {
            return;
        }
        let mut out = String::new();
        if let Some(window) = app.get_webview_window("launcher") {
            let _ = window.show();
            thread::sleep(Duration::from_millis(900));
            out += &format!(
                "panel size={:?} visible={:?}\n",
                window.inner_size(),
                window.is_visible()
            );
        } else {
            out += "panel missing\n";
        }
        match sync_web_side(&app, true) {
            Ok(()) => {
                thread::sleep(Duration::from_millis(1500));
                match app.get_webview_window(WEB_PANEL) {
                    Some(web) => out += &format!(
                        "web size={:?} visible={:?}\n",
                        web.inner_size(),
                        web.is_visible()
                    ),
                    None => out += "web missing\n",
                }
                out += &format!("site webview={}\n", app.get_webview(SITE_WEBVIEW).is_some());
            }
            Err(error) => out += &format!("web err={error}\n"),
        }
        let _ = sync_web_side(&app, false);
        thread::sleep(Duration::from_millis(500));
        out += &format!("after close web_open={}\n", WEB_PANEL_OPEN.load(Ordering::Relaxed));
        out += &selftest_launcher_files(&app);
        let _ = std::fs::write(std::env::temp_dir().join("hexbench-selftest-out.txt"), out);
    });
}

/// 面板的界面自动化：把一段 JS 放进 `%TEMP%\hexbench-ui.js`，应用会在面板里
/// 执行它（只在 debug 构建里有），脚本用 `selftest_report` 把结果写回来。
///
/// 为什么不用系统级模拟点击：面板是 always_on_top 的小窗口，别的程序浮在上面
/// 时 `mouse_event` 打不到它；而 eval 直接在页面里派发 DOM 事件，测的就是前台
/// 真实的那些 handler。
fn spawn_ui_eval(app: AppHandle) {
    if !cfg!(debug_assertions) {
        return;
    }
    thread::spawn(move || {
        let script_path = std::env::temp_dir().join("hexbench-ui.js");
        let probe_path = std::env::temp_dir().join("hexbench-probe.txt");
        loop {
            thread::sleep(Duration::from_millis(250));
            // 主线程卡死时 webview 也调不动，用文件触发 Rust 侧探针。
            if probe_path.exists() {
                let _ = std::fs::remove_file(&probe_path);
                probe_state(&app);
            }
            // 吸附参考线画布的界面自动化：它没有可见内容，出问题时只能这么看。
            let guides_script_path = std::env::temp_dir().join("hexbench-ui-guides.js");
            if guides_script_path.exists() {
                if let Ok(script) = std::fs::read_to_string(&guides_script_path) {
                    let _ = std::fs::remove_file(&guides_script_path);
                    match app.get_webview_window(GUIDES_LABEL) {
                        Some(window) => {
                            let _ = window.eval(&script);
                            log::info!(
                                "ui-eval: guides script injected url={:?} title={:?} visible={:?}",
                                window.url(),
                                window.title(),
                                window.is_visible()
                            );
                        }
                        None => log::warn!("ui-eval: guides window missing"),
                    }
                }
            }
            // 主设置窗口的界面自动化（桌面格子设置页这类）。
            let main_script_path = std::env::temp_dir().join("hexbench-ui-main.js");
            if main_script_path.exists() {
                if let Ok(script) = std::fs::read_to_string(&main_script_path) {
                    let _ = std::fs::remove_file(&main_script_path);
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.eval(&script);
                        log::info!("ui-eval: main script injected");
                    }
                }
            }
            if !script_path.exists() {
                continue;
            }
            let Ok(script) = std::fs::read_to_string(&script_path) else {
                continue;
            };
            let _ = std::fs::remove_file(&script_path);
            let Some(window) = app.get_webview_window("launcher") else {
                log::warn!("ui-eval: launcher missing");
                continue;
            };
            let _ = window.show();
            let _ = window.eval(&script);
            log::info!("ui-eval: script injected ({} bytes)", script.len());
        }
    });
}

/// 主线程 id：OLE 拖放目标必须登记在这条线程上，因为它自己泵消息。
static MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);

/// 调试自测通道（仅 debug 构建）：拖放目标登记在哪条线程上。
///
/// OLE 规定调用 `RegisterDragDrop` 的线程必须自己泵消息，否则别的程序拖着东西
/// 经过这个窗口时会一直卡在 `DragEnter` —— 表现就是桌面往收纳盒里拖文件时整个
/// 拖拽冻住。每行末尾 `main=true` 才是对的。
#[tauri::command]
async fn debug_drop_targets(app: AppHandle) -> Result<Vec<String>, String> {
    if !cfg!(debug_assertions) {
        return Err("disabled".to_string());
    }
    let main = MAIN_THREAD_ID.load(Ordering::Relaxed);
    let windows: Vec<(isize, String)> = app
        .webview_windows()
        .into_iter()
        .filter_map(|(label, window)| window.hwnd().ok().map(|hwnd| (hwnd.0 as isize, label)))
        .collect();
    let mut lines: Vec<String> = hexbench_core::drop_target::attach_report()
        .into_iter()
        .map(|(hwnd, route, registered, owner)| {
            let label = windows
                .iter()
                .find(|(handle, _)| *handle == route)
                .map(|(_, label)| label.as_str())
                .unwrap_or("-");
            format!(
                "label={label} hwnd={hwnd} route={route} registered_tid={registered} owner_tid={owner} main={} owner_match={}",
                registered == main,
                registered == owner
            )
        })
        .collect();
    lines.sort();
    Ok(lines)
}

/// 卡死诊断用：主线程回不回应、shared 锁有没有被占着。
///
/// 必须是 async 命令（跑在异步线程池上）——同步命令本身就在主线程上执行，
/// 主线程卡住时它连进来都进不来。
#[tauri::command]
async fn debug_probe(app: AppHandle) -> String {
    if !cfg!(debug_assertions) {
        return String::new();
    }
    probe_state(&app)
}

/// 主线程 / 锁状态探针：结果写进 `%TEMP%\hexbench-debug.txt`。
///
/// 不依赖 webview：主线程卡住时 IPC 也回不来，只能从普通线程里问。
fn probe_state(app: &AppHandle) -> String {
    let lock_free = {
        let state = app.state::<AppState>();
        state.shared.try_lock().is_ok()
    };
    let path = std::env::temp_dir().join("hexbench-debug.txt");
    let _ = std::fs::write(&path, format!("lock_free={lock_free}\n"));
    // run_on_main_thread 自己就可能卡住（主线程不接消息时它会一直等），
    // 所以放到单独的线程里，结果再写进文件。
    let handle = app.clone();
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let sent = handle
            .run_on_main_thread(move || {
                let _ = tx.send(());
            })
            .is_ok();
        let main_ok = if sent {
            rx.recv_timeout(Duration::from_millis(1500)).is_ok()
        } else {
            false
        };
        let _ = std::fs::write(&path, format!("lock_free={lock_free}\nmain_responsive={main_ok}\n"));
    });
    format!("lock_free={lock_free}")
}

/// 原生格子的窗口清单：给自动化验证用（没有 WebView 页面可注入脚本）。
#[tauri::command]
async fn debug_native_state(_app: AppHandle) -> String {
    if !cfg!(debug_assertions) {
        return String::new();
    }
    let host = NATIVE_FENCES.lock().ok().and_then(|slot| slot.clone());
    let mut out = format!(
        "mode=native count={}\n",
        host.as_ref().map(|host| host.hwnds().len()).unwrap_or(0)
    );
    if let Some(host) = host {
        for (index, hwnd) in host.hwnds().into_iter().enumerate() {
            let rect = hexbench_core::sys::window_rect(hwnd);
            out.push_str(&format!(
                "#{index} hwnd={hwnd} visible={} topmost={} behind_desktop={} rect={:?}\n",
                hexbench_core::sys::window_visible(hwnd),
                hexbench_core::sys::is_topmost(hwnd),
                hexbench_core::sys::behind_desktop(hwnd),
                rect.map(|rect| (rect.left, rect.top, rect.width(), rect.height())),
            ));
        }
    }
    out
}

/// 原生重绘压测：和 proto 的 `--bench` 同一套方法（滚轮滚动 + 悬停高亮）。
#[tauri::command]
async fn debug_native_bench(app: AppHandle, frames: Option<usize>) -> String {
    if !cfg!(debug_assertions) {
        return String::new();
    }
    tauri::async_runtime::spawn_blocking(move || {
        let host = NATIVE_FENCES.lock().ok().and_then(|slot| slot.clone());
        let Some(host) = host else {
            return "mode=webview (no native host)".to_string();
        };
        let desktop = desktop_snapshot(&app);
        let Some(fence) = desktop.fences.iter().find(|fence| fence.visible) else {
            return "no visible fence".to_string();
        };
        let frames = frames.unwrap_or(600);
        match host.bench(&fence.id, frames) {
            Some(bench) => format!(
                "mode=native fence={} frames={} avg={:.0}us p50={}us p95={}us max={}us",
                fence.id, bench.frames, bench.avg_us, bench.p50_us, bench.p95_us, bench.max_us
            ),
            None => "bench failed (no items?)".to_string(),
        }
    })
    .await
    .unwrap_or_else(|error| error.to_string())
}

/// 原生格子的拖放落点自检：按 OLE 回调同一份口径算一次（只算不搬）。
///
/// `x` / `y` 是窗口客户区的物理像素，和拖放回调拿到的是同一套坐标。滚动格子
/// 之后调用它，返回的应该就是此刻压在那个点下的文件夹——滚动后拖放落错目录
/// 的回归测试靠这个。
#[tauri::command]
async fn debug_native_drop_target(id: String, x: i32, y: i32) -> String {
    if !cfg!(debug_assertions) {
        return String::new();
    }
    let host = NATIVE_FENCES.lock().ok().and_then(|slot| slot.clone());
    let Some(host) = host else {
        return "mode=none".to_string();
    };
    match host.drop_target(&id, x as f32, y as f32) {
        Some(dest) => format!("dest={dest}"),
        None => "dest=".to_string(),
    }
}

/// 界面自动化脚本的回报通道。只在 debug 构建里写文件。
#[tauri::command]
fn selftest_report(text: String) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("disabled".to_string());
    }
    use std::io::Write;
    let path = std::env::temp_dir().join("hexbench-ui-out.txt");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    writeln!(file, "{text}").map_err(|error| error.to_string())
}

pub fn run() {
    // 必须在任何窗口创建之前：OLE 拖放目标要在这个线程上注册。
    hexbench_core::sys::init_ole();
    // 记下主线程：拖放目标登记到了别的线程上就是卡死的隐患（见 debug_drop_targets）。
    MAIN_THREAD_ID.store(hexbench_core::sys::current_thread_id(), Ordering::Relaxed);
    // A GUI build has no console; keep the last panic where a bug report can
    // find it instead of dying silently with 0xC0000409.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = format!("{info}\n");
        let path = std::env::temp_dir().join("hexbench-panic.log");
        let _ = std::fs::write(&path, text.as_bytes());
        default_hook(info);
    }));
    let config_path = config::default_path().unwrap_or_else(|| PathBuf::from("config.json"));
    let (loaded, source) = config::load(&config_path);
    LOAD_SOURCE.set(source).ok();
    if !config_path.exists() {
        let _ = config::save(&config_path, &loaded);
    }

    let shared = Arc::new(Mutex::new(Shared::new(loaded)));
    let engine_shared = shared.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // `hexbench-app --overlay` is a convenience for scripts and for
            // checking the zones without opening the window first.
            if args.iter().any(|arg| arg == "--quit") {
                request_quit(app);
            } else if args.iter().any(|arg| arg == "--overlay") {
                let handle = app.clone();
                thread::spawn(move || {
                    let _ = open_overlay(&handle);
                });
            } else if menu_args_handled(app, &args) {
                // 桌面右键菜单转发过来的动作（建格子 / 切换显示 / 设置页）。
            } else {
                open_main(app);
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                // Info is plenty: the diagnostic export ships the tail of this
                // file, and dependency TRACE output (certificate dumps, HTTP
                // internals) would drown our own lines.
                .level(log::LevelFilter::Info)
                .level_for("rustls", log::LevelFilter::Warn)
                .level_for("reqwest", log::LevelFilter::Warn)
                .level_for("hyper", log::LevelFilter::Warn)
                .level_for("hyper_util", log::LevelFilter::Warn)
                .level_for("tao", log::LevelFilter::Warn)
                .level_for("wry", log::LevelFilter::Warn)
                .build(),
        )
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // 呼出面板和触发提示各自记几何，别让插件恢复旧尺寸；
                // 便签的几何存在配置的 notes.pinned 里，也不需要插件插手。
                .with_filter(|label| {
                    label != "overlay"
                        && label != "launcher"
                        && !label.starts_with(NOTE_LABEL_PREFIX)
                })
                // The window is frameless by design: restoring a saved
                // "decorated" flag would bring the system title bar back.
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        & !tauri_plugin_window_state::StateFlags::DECORATIONS,
                )
                .build(),
        )
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let (pause, launcher) = {
                        let state = app.state::<AppState>();
                        let shared = state.shared.lock().unwrap();
                        (
                            shared.config.hotkeys.pause.clone(),
                            shared.config.hotkeys.launcher.clone(),
                        )
                    };
                    if shortcut_matches(shortcut, &pause) {
                        toggle_pause(app);
                    } else if shortcut_matches(shortcut, &launcher) {
                        toggle_launcher(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(AppState {
            shared,
            config_path,
            pending_update: Mutex::new(None),
            update_status: Mutex::new(UpdateStatus::default()),
            pending_page: Mutex::new(None),
            tray_menu: Mutex::new(None),
        })
        // 图标直接从磁盘给 WebView：`http://hbicon.localhost/<缓存键>`。
        // 走协议比把 base64 塞进 IPC 省得多——解码和缓存都交给引擎。
        .register_uri_scheme_protocol("hbicon", |_app, request| {
            let key = request.uri().path().trim_start_matches('/');
            let body = hexbench_core::icons::cache_file(key)
                .and_then(|file| std::fs::read(file).ok())
                .unwrap_or_default();
            let status = if body.is_empty() { 404 } else { 200 };
            tauri::http::Response::builder()
                .status(status)
                .header("Content-Type", "image/png")
                // 键里带了修改时间，内容不会再变。
                .header("Cache-Control", "public, max-age=31536000, immutable")
                // <img> 不需要 CORS，但 fetch 要：自动化测试/调试时能直接读回图标。
                .header("Access-Control-Allow-Origin", "*")
                .body(body)
                .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
        })
        // 便签里的图片给原图：`http://hbnote.localhost/<文件名>`。
        // 不放进 Markdown 的 base64：一条笔记几张图会让正文变得没法读。
        .register_uri_scheme_protocol("hbnote", |app, request| {
            let name = percent_decode(request.uri().path().trim_start_matches('/'));
            let root = app
                .app_handle()
                .state::<AppState>()
                .shared
                .lock()
                .map(|shared| shared.config.notes.folder.clone())
                .unwrap_or_default();
            let body = hexbench_core::notes::resolve_root(&root)
                .ok()
                .and_then(|root| hexbench_core::notes::image_file(&root, &name))
                .and_then(|file| std::fs::read(file).ok())
                .unwrap_or_default();
            let status = if body.is_empty() { 404 } else { 200 };
            tauri::http::Response::builder()
                .status(status)
                .header("Content-Type", hexbench_core::notes::image_mime(&name))
                // 文件名带内容哈希；同名改内容的概率很低，短缓存够了。
                .header("Cache-Control", "public, max-age=86400")
                .header("Access-Control-Allow-Origin", "*")
                .body(body)
                .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            diagnostics,
            save_config,
    ui_theme_preference,
            set_paused,
            refresh_layout,
            set_autostart,
            open_config_folder,
            create_virtual_desktop,
            hit_state,
            begin_drag,
            overlay_zones,
            show_trigger_overlay,
            running_apps,
            dismiss_launcher,
            launcher_categories,
            launcher_categories_at,
            launcher_items,
            launcher_items_stream,
            launcher_pick_folder,
            launcher_open,
            launcher_begin_drag,
            launcher_context_menu,
    launcher_background_menu,
    launcher_trash,
    launcher_undo,
    launcher_clipboard_copy,
    launcher_paste,
            launcher_rename,
            open_settings,
            take_pending_page,
            launcher_set_drop_target,
            notes_list,
            notes_read,
            notes_create,
            notes_save,
            notes_delete,
            notes_import_images,
            notes_copy_images,
            notes_folder,
            notes_default_folder,
            notes_open_folder,
            notes_clear_all,
            notes_send,
            notes_pin,
            notes_unpin,
            notes_pinned,
            open_url,
            note_state,
            note_ready,
            note_unpin,
            note_geometry,
            note_roll,
            note_rolled,
            launcher_new_folder,
    launcher_create_dir,
    launcher_set_web_mode,
    launcher_open_site,
    launcher_close_site,
    launcher_site_nav,
    site_title,
    site_icon,
    web_panel_show,
    web_side_state,
    web_side_toggle,
    web_panel_hide,
    web_page_open,
    web_page_nav,
    launcher_web_sites,
    site_icon,
    web_panel_show,
    web_side_state,
    web_panel_hide,
    web_page_open,
    web_page_nav,
    launcher_web_sites,
    launcher_site_bounds,
    selftest_report,
    debug_probe,
    debug_drop_targets,
    debug_native_state,
    debug_native_bench,
    debug_native_drop_target,
            launcher_save_location,
            launcher_icons,
            launcher_save_view,
            launcher_usage,
            launcher_move_into,
            launcher_copy_into,
            launcher_refocus,
            cache_stats,
            cache_clear,
            launcher_clear_folder,
    launcher_save_folders,
    launcher_save_category_order,
            sync_test,
            sync_save,
            sync_push,
            sync_pull,
            update_check,
            update_status,
            update_download,
            update_install,
            pick_app,
            fence_create,
            fence_create_box,
            fence_delete,
            fence_box_info,
            fence_known_folders,
            fence_rule_preview,
            fence_rule_run,
            fence_set_enabled,
            fence_set_appearance,
            fence_update,
            fence_watch_path,
            fence_path_exists
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            for note in hexbench_core::config::take_notes() {
                log::warn!("{note}");
            }
            match LOAD_SOURCE.get() {
                Some(hexbench_core::config::LoadSource::Backup) => {
                    log::warn!("config: 主配置读取失败，已经回退到备份");
                }
                Some(hexbench_core::config::LoadSource::Default) => {
                    log::info!("config: 没有可用配置，以默认值启动");
                }
                _ => {}
            }
            build_tray(&handle)?;

            let enabled = handle.autolaunch().is_enabled().unwrap_or(false);
            {
                let state = app.state::<AppState>();
                let mut shared = state.shared.lock().unwrap();
                shared.autostart = enabled;
                shared.accent = hexbench_core::sys::accent_color();
            }

            spawn_engine(engine_shared.clone());

            let args: Vec<String> = std::env::args().collect();
            // 从右键菜单启动时：动作在后台做，不弹设置窗口。
            let menu_action = menu_args_handled(&handle, &args);

            register_hotkeys(&handle);
            spawn_stall_watchdog(handle.clone());

            // Manual launch shows the window; only the autostart entry stays
            // in the tray (the plan's "开机自启绝不弹窗"). 右键菜单进来的
            // 也一样：它是来建格子 / 切换显示的，不该顺手弹出设置窗口。
            let started_by_autostart = args.iter().any(|arg| arg == "--autostart");
            if !started_by_autostart && !menu_action {
                // Setup runs before the event loop, so building here is safe.
                open_main_now(&handle);
            }
                    spawn_launcher_watcher(handle.clone());
            // 桌面格子：窗口在管理器线程上建（不占启动主线程、也不在 UI 线程
            // 重入消息循环），配置里有格子时才真正建窗。
            spawn_fence_manager(handle.clone());
            // 整理规则：启动时收一次（关掉应用期间进到桌面 / 下载里的文件），
            // 之后由 watcher 的目录变化触发。
            spawn_rule_engine(handle.clone());

            // 桌面右键菜单：按配置对一遍注册表。打开时每次都重写，应用换了
            // 目录 / 升级之后菜单指向的仍是当前 exe；关掉时只在自己装过的时候删。
            let menu_wanted = desktop_snapshot(&handle).context_menu;
            if menu_wanted || hexbench_core::desktop_menu::is_installed() {
                if let Err(error) = apply_desktop_menu(menu_wanted) {
                    log::warn!("desktop menu: sync failed: {error}");
                }
            }

            #[cfg(debug_assertions)]
            spawn_selftest(handle.clone());
            spawn_ui_eval(handle.clone());

            // A tray app can run for days: checking only at launch would mean
            // "restart to find out you have an update". latest.json is a few
            // hundred bytes, so a ten-minute poll costs nothing and gets a
            // release in front of people well inside the hour.
            let check_handle = handle.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(3));
                loop {
                    let _ = tauri::async_runtime::block_on(update_check(check_handle.clone()));
                    thread::sleep(Duration::from_secs(10 * 60));
                }
            });

            // 面板窗口在主线程上先建好（隐藏）：OLE 拖放目标必须挂在创建窗口的
            // 那个线程上，工作线程建完就退出，注册会失效 —— 表现就是外部拖进来
            // 的文件被系统送给下面的主窗口。
            if app
                .state::<AppState>()
                .shared
                .lock()
                .unwrap()
                .config
                .launcher
                .enabled
            {
        // 网页窗口也在启动时预建（隐藏）：创建窗口必须在启动阶段的主线程，
        // 运行中创建会得到「有外壳、没注册」的窗口（自测已验证）。
        // 设置里关掉内置网页就一个 WebView 都不建（这正是那个开关的承诺，
        // 之前没照做：web.html 照样加载，还自动打开了第一个站点）。
        let web_enabled = app
            .state::<AppState>()
            .shared
            .lock()
            .map(|shared| shared.config.launcher.web_enabled)
            .unwrap_or(false);
        if web_enabled {
            if let Err(error) = ensure_web_window(app.handle()) {
                log::warn!("web: pre-build failed: {error}");
            }
            // visible(false) 在透明/无边框窗口上不一定生效，补一次隐藏
            if let Some(web) = web_window_handle() {
                let _ = web.hide();
            }
        } else {
            log::info!("web: disabled in settings, no webview built");
        }
        if let Some(window) = build_launcher(app.handle()) {
                    let _ = window.hide();
                }
            }
            // 吸附参考线的画布同样在启动阶段预建（隐藏）：运行中建出来的窗口没有 webview。
            prebuild_guides(app.handle());
            // 上次钉在桌面上的便签：窗口在后台线程里建，不拖慢启动。
            restore_note_pins(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::DragDrop(drop) = event {
                // 一旦有东西被拖进来，就记下文件列表并开始盯光标（面板可能在中途才呼出）。
                if let tauri::DragDropEvent::Enter { paths, .. } = &drop {
                    if hexbench_core::drop_target::is_dragging_out() {
                        return;
                    }
                    let list: Vec<String> = paths
                        .iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect();
                    watch_external_drag(window.app_handle().clone(), list);
                }
                // Windows 上拖放目标只挂在第一个 webview 上，事件会全部报给主窗口。
                // 所以只要面板开着、光标又在面板里，就先交给面板处理。
                let app = window.app_handle();
                if let Some(launcher) = app.get_webview_window("launcher") {
                    if panel_visible(&launcher).unwrap_or(false) {
                        let frame = launcher.outer_position().ok().zip(launcher.inner_size().ok());
                        if let Some((position, size)) = frame {
                            let (cursor_x, cursor_y) = hexbench_core::sys::cursor_position();
                            let inside = cursor_x >= position.x
                                && cursor_y >= position.y
                                && cursor_x < position.x + size.width as i32
                                && cursor_y < position.y + size.height as i32;
                            if inside {
                                // 界面按客户区坐标找元素。
                                let (client_x, client_y) = launcher
                                    .hwnd()
                                    .ok()
                                    .map(|handle| {
                                        hexbench_core::sys::screen_to_client(
                                            handle.0 as isize,
                                            cursor_x,
                                            cursor_y,
                                        )
                                    })
                                    .unwrap_or((cursor_x, cursor_y));
                                match drop {
                                    tauri::DragDropEvent::Enter { .. }
                                    | tauri::DragDropEvent::Over { .. } => {
                                        let _ = app.emit(
                                            "launcher-drag-pos",
                                            serde_json::json!({ "x": client_x, "y": client_y }),
                                        );
                                    }
                                    tauri::DragDropEvent::Drop { paths, .. } => {
                                        let list: Vec<String> = paths
                                            .iter()
                                            .map(|path| path.to_string_lossy().to_string())
                                            .collect();
                                        let _ = app.emit(
                                            "launcher-dropped",
                                            serde_json::json!({
                                                "x": client_x,
                                                "y": client_y,
                                                "paths": list,
                                                "copy": hexbench_core::sys::ctrl_down(),
                                            }),
                                        );
                                    }
                                    tauri::DragDropEvent::Leave => {
                                        // 光标还在面板里却收到 Leave：Esc 取消了拖拽。
                                        if cursor_inside_launcher(app) {
                                            DRAG_CANCELLED.store(true, Ordering::SeqCst);
                                        }
                                        let _ = app.emit("launcher-drag-leave", ());
                                    }
                                    _ => {}
                                }
                                return;
                            }
                        }
                    }
                }
                if window.label() == "launcher" {
                    // 拖到面板上时，把坐标交给界面：由它判断落点在哪个格子上
                    // （文件夹 = 移进去，空白 = 放进当前目录）。
                    let app = window.app_handle();
                    match drop {
                        tauri::DragDropEvent::Enter { position, .. }
                        | tauri::DragDropEvent::Over { position } => {

                            let _ = app.emit(
                                "launcher-drag-over",
                                serde_json::json!({ "x": position.x, "y": position.y }),
                            );
                        }
                        tauri::DragDropEvent::Drop { paths, position } => {
                            log::info!("launcher: drop event at {},{}", position.x, position.y);
                            let list: Vec<String> = paths
                                .iter()
                                .map(|path| path.to_string_lossy().to_string())
                                .collect();
                            let _ = app.emit(
                                "launcher-dropped",
                                serde_json::json!({
                                    "x": position.x,
                                    "y": position.y,
                                    "paths": list,
                                }),
                            );
                        }
                        tauri::DragDropEvent::Leave => {
                            log::info!("launcher: drag left");
                            if cursor_inside_launcher(app) {
                                DRAG_CANCELLED.store(true, Ordering::SeqCst);
                            }
                            let _ = app.emit("launcher-drag-leave", ());
                        }
                        _ => {}
                    }
                    return;
                }

                return;
            }
            if let WindowEvent::Resized(_) = event {
                if window.label() == "launcher" {
                    reflow_web_page(window.app_handle());
                }
            }            // 面板是「呼出即用」的：点到别处就收起来。
            // 网页面板算它的附属窗口，两者任何一个失焦都一起收。
            if matches!(window.label(), "launcher" | "web") {
                if let WindowEvent::Focused(false) = event {
                    // 刚显示的那 500ms、以及右键菜单开着的时候，都别收起。
                    let since = LAUNCHER_SHOWN_AT.load(Ordering::Relaxed);
                    let menu_open = LAUNCHER_MENU_OPEN.load(Ordering::Relaxed);
                    // 内嵌网页是同一进程的子窗口，抢焦点时不能当成「点到别处」。
                    let left = !hexbench_core::sys::foreground_is_current_process();
                    let age = now_millis().saturating_sub(since);
                    if left && !menu_open && age > 500 && shell_op_active() {
                        // 刚从菜单里发了一条系统命令（多半是压缩 / 复制），它自己的
                        // 进度窗口把前台拿走了：这不是「点走了」，面板留着。
                        log::info!(
                            "launcher: keep panel open (shell command running, label={})",
                            window.label()
                        );
                        hide_after_shell_op(window.app_handle().clone());
                    } else if left && !menu_open && age > 500 {
                        if hexbench_core::sys::mouse_button_down() {
                            // 从资源管理器往面板里拖文件时，鼠标一按下焦点就跑了。
                            // 立刻收起的话拖放目标会凭空消失——按住期间先留着，
                            // 等松开再看光标是不是还在面板里。
                            let app = window.app_handle().clone();
                            let shown = since;
                            std::thread::spawn(move || {
                                for _ in 0..40 {
                                    thread::sleep(Duration::from_millis(100));
                                    if !hexbench_core::sys::mouse_button_down() {
                                        break;
                                    }
                                }
                                thread::sleep(Duration::from_millis(150));
                                // 这段时间里面板又被呼出过一次：那是新的一轮，别误收。
                                if LAUNCHER_SHOWN_AT.load(Ordering::Relaxed) != shown {
                                    return;
                                }
                                if cursor_inside_launcher(&app)
                                    || hexbench_core::sys::foreground_is_current_process()
                                {
                                    return;
                                }
                                hide_launcher(&app, "focus-lost");
                            });
                        } else {
                            // 走统一的收起逻辑：面板和网页面板一起收。
                            hide_launcher(window.app_handle(), "focus-lost");
                        }
                    } else {
                        log::info!(
                            "launcher: focus lost ignored (label={}, left={left}, menu={menu_open}, age={age}ms)",
                            window.label()
                        );
                    }
                }
                return;
            }
            let WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            // 便签窗和别的窗口不一样：它没有「藏起来下次再显示」的命运。
            // Alt+F4 / 系统关闭 = 取消钉住，配置里不能留下一扇打不开的窗。
            if window.label().starts_with(NOTE_LABEL_PREFIX) {
                // 退出程序时便签窗也会收到关闭请求，那是关程序，不是「取消钉住」。
                if QUITTING.load(Ordering::SeqCst) {
                    return;
                }
                let app = window.app_handle().clone();
                let id = unpin_label(&app, window.label());
                let _ = app.emit("notes-changed", NotesChanged { id, source: None });
                return;
            }
            // Other windows (launcher, overlay, fences) keep the cheap hide/show toggle.
            if window.label() != "main" {
                api.prevent_close();
                let _ = window.hide();
                return;
            }
            // The app lives in the tray, so closing the window must not quit.
            // See below: it is hidden now and destroyed a little later.
            api.prevent_close();
            // Hide first so the click feels instant. The window itself (and its
            // renderer, ~40–50 MB) is torn down a moment later: destroying a
            // WebView2 can block the event loop for a noticeable moment, but
            // keeping it around forever wastes the memory the close was meant
            // to free. Rebuilding takes ~400 ms, so a quick reopen keeps the
            // warm window; the launcher WebView is never touched.
            let _ = window.hide();
            let generation = MAIN_SHOW_GEN.load(Ordering::SeqCst);
            let window = window.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(5));
                // Reopened meanwhile. The generation also covers a show that is
                // already on its way through the event loop.
                if window.is_visible().unwrap_or(false)
                    || MAIN_SHOW_GEN.load(Ordering::SeqCst) != generation
                {
                    return;
                }
                let started = Instant::now();
                let _ = window.destroy();
                log::info!(
                    "window: main destroyed after close in {} ms",
                    started.elapsed().as_millis()
                );
            });
        })
        .build(tauri::generate_context!())
        .expect("error while building HexBench")
        .run(|_, event| match event {
            RunEvent::ExitRequested { api, .. } => {
                log::info!(
                    "quit: ExitRequested (quitting={})",
                    QUITTING.load(Ordering::SeqCst)
                );
                if !QUITTING.load(Ordering::SeqCst) {
                    api.prevent_exit();
                }
            }
            RunEvent::Exit => {
                // 原生窗口不属于 Tauri；退出前自己收干净，别留下一个常驻线程。
                native_shutdown();
                log::info!("quit: Exit");
            }
            _ => {}
        });
}
