use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
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
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use hexbench_core::config::{self, Config, DockSide};
use hexbench_core::engine::Engine;
use hexbench_core::geometry::Region;
use hexbench_core::model::{Monitor, Side};
use hexbench_core::trigger::{Direction, Hit};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MonitorDto {
    index: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
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
    test_mode: bool,
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
    test_mode: bool,
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
            test_mode: false,
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
    /// Panel is open on `panel_desktop_id`; a desktop switch hides it.
    panel_open: AtomicBool,
    panel_desktop_id: Mutex<Option<Vec<u8>>>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateStatus {
    version: Option<String>,
    error: Option<String>,
    checked_at: i64,
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

    {
        let state = app.state::<AppState>();
        let open = state.panel_open.load(Ordering::Relaxed);
        let recorded = state.panel_desktop_id.lock().unwrap().clone();
        let current = hexbench_core::sys::current_desktop_id();
        let visible = app
            .get_webview_window("phrases")
            .map(|w| w.is_visible().unwrap_or(false));
        let same = matches!((&recorded, &current), (Some(a), Some(b)) if a == b);
        out.push_str(&format!(
            "panel open={open} sameDesktop={same} recorded={:?} current={:?} visible={visible:?}\n",
            recorded.as_ref().map(|id| id.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>()),
            current.as_ref().map(|id| id.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>()),
        ));
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
        test_mode: shared.test_mode,
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

#[tauri::command]
fn save_config(
    config: Config,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    config::save(&state.config_path, &config).map_err(|error| error.to_string())?;
    {
        let mut shared = state.shared.lock().unwrap();
        shared.config = config;
    }
    register_hotkeys(&app);
    // 面板打开着的时候也能看到路径/分类变化。
    let _ = app.emit("launcher-changed", ());

    Ok(())
}

#[tauri::command]
fn set_paused(paused: bool, state: State<'_, AppState>) {
    state.shared.lock().unwrap().paused = paused;
}

/// Re-register every global shortcut from the config.
///
/// Shortcuts are declared in one place (`config.hotkeys`) and dispatched here,
/// so adding a tool cannot create a second, conflicting registration.
fn register_hotkeys(app: &AppHandle) {
    let (pause, panel, launcher) = {
        let state = app.state::<AppState>();
        let shared = state.shared.lock().unwrap();
        (
            shared.config.hotkeys.pause.clone(),
            shared.config.hotkeys.phrases_panel.clone(),
            shared.config.hotkeys.launcher.clone(),
        )
    };

    let manager = app.global_shortcut();
    let _ = manager.unregister_all();

    let mut error = None;
    for (label, combo) in [
        ("暂停 / 继续", &pause),
        ("常用语", &panel),
        ("快速呼出", &launcher),
    ] {
        if combo.trim().is_empty() {
            continue;
        }
        if let Err(reason) = manager.register(combo.as_str()) {
            log::warn!("注册热键「{label}」{combo} 失败：{reason}");
            error = Some(format!("「{label}」{combo} 注册失败，可能已被其他程序占用"));
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

/// Toggle the pause flag from outside the UI (tray, global shortcut).
fn toggle_pause(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut shared = state.shared.lock().unwrap();
    shared.paused = !shared.paused;
}

#[tauri::command]
fn set_test_mode(enabled: bool, state: State<'_, AppState>) {
    state.shared.lock().unwrap().test_mode = enabled;
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
            let height = ((monitor.h as f64 * ratio).round() as i32).min(monitor.h);
            let top = monitor.y;
            let bottom = monitor.y + monitor.h - height;
            let ranges: Vec<(i32, i32)> = match position {
                hexbench_core::model::BandPosition::Bottom => vec![(bottom, monitor.y + monitor.h)],
                hexbench_core::model::BandPosition::Top => vec![(top, top + height)],
                hexbench_core::model::BandPosition::Both => {
                    if top + height >= bottom {
                        vec![(top, monitor.y + monitor.h)]
                    } else {
                        vec![(top, top + height), (bottom, monitor.y + monitor.h)]
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
        return Err("还没有读到显示器".to_string());
    };

    let overlay = match app.get_webview_window("overlay") {
        Some(window) => window,
        None => WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
            .title("HexBench")
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
    overlay
        .emit("overlay-zones", payload)
        .map_err(|error| error.to_string())?;

    let handle = overlay.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(5000));
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhraseDto {
    id: String,
    title: String,
    text: String,
}

/// Phrases for the floating panel.
#[tauri::command]
fn phrase_items(state: State<'_, AppState>) -> Vec<PhraseDto> {
    state
        .shared
        .lock()
        .unwrap()
        .config
        .phrases
        .items
        .iter()
        .map(|phrase| PhraseDto {
            id: phrase.id.clone(),
            title: phrase.title.clone(),
            text: phrase.text.clone(),
        })
        .collect()
}

/// Persist the phrase list edited inside the panel.
#[tauri::command]
fn phrase_save(items: Vec<PhraseDto>, state: State<'_, AppState>) -> Result<(), String> {
    let mut shared = state.shared.lock().unwrap();
    let mut config = shared.config.clone();
    config.phrases.items = items
        .into_iter()
        .map(|dto| hexbench_core::config::Phrase {
            id: dto.id,
            title: dto.title,
            text: dto.text,
        })
        .collect();
    config::save(&state.config_path, &config).map_err(|error| error.to_string())?;
    shared.config = config;
    Ok(())
}

/// Move the panel with the mouse. The window never activates, so the webview
/// drag-region helper cannot be used; this tracks the cursor instead.
#[tauri::command]
fn panel_drag(app: AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("phrases").ok_or("面板未打开")?;
    let start = hexbench_core::sys::cursor_pos().ok_or("读不到光标")?;
    let origin = window.outer_position().map_err(|error| error.to_string())?;
    let handle = app.clone();
    let dragged = window.clone();
    thread::spawn(move || {
        while hexbench_core::sys::mouse_button_down() {
            if let Some(current) = hexbench_core::sys::cursor_pos() {
                let _ = dragged.set_position(PhysicalPosition::new(
                    origin.x + (current.0 - start.0),
                    origin.y + (current.1 - start.1),
                ));
            }
            thread::sleep(Duration::from_millis(16));
        }
        if let Ok(position) = dragged.outer_position() {
            save_panel_position(&handle, position.x, position.y);
        }
    });
    Ok(())
}

fn save_panel_position(app: &AppHandle, x: i32, y: i32) {
    let state = app.state::<AppState>();
    let config = {
        let mut shared = state.shared.lock().unwrap();
        shared.config.phrases.panel_x = Some(x);
        shared.config.phrases.panel_y = Some(y);
        shared.config.clone()
    };
    let _ = config::save(&state.config_path, &config);
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhrasesUi {
    collapsed: bool,
    target: String,
    send_on_click: bool,
}

#[tauri::command]
fn phrases_ui(state: State<'_, AppState>) -> PhrasesUi {
    let shared = state.shared.lock().unwrap();
    PhrasesUi {
        collapsed: shared.config.phrases.collapsed,
        target: shared.config.phrases.target.clone(),
        send_on_click: shared.config.phrases.send_on_click,
    }
}

/// Press Enter in the focused window after a phrase was typed in.
#[tauri::command]
fn phrase_submit(state: State<'_, AppState>) -> Result<(), String> {
    let send_key = state.shared.lock().unwrap().config.phrases.send_key;
    let sent = match send_key {
        hexbench_core::config::SendKey::CtrlEnter => hexbench_core::sys::press_ctrl_enter(),
        hexbench_core::config::SendKey::Enter => hexbench_core::sys::press_enter(),
    };
    if sent {
        Ok(())
    } else {
        Err("发送失败：前台窗口不接受模拟按键".to_string())
    }
}

/// Hide the panel from its own close button.
#[tauri::command]
fn panel_close(app: AppHandle) {
    if let Some(window) = app.get_webview_window("phrases") {
        let _ = window.hide();
    }
    set_panel_open(&app, false);
}

/// Persist the panel's "click to send" toggle.
#[tauri::command]
fn phrase_send_mode(send: bool, state: State<'_, AppState>) -> Result<(), String> {
    let config = {
        let mut shared = state.shared.lock().unwrap();
        shared.config.phrases.send_on_click = send;
        shared.config.clone()
    };
    config::save(&state.config_path, &config).map_err(|error| error.to_string())
}

/// Collapse the panel into a small icon, or expand it again.
#[tauri::command]
fn panel_collapse(collapsed: bool, app: AppHandle) {
    if let Some(window) = app.get_webview_window("phrases") {
        let size = if collapsed {
            tauri::LogicalSize::new(72.0, 72.0)
        } else {
            tauri::LogicalSize::new(300.0, 460.0)
        };
        let _ = window.set_size(size);

        // Growing from a bubble near the screen edge would push the panel
        // off-screen; pull it back inside the work area.
        if !collapsed {
            if let (Ok(position), Ok(outer)) = (window.outer_position(), window.outer_size()) {
                if let Some((left, top, right, bottom)) =
                    hexbench_core::sys::work_area_at(position.x, position.y)
                {
                    let width = outer.width as i32;
                    let height = outer.height as i32;
                    let x = position.x.min(right - width).max(left);
                    let y = position.y.min(bottom - height).max(top);
                    if x != position.x || y != position.y {
                        let _ = window.set_position(PhysicalPosition::new(x, y));
                    }
                }
            }
        }
    }
    let state = app.state::<AppState>();
    let config = {
        let mut shared = state.shared.lock().unwrap();
        shared.config.phrases.collapsed = collapsed;
        shared.config.clone()
    };
    let _ = config::save(&state.config_path, &config);
}

/// Editing inside the panel needs keyboard focus, so the window is made
/// activatable while the edit mode is on.
#[tauri::command]
fn panel_editing(editing: bool, app: AppHandle) {
    if let Some(window) = app.get_webview_window("phrases") {
        if let Ok(hwnd) = window.hwnd() {
            hexbench_core::sys::set_no_activate(hwnd.0 as isize, !editing);
        }
        if editing {
            let _ = window.set_focus();
        }
    }
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

#[tauri::command]
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
    config::save(&state.config_path, &config).map_err(|error| error.to_string())
}

#[tauri::command]
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

#[tauri::command]
fn sync_pull(state: State<'_, AppState>) -> Result<String, String> {
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
        config::save(&state.config_path, &remote).map_err(|error| error.to_string())?;
        shared.config = remote;
    }
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
                status.error = None;
            }
            Err(error) => {
                status.version = None;
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

/// Type one phrase into whatever window currently has focus.
#[tauri::command]
fn phrase_send(text: String) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    if hexbench_core::sys::type_text(&text) {
        Ok(())
    } else {
        Err("发送失败：前台窗口可能不接受模拟输入".to_string())
    }
}

#[tauri::command]
fn toggle_phrases_panel(app: AppHandle) {
    // A tool that is switched off should not answer its hotkey.
    if !app
        .state::<AppState>()
        .shared
        .lock()
        .unwrap()
        .config
        .phrases
        .enabled
    {
        return;
    }
    open_phrases_panel(&app);
}

/// A window belongs to the configured target when its executable matches, or
/// when both the window class and the title do (some apps rename their exe;
/// titles alone are too volatile to trust on their own).
fn matches_target(
    info: &hexbench_core::sys::WindowInfo,
    target: &str,
    class: &str,
    title: &str,
) -> bool {
    let by_exe = !target.is_empty() && info.process.eq_ignore_ascii_case(target);
    let by_class_title = !class.is_empty()
        && !title.is_empty()
        && info.class.eq_ignore_ascii_case(class)
        && info.title.contains(title);
    by_exe || by_class_title
}

/// Place the panel: dock next to the target app when it is in front, otherwise
/// fall back to wherever the user last dragged it.
fn place_panel(app: &AppHandle, window: &tauri::WebviewWindow) {
    let (saved, target, target_class, target_title, side, inside) = {
        let state = app.state::<AppState>();
        let shared = state.shared.lock().unwrap();
        let phrases = &shared.config.phrases;
        (
            phrases.panel_x.zip(phrases.panel_y),
            phrases.target.clone(),
            phrases.target_class.clone(),
            phrases.target_title.clone(),
            phrases.side,
            phrases.inside,
        )
    };

    let docked = hexbench_core::sys::foreground_window_info()
        .map(|info| matches_target(&info, &target, &target_class, &target_title))
        .unwrap_or(false);

    if !docked {
        if let Some((x, y)) = saved {
            let (width, height) = window
                .outer_size()
                .map(|size| (size.width as i32, size.height as i32))
                .unwrap_or((300, 460));
            let (x, y) = hexbench_core::sys::clamp_to_work_area(x, y, width, height);
            let _ = window.set_position(PhysicalPosition::new(x, y));
            return;
        }
    }

    let Some(rect) = hexbench_core::sys::foreground_window_rect() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let width = size.width as i32;
    let height = size.height as i32;

    let mut x = match (side, inside) {
        (DockSide::Right, false) => rect.right + 8,
        (DockSide::Left, false) => rect.left - width - 8,
        (DockSide::Right, true) => rect.right - width - 8,
        (DockSide::Left, true) => rect.left + 8,
    };
    let mut y = rect.top + 48;

    // Keep the panel on the monitor that holds the target window; docking
    // inside a narrow window could otherwise push it off screen. One point
    // query instead of enumerating every display: this runs per frame.
    let center_x = (rect.left + rect.right) / 2;
    let center_y = (rect.top + rect.bottom) / 2;
    if let Some((left, top, right, bottom)) =
        hexbench_core::sys::work_area_at(center_x, center_y)
    {
        x = x.clamp(left, (right - width).max(left));
        y = y.clamp(top, (bottom - height).max(top));
    }

    move_panel_to(window, x, y);
}

/// Move the panel with the raw Win32 call when we have a handle, so the
/// follow loop is not queued behind the Tauri event loop.
fn move_panel_to(window: &tauri::WebviewWindow, x: i32, y: i32) {
    match window.hwnd() {
        Ok(hwnd) => hexbench_core::sys::move_window_now(hwnd.0 as isize, x, y),
        Err(_) => {
            let _ = window.set_position(PhysicalPosition::new(x, y));
        }
    }
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
    for label in ["main", "phrases", "overlay"] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
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
            log::warn!("ui: main thread busy {} ms", waited.as_millis());
        }
    });
}

/// Remember that the panel is open, and which desktop it was opened on.
fn set_panel_open(app: &AppHandle, open: bool) {
    let state = app.state::<AppState>();
    state.panel_open.store(open, Ordering::Relaxed);
    let mut recorded = state.panel_desktop_id.lock().unwrap();
    *recorded = if open {
        hexbench_core::sys::current_desktop_id()
    } else {
        None
    };
}

/// Windows carries a window along on a virtual desktop switch. Hide the panel
/// while another desktop is active and show it again on return, so it stays
/// where the user opened it.
fn spawn_panel_desktop_watch(app: AppHandle) {
    // The shell event fires as the switch starts; polling the desktop id would
    // be too late and the panel would flash on the new desktop.
    hexbench_core::sys::watch_desktop_switches();

    thread::spawn(move || {
        let mut hidden_by_watch = false;
        let mut last_switch = hexbench_core::sys::desktop_switch_count();
        let mut last_switch_at = Instant::now() - Duration::from_secs(5);
        let mut ticks: u32 = 0;

        loop {
            // 30 ms keeps the hide ahead of the desktop animation.
            thread::sleep(Duration::from_millis(30));
            let (open, enabled) = {
                let state = app.state::<AppState>();
                let shared = state.shared.lock().unwrap();
                (
                    state.panel_open.load(Ordering::Relaxed),
                    shared.config.phrases.enabled,
                )
            };
            if !enabled {
                if let Some(window) = app.get_webview_window("phrases") {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                        set_panel_open(&app, false);
                        log::info!("panel: tool switched off, hiding");
                    }
                }
                continue;
            }
            if !open {
                continue;
            }

            let switches = hexbench_core::sys::desktop_switch_count();
            let switched = switches != last_switch;
            if switched {
                last_switch = switches;
                last_switch_at = Instant::now();
                ticks = 0;
            }

            ticks += 1;
            // Windows 11 24H2 no longer raises the desktop-switch event, so the
            // registry is the only signal left: check it every ~150 ms.
            if !switched && ticks < 5 {
                continue;
            }

            let Some(window) = app.get_webview_window("phrases") else {
                continue;
            };

            if switched && window.is_visible().unwrap_or(false) {
                let _ = window.hide();
                hidden_by_watch = true;
            }

            let recorded = {
                let state = app.state::<AppState>();
                state.panel_desktop_id.lock().unwrap().clone()
            };
            let (Some(recorded), Some(current)) =
                (recorded, hexbench_core::sys::current_desktop_id())
            else {
                continue;
            };
            // Right after a switch the registry can still report the old
            // desktop; wait for it to settle before deciding to show again.
            let settling = last_switch_at.elapsed() < Duration::from_millis(350);
            let visible = window.is_visible().unwrap_or(false);

            if current != recorded {
                if visible || !hidden_by_watch {
                    log::info!("panel: on another desktop, hiding");
                }
                hidden_by_watch = true;
                let _ = window.hide();
            } else if !visible && !settling {
                if hidden_by_watch {
                    log::info!("panel: back on its desktop, showing");
                }
                hidden_by_watch = false;
                place_panel(&app, &window);
                let _ = window.show();
                if let Ok(hwnd) = window.hwnd() {
                    hexbench_core::sys::set_no_activate(hwnd.0 as isize, true);
                }
            }
        }
    });
}
fn open_phrases_panel(app: &AppHandle) {
    // Never build a webview inside an event-loop callback (tray menu, global
    // shortcut): creating a WebView2 window there re-enters the loop and can
    // deadlock it. A plain worker thread is safe.
    let handle = app.clone();
    thread::spawn(move || open_phrases_panel_now(&handle));
}

fn open_phrases_panel_now(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("phrases") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
            set_panel_open(app, false);
        } else {
            place_panel(app, &window);
            let _ = window.show();
            set_panel_open(app, true);
        }
        return;
    }

    if let Some(window) = build_phrases_panel(app) {
        // A WebView2 window created and shown in the same tick can stay blank,
        // so let the webview settle before the first paint.
        let handle = app.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(400));
            let _ = window.show();
            if let Some(panel) = handle.get_webview_window("phrases") {
                place_panel(&handle, &panel);
                set_panel_open(&handle, true);
                // Showing the window lets Tauri re-apply its decorations, which
                // wipes the ex-styles set at build time. Re-assert afterwards,
                // otherwise clicking the panel steals focus from the chat app
                // and the phrase is typed into nothing.
                if let Ok(hwnd) = panel.hwnd() {
                    hexbench_core::sys::set_no_activate(hwnd.0 as isize, true);
                }
            }
        });
    }
}

