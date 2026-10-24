use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_updater::UpdaterExt;
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
    let (pause, panel) = {
        let state = app.state::<AppState>();
        let shared = state.shared.lock().unwrap();
        (
            shared.config.hotkeys.pause.clone(),
            shared.config.hotkeys.phrases_panel.clone(),
        )
    };

    let manager = app.global_shortcut();
    let _ = manager.unregister_all();

    let mut error = None;
    for (label, combo) in [("暂停 / 继续", &pause), ("快捷语面板", &panel)] {
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

/// Paint the current trigger zones on top of the real screens for a few seconds.
///
/// The overlay is click-through and unfocused, so it is a pure visual hint: the
/// user can keep working while it fades away on its own.
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
        return Err("还没有读到显示器".to_string());
    };

    let overlay = match app.get_webview_window("overlay") {
        Some(window) => window,
        None => WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
            .title("HexBench")
            .decorations(false)
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

    overlay
        .set_position(PhysicalPosition::new(origin_x, origin_y))
        .map_err(|error| error.to_string())?;
    overlay
        .set_size(PhysicalSize::new(width, height))
        .map_err(|error| error.to_string())?;
    overlay
        .set_ignore_cursor_events(true)
        .map_err(|error| error.to_string())?;
    overlay.show().map_err(|error| error.to_string())?;
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
    open_overlay(&app)
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
fn phrase_submit() -> Result<(), String> {
    if hexbench_core::sys::press_enter() {
        Ok(())
    } else {
        Err("发送失败：前台窗口不接受模拟按键".to_string())
    }
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
    let updater = build_updater(&app)?;
    let update = updater.check().await.map_err(|error| error.to_string())?;
    Ok(update.map(|update| UpdateInfoDto {
        version: update.version.clone(),
        notes: update.body.clone().unwrap_or_default(),
        date: update.date.map(|date| date.to_string()).unwrap_or_default(),
    }))
}

#[tauri::command]
async fn update_install(app: AppHandle) -> Result<(), String> {
    let updater = build_updater(&app)?;
    let Some(update) = updater.check().await.map_err(|error| error.to_string())? else {
        return Err("没有可用的更新".to_string());
    };

    let progress_app = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                let _ = progress_app.emit("update-progress", UpdateProgress { chunk, total });
            },
            || {},
        )
        .await
        .map_err(|error| error.to_string())
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
    // inside a narrow window could otherwise push it off screen.
    let center_x = (rect.left + rect.right) / 2;
    let center_y = (rect.top + rect.bottom) / 2;
    let monitors = hexbench_core::sys::enumerate_monitors();
    if let Some(monitor) = monitors.iter().find(|monitor| {
        let area = monitor.rect;
        center_x >= area.left && center_x < area.right && center_y >= area.top && center_y < area.bottom
    }) {
        let work = monitor.work;
        let max_x = (work.right - width).max(work.left);
        let max_y = (work.bottom - height).max(work.top);
        x = x.clamp(work.left, max_x);
        y = y.clamp(work.top, max_y);
    }

    let _ = window.set_position(PhysicalPosition::new(x, y));
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
        } else {
            place_panel(app, &window);
            let _ = window.show();
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
        .title("快捷语")
        .inner_size(width, height)
        .min_inner_size(240.0, 200.0)
        .decorations(false)
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
        loop {
            thread::sleep(Duration::from_millis(250));
            let Some(window) = app.get_webview_window("phrases") else {
                last = None;
                continue;
            };
            if !window.is_visible().unwrap_or(false) {
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
            let Some(info) = hexbench_core::sys::foreground_window_info() else {
                continue;
            };
            if !matches_target(&info, &target, &target_class, &target_title) {
                continue;
            }
            let Some(rect) = hexbench_core::sys::foreground_window_rect() else {
                continue;
            };
            let key = (rect.left, rect.top, rect.right, rect.bottom);
            if last == Some(key) {
                continue;
            }
            last = Some(key);
            place_panel(&app, &window);
        }
    });
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
        .center()
        .build();
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开主界面", true, None::<&str>)?;
    let test = MenuItem::with_id(app, "test", "测试模式", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "暂停 / 继续", true, None::<&str>)?;
    let phrases = MenuItem::with_id(app, "phrases", "快捷语面板", true, None::<&str>)?;
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
            "quit" => app.exit(0),
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
pub fn run() {
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
    let (loaded, _source) = config::load(&config_path);
    if !config_path.exists() {
        let _ = config::save(&config_path, &loaded);
    }

    let shared = Arc::new(Mutex::new(Shared::new(loaded)));
    let engine_shared = shared.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // `hexbench-app --overlay` is a convenience for scripts and for
            // checking the zones without opening the window first.
            if args.iter().any(|arg| arg == "--overlay") {
                let _ = open_overlay(app);
            } else if args.iter().any(|arg| arg == "--panel") {
                open_phrases_panel(app);
            } else {
                open_main(app);
            }
        }))
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // The panel remembers its own geometry (config.phrases.panel*)
                // and changes size on collapse; letting the plugin restore a
                // stale size would fight both.
                .with_denylist(&["phrases", "overlay"])
                .build(),
        )
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let (pause, panel) = {
                        let state = app.state::<AppState>();
                        let shared = state.shared.lock().unwrap();
                        (
                            shared.config.hotkeys.pause.clone(),
                            shared.config.hotkeys.phrases_panel.clone(),
                        )
                    };
                    if shortcut_matches(shortcut, &pause) {
                        toggle_pause(app);
                    } else if shortcut_matches(shortcut, &panel) {
                        open_phrases_panel(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(AppState { shared, config_path })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            save_config,
            set_paused,
            set_test_mode,
            refresh_layout,
            set_autostart,
            open_config_folder,
            create_virtual_desktop,
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
            phrase_send_mode,
            sync_test,
            sync_save,
            sync_push,
            sync_pull,
            update_check,
            update_install,
            pick_app,
            toggle_phrases_panel
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
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
            if std::env::args().any(|arg| arg == "--panel") {
                let panel_handle = handle.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(600));
                    open_phrases_panel(&panel_handle);
                });
            }

            register_hotkeys(&handle);

            // Manual launch shows the window; only the autostart entry stays
            // in the tray (the plan's "开机自启绝不弹窗").
            let started_by_autostart = std::env::args().any(|arg| arg == "--autostart");
            if !started_by_autostart {
                open_main(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Hide instead of destroy: destroying the last window makes Tauri
                // end the event loop, which quits a tray app. The WebView stays
                // alive, so closing only wins the screen, not the RAM.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running HexBench");
}