fn build_phrases_panel(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    let collapsed = app
        .state::<AppState>()
        .shared
        .lock()
        .unwrap()
        .config
        .phrases
        .collapsed;
    let (width, height) = if collapsed { (72.0, 72.0) } else { (300.0, 460.0) };
    let built = WebviewWindowBuilder::new(app, "phrases", WebviewUrl::App("phrases.html".into()))
        .title("常用语")
        .inner_size(width, height)
        // Must allow the collapsed circle; the expanded panel is bigger anyway.
        .min_inner_size(72.0, 72.0)
        .decorations(false)
        .zoom_hotkeys_enabled(false)
        .transparent(true)
        // A system shadow would draw a square around the round bubble.
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build();

    if let Ok(window) = built {
        // The panel must never take focus, otherwise the caret leaves the chat
        // input and the phrase would be typed into the void.
        if let Ok(hwnd) = window.hwnd() {
            hexbench_core::sys::set_no_activate(hwnd.0 as isize, true);
        }
        place_panel(app, &window);
        Some(window)
    } else {
        None
    }
}

/// Keep the panel glued to the target app while that app is in the foreground.
fn spawn_panel_follower(app: AppHandle) {
    thread::spawn(move || {
        let mut last: Option<(i32, i32, i32, i32)> = None;
        // Frame rate only while something is actually moving; a docked panel
        // that nobody drags must not cost 6% of a core.
        let mut last_change = Instant::now() - Duration::from_secs(5);
        loop {
            let open = app.state::<AppState>().panel_open.load(Ordering::Relaxed);
            if !open {
                last = None;
                thread::sleep(Duration::from_millis(150));
                continue;
            }
            let interval = if last_change.elapsed() < Duration::from_millis(350) {
                16
            } else {
                60
            };
            thread::sleep(Duration::from_millis(interval));
            let Some(window) = app.get_webview_window("phrases") else {
                last = None;
                continue;
            };
            if !window.is_visible().unwrap_or(false) {
                // Forget the last rect so the panel is placed again on show.
                last = None;
                continue;
            }
            let (target, target_class, target_title) = {
                let state = app.state::<AppState>();
                let shared = state.shared.lock().unwrap();
                let phrases = &shared.config.phrases;
                (
                    phrases.target.clone(),
                    phrases.target_class.clone(),
                    phrases.target_title.clone(),
                )
            };
            // The rect is two cheap calls; only when it moves do we pay for
            // the process lookup that proves it is still the target.
            let Some(rect) = hexbench_core::sys::foreground_window_rect() else {
                continue;
            };
            let key = (rect.left, rect.top, rect.right, rect.bottom);
            if last == Some(key) {
                continue;
            }
            last = Some(key);
            last_change = Instant::now();
            // Nothing configured to dock to: the panel simply stays where the
            // user put it, so there is nothing to follow.
            if target.is_empty() && target_class.is_empty() && target_title.is_empty() {
                last = None;
                thread::sleep(Duration::from_millis(150));
                continue;
            }
            let Some(info) = hexbench_core::sys::foreground_window_info() else {
                continue;
            };
            if !matches_target(&info, &target, &target_class, &target_title) {
                continue;
            }
            place_panel(&app, &window);
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
    // Same rule as the phrase panel: never build a webview inside an
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

fn open_main_now(app: &AppHandle) {
    check_update_if_stale(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
            return;
    }

    let _ = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("HexBench")
        .inner_size(980.0, 660.0)
        .min_inner_size(840.0, 560.0)
        // The app draws its own title bar and window buttons.
        .decorations(false)
        .zoom_hotkeys_enabled(false)
        .center()
        .build();
}

/// 呼出面板：一个窗口装「快捷启动 + 文件区」。
///
/// 懒创建、之后常驻隐藏：呼出要的是「按一下就出」，重建 WebView2 要几百毫秒，
/// 手感立刻垮掉。第一次呼出慢一点，之后只是一次 show。
/// 拖拽一开始就把文件列表记下来：光标进入面板后 OLE 就不再发事件了，
/// 只能自己盯着光标，松手时若还在面板里就当成落在这里。
static DRAG_WATCHING: AtomicBool = AtomicBool::new(false);

fn watch_external_drag(app: AppHandle, paths: Vec<String>) {
    if paths.is_empty() || DRAG_WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(move || {
        let mut last_inside = false;
        loop {
            thread::sleep(Duration::from_millis(60));
            let (x, y) = hexbench_core::sys::cursor_position();
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
                    app.emit("launcher-drag-pos", serde_json::json!({ "x": x, "y": y }))
                } else {
                    app.emit("launcher-drag-leave", ())
                };
            } else if inside {
                let _ = app.emit("launcher-drag-pos", serde_json::json!({ "x": x, "y": y }));
            }
            if !hexbench_core::sys::left_button_down() {
                // 松手：还在面板里就是落在这里了。
                if inside {
                    let _ = app.emit(
                        "launcher-dropped",
                        serde_json::json!({ "x": x, "y": y, "paths": paths }),
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
        .title("快速呼出")
        .inner_size(780.0, 500.0)
        .min_inner_size(520.0, 320.0)
        .decorations(false)
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
                let app = app.clone();
                let late_app = app.clone();
                let attached = hexbench_core::drop_target::attach(
                    hwnd.0 as isize,
                    Box::new(move |event| {
                        use hexbench_core::drop_target::DropEvent;
                        match event {
                            DropEvent::Enter { x, y, .. } | DropEvent::Over { x, y } => {
                                let _ = app.emit(
                                    "launcher-drag-pos",
                                    serde_json::json!({ "x": x, "y": y }),
                                );
                            }
                            DropEvent::Leave => {
                                let _ = app.emit("launcher-drag-leave", ());
                            }
                            DropEvent::Drop { paths, x, y } => {
                                let list: Vec<String> = paths
                                    .iter()
                                    .map(|path| path.to_string_lossy().to_string())
                                    .collect();
                                let _ = app.emit(
                                    "launcher-dropped",
                                    serde_json::json!({ "x": x, "y": y, "paths": list }),
                                );
                            }
                        }
                    }),
                );
                match attached {
                    Ok(()) => log::info!("launcher: drop target registered"),
                    Err(error) => log::warn!("launcher: drop target failed: {error}"),
                }
                // WebView2 的子窗口是异步创建的：稍后再扫一遍补挂。
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
    // 尺寸按工作区的百分比算，换显示器也不会一下子占满或小得没法用。
    let (width, height) = match area {
        Some((left, top, right, bottom)) => {
            let width = ((right - left) * size_percent.0 as i32 / 100).clamp(520, 1400);
            let height = ((bottom - top) * size_percent.1 as i32 / 100).clamp(320, 1100);
            (width, height)
        }
        None => (780, 500),
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

/// 面板现在浏览到哪个文件夹：中转站可以进子目录，拖进来的文件就放进这里。
static LAUNCHER_DROP_TARGET: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

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
fn reveal_launcher(window: &tauri::WebviewWindow, appearance: (bool, (u32, u32))) {
    place_launcher(window, appearance.0, appearance.1);
    LAUNCHER_SHOWN_AT.store(now_millis(), Ordering::Relaxed);
    // 网页模式下窗口是竖版的：这时候再按配置的 50%×50% 摆一次，
    // 网页仍按竖版矩形挂着，两边尺寸对不上就是「排版错位」。
    let _ = window.show();
    // 设置里开了内置网页：把预建好的网页窗口摆到右侧并显示
    // （只 show，不在这里创建窗口——运行中建窗口会卡住面板）。
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
                place_web_window(app, &web);
                let _ = web.show();
                WEB_PANEL_OPEN.store(true, Ordering::Relaxed);
            }
        }
    }
    let _ = window.set_focus();
    // 之前开着网页面板的话，跟着一起回来（页面没重新加载）。
    // 之前开着网页区的话，跟着一起恢复（子 WebView 留在窗口里，不重新加载）。
    if WEB_PANEL_OPEN.load(Ordering::Relaxed) {
        let handle = window.app_handle().clone();
        let _ = sync_web_side(&handle, true);
    }
    // Windows 的前台锁会吞掉单纯的 show+focus，必须走 force_foreground，
    // 否则按完快捷键打不了字。
    // show() 只是排进事件循环，此刻窗口可能还没真正绘制出来；延迟一点再抢
    // 一次前台，比在同一帧里连调两次可靠。
    let handle = window.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(130));
        if let Ok(hwnd) = handle.hwnd() {
            hexbench_core::sys::force_foreground(hwnd.0 as isize);
        }
    });
    // 网页窗口跟着面板一起回来：页面还在里面，不会重新加载。
    if let Some(site) = window.app_handle().get_webview_window(SITE_WEBVIEW) {
        place_site_window(window, &site);
        let _ = site.show();
    }
    let _ = window.app_handle().emit("launcher-shown", ());
    log::info!("launcher: shown");
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
    open_launcher(app);
}



fn hide_launcher(app: &AppHandle) {
    log::info!(
        "launcher: hide (site_open={})",
        app.get_webview_window(SITE_WEBVIEW).is_some()
    );
    if let Some(window) = app.get_webview_window("launcher") {
        let _ = window.hide();
    // 网页窗口是独立窗口：面板收起时一起隐藏
    if let Some(web) = web_window_handle() {
        let _ = web.hide();
    }
    }
    // 网页面板跟着面板一起收起：页面继续跑，但不再挡在前台。
    // 用句柄隐藏：这个窗口会从注册表消失，查不到。
    // 网页是面板窗口里的子 WebView，跟着窗口一起隐藏，不用单独处理。
    let keep = app
        .state::<AppState>()
        .shared
        .lock()
        .unwrap()
        .config
        .launcher
        .site_keep_alive_secs;
    if keep == 0 {
        close_site_window(app);
        return;
    }
    let handle = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(keep as u64));
        let visible = handle
            .get_webview_window("launcher")
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false);
        if visible {
            return;
        }
        log::info!("launcher: page idle for too long, closing the site window");
        close_site_window(&handle);
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
    if open {
        ensure_web_window(app)?;
    }
    if let Some(web) = web_window_handle() {
        if open {
            place_web_window(app, &web);
            let _ = web.show();
        } else {
            let _ = web.hide();
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
    let height = (area_h as f64 * 0.85).max(320.0);
    let built = tauri::WebviewWindowBuilder::new(
        app,
        WEB_PANEL,
        tauri::WebviewUrl::App("web.html".into()),
    )
    .title("网页")
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
    let pct_width = (area_w as f64 * width_percent as f64 / 100.0).max(240.0);
    let pct_height = (area_h as f64 * height_percent as f64 / 100.0).max(240.0);

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
                    _ => (area_x as f64, area_y as f64),
                }
            }
            None => (area_x as f64, area_y as f64),
        };
        (pct_width, pct_height, left, top)
    } else if let Some((saved_x, saved_y, saved_w, saved_h)) = saved {
        // 用户拖过：位置和大小都按他拖出来的来
        (saved_w, saved_h, saved_x as f64, saved_y as f64)
    } else if side == "left" {
        (
            pct_width,
            pct_height,
            area_x as f64,
            area_y as f64 + (area_h as f64 - pct_height) / 2.0,
        )
    } else {
        (
            pct_width,
            pct_height,
            area_x as f64 + area_w as f64 - pct_width,
            area_y as f64 + (area_h as f64 - pct_height) / 2.0,
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

/// 用户拖动网页窗口（移动或拖边角改大小）后：重排子网页 + 记住几何。
fn remember_web_geometry(window: &tauri::WebviewWindow) {
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



/// 网页窗口的句柄。
///
/// 不能用 `get_webview_window("web")` 去找：实测这个窗口的 webview 注册了、
/// 窗口实体却没进注册表（自测输出 `web missing` 就是这么来的）。
static WEB_WINDOW: Mutex<Option<tauri::WebviewWindow>> = Mutex::new(None);

/// 取网页窗口句柄。
fn web_window_handle() -> Option<tauri::WebviewWindow> {
    WEB_WINDOW.lock().ok().and_then(|held| held.clone())
}

/// 网页面板当前是不是「打开着」（用户点了 ✕ 才算关）。
/// 面板收起时网页跟着藏，但状态保留——下次呼出面板它一起回来。
static WEB_PANEL_OPEN: AtomicBool = AtomicBool::new(false);

/// 网页当前打开的地址：地址没变就不重新导航（保住滚动位置和未完成的回答）。
static LAST_SITE_URL: Mutex<Option<String>> = Mutex::new(None);

fn close_site_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(SITE_WEBVIEW) {
        let _ = window.close();
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
    let normalized = hexbench_core::net::normalize_url(&url);
    let parsed: tauri::Url = normalized
        .parse()
        .map_err(|_| format!("网址不对：{url}"))?;
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
    let normalized = hexbench_core::net::normalize_url(&url);
    let parsed: tauri::Url = normalized
        .parse()
        .map_err(|_| format!("网址不对：{url}"))?;
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
    let Some(window) = app.get_webview_window(SITE_WEBVIEW) else {
        return;
    };
    let script = match action.as_str() {
        "back" => "history.back()",
        "forward" => "history.forward()",
        "reload" => "location.reload()",
        _ => return,
    };
    let _ = window.eval(script);
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
    match app.get_webview_window("launcher") {
        Some(window) if window.is_visible().unwrap_or(false) => hide_launcher(app),
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
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };

    let mut items: Vec<LauncherItem> = Vec::new();
    for entry in entries.flatten() {
        if items.len() >= 300 {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let metadata = entry.metadata().ok();
        let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = metadata
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let icon = if with_icons {
            hexbench_core::icons::file_icon_base64(&path, is_dir)
        } else {
            None
        };
        items.push(LauncherItem {
            name,
            path: path.to_string_lossy().to_string(),
            is_dir,
            size,
            modified,
            icon,
        });
    }
    items.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    items
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
    /// 上次用的视图和排序。
    view: String,
    sort_key: String,
    sort_dir: i32,
    /// 下次呼出要不要回到上次的位置。
    remember_location: bool,
    last_kind: String,
    last_base: String,
    last_label: String,
    last_path: String,
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
            view: "grid".to_string(),
            sort_key: "name".to_string(),
            sort_dir: 1,
            remember_location: false,
            last_kind: String::new(),
            last_base: String::new(),
            last_label: String::new(),
            last_path: String::new(),
        })
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
    let web_enabled = shared.config.launcher.web_enabled;
    drop(shared);
    let all_visible = !hidden.iter().any(|entry| entry == "__all__");
    let empty = |root: String| LauncherRoot {
        root,
        categories: Vec::new(),
        all_visible,
        folders: folders.clone(),
        sites: sites.clone(),
        web_enabled,
        view: view.clone(),
        sort_key: sort_key.clone(),
        sort_dir,
        remember_location,
        last_kind: location.kind.clone(),
        last_base: location.base.clone(),
        last_label: location.label.clone(),
        last_path: location.path.clone(),
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
        view,
        sort_key,
        sort_dir,
        remember_location,
        last_kind: location.kind,
        last_base: location.base,
        last_label: location.label,
        last_path: location.path,
    }
}

/// 记住面板这次停在哪儿（只有开启「记住上次位置」时才写盘）。
#[tauri::command]
fn launcher_save_location(
    kind: String,
    base: String,
    label: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut shared = state.shared.lock().unwrap();
    let launcher = &mut shared.config.launcher;
    if !launcher.remember_location {
        return Ok(());
    }
    if launcher.last_location.kind == kind
        && launcher.last_location.base == base
        && launcher.last_location.path == path
    {
        return Ok(());
    }
    launcher.last_location = hexbench_core::config::LauncherLocation {
        kind,
        base,
        label,
        path,
    };
    let config = shared.config.clone();
    drop(shared);
    hexbench_core::config::save(&state.config_path, &config).map_err(|error| error.to_string())
}

/// 从面板跳到设置页：把主窗口叫出来，再告诉前端要打开哪一页。
#[tauri::command]
fn open_settings(page: String, tab: Option<String>, app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit("navigate", serde_json::json!({ "page": page, "tab": tab }));
}
/// 记住面板的视图和排序方式。
#[tauri::command]
fn launcher_save_view(
    view: String,
    sort_key: String,
    sort_dir: i32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut shared = state.shared.lock().unwrap();
    let launcher = &mut shared.config.launcher;
    if launcher.view == view && launcher.sort_key == sort_key && launcher.sort_dir == sort_dir {
        return Ok(());
    }
    launcher.view = view;
    launcher.sort_key = sort_key;
    launcher.sort_dir = sort_dir;
    let config = shared.config.clone();
    drop(shared);
    hexbench_core::config::save(&state.config_path, &config).map_err(|error| error.to_string())
}
/// 把文件移进某个文件夹（面板内部拖拽用）。
#[tauri::command]
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
        return Err("文件已经不在了".to_string());
    }
    let moved = move_paths_into(&dir, &sources);
    // 记下「新位置 ← 旧位置」，撤销时搬回去
    let pairs: Vec<(PathBuf, PathBuf)> = sources
        .iter()
        .filter_map(|source| {
            let name = source.file_name()?;
            Some((dir.join(name), source.clone()))
        })
        .collect();
    if !pairs.is_empty() {
        push_undo(UndoOp::Move(pairs));
    }
    log::info!("launcher: moved {moved} item(s) into {}", dir.display());
    let _ = app.emit("launcher-changed", ());
    Ok(moved)
}
/// 面板告诉后端「我现在在哪个目录」，拖进来的文件就放进这里。
#[tauri::command]
fn launcher_set_drop_target(path: Option<String>) {
    log::info!("launcher: drop target = {path:?}");
    if let Ok(mut target) = LAUNCHER_DROP_TARGET.lock() {
        *target = path.map(PathBuf::from);
    }
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
    push_undo(UndoOp::NewFolder(target.clone()));
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

/// 批量补图标：面板先出名字，图标随后分批填进来。
#[tauri::command]
async fn launcher_icons(paths: Vec<String>) -> Vec<(String, Option<String>)> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|raw| {
                let path = std::path::PathBuf::from(&raw);
                let is_dir = path.is_dir();
                let icon = hexbench_core::icons::file_icon_base64(&path, is_dir);
                (raw, icon)
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default()
}

/// 面板里按住一个条目拖出去：交给 OLE，资源管理器 / 浏览器都认。
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

    // 面板在屏幕上的位置：松手时用它判断「还在面板里」。
    let frame = app.get_webview_window("launcher").and_then(|window| {
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
    let stop = Arc::new(AtomicBool::new(false));
    let poller = {
        let app = app.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let (x, y) = hexbench_core::sys::cursor_position();
                let _ = app.emit(
                    "launcher-drag-pos",
                    serde_json::json!({ "x": x, "y": y }),
                );
                thread::sleep(Duration::from_millis(60));
            }
        })
    };

    hexbench_core::drop_target::set_drag_out(true);
    let result = hexbench_core::sys::start_file_drag(&targets);
    hexbench_core::drop_target::set_drag_out(false);

    stop.store(true, Ordering::SeqCst);
    let _ = poller.join();

    // 松手时如果还在面板里、而且左键已经松开（不是 Esc 取消），就把落点交给界面：
    // 落在文件夹上 = 移进去，落在空白 = 放进当前目录。
    if !hexbench_core::sys::left_button_down() {
        if let Some((left, top, width, height)) = frame {
            let (x, y) = hexbench_core::sys::cursor_position();
            if x >= left && y >= top && x < left + width && y < top + height {
                let list: Vec<String> = targets
                    .iter()
                    .map(|path| path.to_string_lossy().to_string())
                    .collect();
                let _ = app.emit(
                    "launcher-drag-drop",
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
    /// 新建文件夹
    NewFolder(PathBuf),
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
#[tauri::command]
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
        UndoOp::NewFolder(path) => {
            hexbench_core::shell_ops::trash(&[path]).map_err(|error| error.to_string())?;
            "已删除刚新建的文件夹".to_string()
        }
    };
    let _ = app.emit("launcher-changed", ());
    log::info!("undo: {message}");
    Ok(message)
}
/// 删除到回收站（系统会弹确认框）。
#[tauri::command]
fn launcher_trash(paths: Vec<String>) -> Result<usize, String> {
    log::info!("trash: {} items", paths.len());
    let targets: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let removed = hexbench_core::shell_ops::trash(&targets)?;
    if removed > 0 {
        push_undo(UndoOp::Trash(targets));
    }
    Ok(removed)
}

/// 把选中项放上剪贴板（cut=true 表示剪切）。
#[tauri::command]
fn launcher_clipboard_copy(paths: Vec<String>, cut: bool) -> Result<(), String> {
    log::info!("clipboard: copy {} items cut={cut}", paths.len());
    let targets: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    hexbench_core::shell_ops::clipboard_set_files(&targets, cut)
}

/// 粘贴剪贴板里的文件到目标目录。
#[tauri::command]
fn launcher_paste(dest: String) -> Result<usize, String> {
    log::info!("clipboard: paste into {dest}");
    hexbench_core::shell_ops::paste_into(&PathBuf::from(dest))
}
/// 文件夹空白处的原生右键菜单（新建 / 粘贴 / 属性 / 刷新）。
#[tauri::command]
fn launcher_background_menu(path: String, app: AppHandle) -> Result<(), String> {
    log::info!("bg menu: invoked for {path}");
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err("目录已经不在了".to_string());
    }
    let (x, y) = hexbench_core::sys::cursor_position();
    let owner = app
        .get_webview_window("launcher")
        .and_then(|window| window.hwnd().ok())
        .map(|hwnd| hwnd.0 as isize)
        .unwrap_or(0);
    // 剪贴板里有文件时才让「粘贴」可用
    let can_paste = hexbench_core::shell_ops::clipboard_get_files().is_some();
    LAUNCHER_MENU_OPEN.store(true, Ordering::Relaxed);
    let result = hexbench_core::sys::show_folder_background_menu(&dir, x, y, owner, can_paste);
    LAUNCHER_MENU_OPEN.store(false, Ordering::Relaxed);
    if let Ok(Some("paste")) = &result {
        if let Err(error) = hexbench_core::shell_ops::paste_into(&dir) {
            log::warn!("background menu: paste failed: {error}");
        }
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
/// 面板里右键：弹出系统原生的文件菜单（和资源管理器同一套）。
/// 面板里右键：系统原生菜单 + 自己补的重命名 / 删除。
///
/// 返回 `"rename"` 表示用户选了重命名（改名要用户输入，交给界面做），
/// `"deleted"` 表示删到回收站了，`None` 表示没选或执行了系统命令。
#[tauri::command]
fn launcher_context_menu(path: String, app: AppHandle) -> Result<Option<String>, String> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err("文件已经不在了".to_string());
    }
    let (x, y) = hexbench_core::sys::cursor_position();
    let owner = app
        .get_webview_window("launcher")
        .and_then(|window| window.hwnd().ok())
        .map(|hwnd| hwnd.0 as isize)
        .unwrap_or(0);
    log::info!("launcher: context menu for {path} at {x},{y}");
    LAUNCHER_MENU_OPEN.store(true, Ordering::Relaxed);
    let result = hexbench_core::sys::show_shell_context_menu(&target, x, y, owner);
    LAUNCHER_MENU_OPEN.store(false, Ordering::Relaxed);
    match result {
        Ok(choice) => {
            // 菜单关掉之后窗口不一定还拿着键盘焦点；要改名就必须先把焦点抢回来，
            // 否则输入框看得见却收不到按键。

            let _ = app.emit("launcher-changed", ());
            Ok(choice.map(|value| value.to_string()))
        }
        Err(error) => {
            log::warn!("launcher: context menu failed: {error}");
            Err(error)
        }
    }
}

/// 在面板里给文件改名（同一目录内）。
#[tauri::command]
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
/// 面板里双击：交给系统默认程序。
#[tauri::command]
fn launcher_open(path: String) -> Result<(), String> {
    hexbench_core::sys::open_path(PathBuf::from(path).as_path())
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
                list
            };

            if wanted != current {
                session = None;
                let (sender, rx) = std::sync::mpsc::channel();
                if let Ok(mut created) = notify::recommended_watcher(move |event| {
                    let _ = sender.send(event);
                }) {
                    let mut watched = 0usize;
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
                current = wanted;
            }

            match session.as_ref() {
                Some((_watcher, rx)) => match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(Ok(_)) => {
                        // 解压 / 批量复制会连发几百个事件，合并成一次刷新。
                        thread::sleep(Duration::from_millis(250));
                        while rx.try_recv().is_ok() {}
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
    hide_launcher(&app);
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {    let open = MenuItem::with_id(app, "open", "打开主界面", true, None::<&str>)?;
    let test = MenuItem::with_id(app, "test", "测试模式", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "暂停 / 继续", true, None::<&str>)?;
    let phrases = MenuItem::with_id(app, "phrases", "常用语", true, None::<&str>)?;
    let separator_a = PredefinedMenuItem::separator(app)?;
    let separator_b = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &separator_a, &test, &pause, &phrases, &separator_b, &quit],
    )?;

    let icon = match app.default_window_icon() {
        Some(icon) => icon.clone(),
        None => tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?,
    };

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("HexBench · 海克斯")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => open_main(app),
            "quit" => request_quit(app),
            "test" => {
                let state = app.state::<AppState>();
                let mut shared = state.shared.lock().unwrap();
                shared.test_mode = !shared.test_mode;
            }
            "pause" => toggle_pause(app),
            "phrases" => open_phrases_panel(app),
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

fn spawn_engine(shared: Arc<Mutex<Shared>>) {
    thread::spawn(move || {
        let initial = shared.lock().unwrap().config.clone();
        let mut engine = Engine::new(initial, false);
        let mut tick_count: u64 = 0;
        let mut last_guard: Option<String> = None;

        loop {
            tick_count += 1;
            let (desired, paused, test_mode, refresh_requested, poll_ms) = {
                let mut state = shared.lock().unwrap();
                let refresh = std::mem::take(&mut state.refresh_requested);
                (
                    state.config.clone(),
                    state.paused,
                    state.test_mode,
                    refresh,
                    state.config.trigger.poll_ms,
                )
            };

            if refresh_requested || engine.config() != &desired {
                engine.set_config(desired.clone());
            }
            engine.set_dry_run(test_mode);

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
        let _ = std::fs::write(std::env::temp_dir().join("hexbench-selftest-out.txt"), out);
    });
}

pub fn run() {
    // 必须在任何窗口创建之前：OLE 拖放目标要在这个线程上注册。
    hexbench_core::sys::init_ole();
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
            } else if args.iter().any(|arg| arg == "--panel") {
                open_phrases_panel(app);
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
                // The panel remembers its own geometry (config.phrases.panel*)
                // and changes size on collapse; letting the plugin restore a
                // stale size would fight both.
                .with_denylist(&["phrases", "overlay", "launcher"])
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
                    let (pause, panel, launcher) = {
                        let state = app.state::<AppState>();
                        let shared = state.shared.lock().unwrap();
                        (
                            shared.config.hotkeys.pause.clone(),
                            shared.config.hotkeys.phrases_panel.clone(),
                            shared.config.hotkeys.launcher.clone(),
                        )
                    };
                    if shortcut_matches(shortcut, &pause) {
                        toggle_pause(app);
                    } else if shortcut_matches(shortcut, &panel) {
                        open_phrases_panel(app);
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
            panel_open: AtomicBool::new(false),
            panel_desktop_id: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            diagnostics,
            save_config,
            set_paused,
            set_test_mode,
            refresh_layout,
            set_autostart,
            open_config_folder,
            create_virtual_desktop,
            hit_state,
            begin_drag,
            overlay_zones,
            show_trigger_overlay,
            running_apps,
            phrase_items,
            phrase_send,
            phrase_save,
            panel_drag,
            panel_editing,
            panel_collapse,
            phrases_ui,
            phrase_submit,
            panel_close,
            dismiss_launcher,
            launcher_categories,
            launcher_categories_at,
            launcher_items,
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
            launcher_set_drop_target,
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
            launcher_save_location,
            launcher_icons,
            launcher_save_view,
            launcher_move_into,
            cache_stats,
            cache_clear,
            launcher_clear_folder,
    launcher_save_folders,
    launcher_save_category_order,
            phrase_send_mode,
            sync_test,
            sync_save,
            sync_push,
            sync_pull,
            update_check,
            update_status,
            update_download,
            update_install,
            pick_app,
            toggle_phrases_panel
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
            spawn_panel_follower(handle.clone());
            spawn_panel_desktop_watch(handle.clone());

            if std::env::args().any(|arg| arg == "--panel") {
                let panel_handle = handle.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(600));
                    open_phrases_panel(&panel_handle);
                });
            }

            register_hotkeys(&handle);
            spawn_stall_watchdog(handle.clone());

            // Manual launch shows the window; only the autostart entry stays
            // in the tray (the plan's "开机自启绝不弹窗").
            let started_by_autostart = std::env::args().any(|arg| arg == "--autostart");
            if !started_by_autostart {
                // Setup runs before the event loop, so building here is safe.
                open_main_now(&handle);
            }
                    spawn_launcher_watcher(handle.clone());
            #[cfg(debug_assertions)]
            spawn_selftest(handle.clone());

            // A tray app can run for days: checking only at launch would mean
            // "restart to find out you have an update". latest.json is a few
            // hundred bytes, so half-hourly is cheap.
            let check_handle = handle.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(3));
                loop {
                    let _ = tauri::async_runtime::block_on(update_check(check_handle.clone()));
                    thread::sleep(Duration::from_secs(30 * 60));
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
        if let Err(error) = ensure_web_window(app.handle()) {
            log::warn!("web: pre-build failed: {error}");
        }
        // visible(false) 在透明/无边框窗口上不一定生效，补一次隐藏
        if let Some(web) = web_window_handle() {
            let _ = web.hide();
        }
        if let Some(window) = build_launcher(app.handle()) {
                    let _ = window.hide();
                }
            }
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
                    if launcher.is_visible().unwrap_or(false) {
                        let frame = launcher.outer_position().ok().zip(launcher.inner_size().ok());
                        if let Some((position, size)) = frame {
                            let (cursor_x, cursor_y) = hexbench_core::sys::cursor_position();
                            let inside = cursor_x >= position.x
                                && cursor_y >= position.y
                                && cursor_x < position.x + size.width as i32
                                && cursor_y < position.y + size.height as i32;
                            if inside {
                                match drop {
                                    tauri::DragDropEvent::Enter { .. }
                                    | tauri::DragDropEvent::Over { .. } => {
                                        let _ = app.emit(
                                            "launcher-drag-pos",
                                            serde_json::json!({ "x": cursor_x, "y": cursor_y }),
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
                                                "x": cursor_x,
                                                "y": cursor_y,
                                                "paths": list,
                                            }),
                                        );
                                    }
                                    tauri::DragDropEvent::Leave => {
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
                    if left && !menu_open && now_millis().saturating_sub(since) > 500 {
                        // 走统一的收起逻辑：面板和网页面板一起收。
                        hide_launcher(window.app_handle());
                    }
                }
                return;
            }
            let WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            // Other windows (the phrase panel) keep the cheap hide/show toggle.
            if window.label() != "main" {
                api.prevent_close();
                let _ = window.hide();
                return;
            }
            // The app lives in the tray, so closing the window must not quit.
            // See below: it is hidden now and destroyed a little later.
            api.prevent_close();
            // Hide, never destroy on the spot: destroying a WebView2 can block
            // the event loop for a noticeable moment, and rebuilding it on the
            // next open costs even more. The window is torn down later, once it
            // is clear the user is not coming straight back.
            let _ = window.hide();
            let window = window.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(30));
                if window.is_visible().unwrap_or(false) {
                    return; // reopened meanwhile, keep the warm window
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
            RunEvent::Exit => log::info!("quit: Exit"),
            _ => {}
        });
}
