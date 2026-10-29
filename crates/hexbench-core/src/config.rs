//! Configuration: one JSON file, atomic writes, unknown fields preserved.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{BandPosition, Side, TriggerConfig};

/// 配置读取发生在日志系统初始化之前，提示先攒在这里，等 setup 里再输出。
static NOTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn note(message: String) {
    log::warn!("{message}");
    if let Ok(mut notes) = NOTES.lock() {
        notes.push(message);
    }
}

/// 取走这次读取攒下的提示（只返回一次）。
pub fn take_notes() -> Vec<String> {
    NOTES
        .lock()
        .map(|mut notes| std::mem::take(&mut *notes))
        .unwrap_or_default()
}

/// 字符串枚举的宽容反序列化：不认识的值落到默认分支，而不是让整份配置解析失败。
///
/// 动机很实际：旧版本写下的值、被手改坏的值、或者从新版本降级回来的值，都不
/// 应该让用户的其余配置一起失效。
macro_rules! lenient_enum {
    ($ty:ty, $default:expr, { $($text:literal => $variant:path),+ $(,)? }) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let raw = String::deserialize(deserializer)?;
                Ok(match raw.as_str() {
                    $($text => $variant,)+
                    other => {
                        note(format!("config: 不认识的取值 {other:?}，已按默认值处理"));
                        $default
                    }
                })
            }
        }
    };
}
/// Directory name under `%APPDATA%`. Finalised together with the GitHub owner.
pub const APP_DIR: &str = "io.github.hexbench";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub enabled: bool,
    pub trigger: TriggerSettings,
    pub guard: GuardSettings,
    pub hotkeys: HotkeySettings,
    pub phrases: PhraseSettings,
    pub launcher: LauncherSettings,
    pub sync: SyncSettings,
    pub updates: UpdateSettings,
    pub ui: UiSettings,
    pub meta: MetaSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            trigger: TriggerSettings::default(),
            guard: GuardSettings::default(),
            hotkeys: HotkeySettings::default(),
            phrases: PhraseSettings::default(),
            launcher: LauncherSettings::default(),
            sync: SyncSettings::default(),
            updates: UpdateSettings::default(),
            ui: UiSettings::default(),
            meta: MetaSettings::default(),
        }
    }
}

impl Config {
    /// The subset the geometry needs.
    pub fn geometry(&self) -> TriggerConfig {
        TriggerConfig {
            ratio: self.trigger.ratio,
            position: self.trigger.position,
            enable_left: self.trigger.sides.includes(Side::Left),
            enable_right: self.trigger.sides.includes(Side::Right),
        }
    }
}

/// Every global shortcut lives here, not inside the tool that uses it: tools
/// come and go, the shortcut namespace must stay in one place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HotkeySettings {
    pub pause: String,
    pub phrases_panel: String,
    /// 呼出「面板」（快捷启动 + 文件区）的快捷键。
    pub launcher: String,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            pause: "Ctrl+Alt+Shift+P".to_string(),
            phrases_panel: "Ctrl+Alt+Shift+Q".to_string(),
            launcher: "Alt+1".to_string(),
        }
    }
}

/// WebDAV config sync. The password is stored as a DPAPI blob in base64 and is
/// never uploaded (it would be useless on another machine anyway).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncSettings {
    pub enabled: bool,
    pub url: String,
    pub username: String,
    pub password: String,
    pub last_push: i64,
    pub last_pull: i64,
}

/// Where the updater looks for `latest.json`. Empty means "use the endpoint
/// compiled into the build"; setting it lets users pick a mirror without
/// rebuilding (GitHub downloads are slow in some networks).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateSettings {
    pub endpoint: String,
    /// Version the user dismissed from the sidebar hint; a newer release
    /// clears it by simply being a different string.
    pub dismissed_version: String,
}


/// 面板里的一个网址。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LauncherSite {
    pub name: String,
    pub url: String,
}

/// 呼出面板的文件区：一个被映射的文件夹，显示在面板里。
/// 面板上次停留的位置。只在 `remember_location` 打开时使用。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LauncherLocation {
    /// `all` 或 `folder`
    pub kind: String,
    /// folder 时的根目录（分类目录或常用文件夹）
    pub base: String,
    pub label: String,
    pub path: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LauncherSettings {
    /// 呼出面板的开关。
    pub enabled: bool,
    /// 常用文件夹：面板左侧可以直接跳过去，顺序就是这里的顺序。
    pub folders: Vec<String>,
    /// 启动器的根目录：它的每个子文件夹就是一个分类。
    pub root: String,
    /// 分类的显示顺序（按子文件夹名）；不在这里的分类排在后面。
    pub category_order: Vec<String>,
    /// 面板里的「网址」分组：可以在面板里直接打开这些站点。
    pub sites: Vec<LauncherSite>,
    /// 内置网页总开关（关掉就只有文件夹，不创建任何 WebView）。
    pub web_enabled: bool,
    /// 面板收起后网页保留多少秒（0 = 立即关闭）。
    pub site_keep_alive_secs: u32,
    /// 网页窗口宽度，占工作区百分比。
    pub site_width_percent: u32,
    /// 网页窗口高度，占工作区百分比。
    pub site_height_percent: u32,
    /// 网页窗口是否跟着呼出面板走（默认 false：固定贴在屏幕一侧）。
    pub site_follow_panel: bool,
    /// 固定位置时贴哪一侧："left" 或 "right"。
    pub site_side: String,
    /// 在面板里隐藏的项：分类名，或 `__all__` 表示「全部」那一项。
    /// 默认只列分类，不要「全部」——启动器里它太干扰。
    pub hidden_categories: Vec<String>,
    /// 旧版本只有一个「中转站」文件夹，读旧配置时搬进 `folders`。
    #[serde(default, skip_serializing, rename = "folder")]
    pub legacy_folder: String,
    /// 呼出时贴着鼠标位置，而不是固定在屏幕中央。
    pub follow_cursor: bool,
    /// 面板宽度，占屏幕工作区的百分比。
    pub width_percent: u32,
    /// 面板高度，占屏幕工作区的百分比。
    pub height_percent: u32,
    /// 视图：grid（图标）或 list（列表）。
    pub view: String,
    /// 排序字段：name / modified / size / type。
    pub sort_key: String,
    /// 排序方向：1 升序，-1 降序。
    pub sort_dir: i32,
    /// 呼出时回到上次浏览的位置。
    pub remember_location: bool,
    /// 上次浏览的位置（`remember_location` 关掉时忽略）。
    pub last_location: LauncherLocation,
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            folders: Vec::new(),
            root: String::new(),
            category_order: Vec::new(),
            hidden_categories: vec!["__all__".to_string()],
            sites: Vec::new(),
            web_enabled: true,
            // 默认留 90 秒：够把一段 AI 回答说完，又不至于一直占着内存。
            site_keep_alive_secs: 90,
            site_width_percent: 24,
            site_height_percent: 85,
            site_follow_panel: false,
            site_side: "right".to_string(),
            legacy_folder: String::new(),
            // 默认就是「贴着鼠标 + 面板占一半」，和产品设定一致。
            follow_cursor: true,
            width_percent: 50,
            height_percent: 50,
            view: "grid".to_string(),
            sort_key: "name".to_string(),
            sort_dir: 1,
            remember_location: true,
            last_location: LauncherLocation::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PhraseSettings {
    /// The panel is a tool of its own; this is its master switch.
    pub enabled: bool,
    pub items: Vec<Phrase>,
    /// Executable name of the app the panel docks to; empty means "follow any
    /// foreground app that is not HexBench itself".
    pub target: String,
    /// Extra matchers captured by the picker. A window class survives app
    /// renames, a title substring survives class changes; both are optional.
    pub target_class: String,
    pub target_title: String,
    pub side: DockSide,
    /// Dock inside the target window instead of beside it.
    pub inside: bool,
    /// Last position the user dragged the panel to (physical pixels).
    pub panel_x: Option<i32>,
    pub panel_y: Option<i32>,
    pub collapsed: bool,
    /// Clicking a phrase only fills the input by default; auto-sending a wrong
    /// message to a customer is far worse than one extra Enter.
    pub send_on_click: bool,
    /// Which key submits the message after a phrase is filled.
    pub send_key: SendKey,
}

/// Chat apps disagree: some send on Enter, others on Ctrl+Enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SendKey {
    #[default]
    Enter,
    CtrlEnter,
}

lenient_enum!(SendKey, SendKey::Enter, {
    "enter" => SendKey::Enter,
    "ctrlEnter" => SendKey::CtrlEnter,
});

impl Default for PhraseSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            items: vec![
                Phrase::new("亲，在的，有什么可以帮您～"),
                Phrase::new("稍等，我帮您查一下。"),
                Phrase::new("已经为您处理好了，还有其他问题吗？"),
            ],
            target: String::new(),
            target_class: String::new(),
            target_title: String::new(),
            side: DockSide::Right,
            inside: false,
            panel_x: None,
            panel_y: None,
            collapsed: false,
            send_on_click: false,
            send_key: SendKey::Enter,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Phrase {
    pub id: String,
    pub title: String,
    pub text: String,
}

impl Phrase {
    fn new(text: &str) -> Self {
        let title: String = text.chars().take(10).collect();
        Self {
            id: format!("p{}", text.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32))),
            title,
            text: text.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DockSide {
    Left,
    Right,
}

lenient_enum!(DockSide, DockSide::Right, {
    "left" => DockSide::Left,
    "right" => DockSide::Right,
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TriggerSettings {
    pub ratio: f64,
    pub position: BandPosition,
    pub sides: Sides,
    pub dwell_ms: u64,
    pub cooldown_ms: u64,
    pub poll_ms: u64,
    pub restore_cursor: bool,
    pub restore_offset_px: i32,
}

impl Default for TriggerSettings {
    fn default() -> Self {
        Self {
            ratio: 0.10,
            position: BandPosition::Bottom,
            sides: Sides::Both,
            dwell_ms: 120,
            cooldown_ms: 500,
            poll_ms: 50,
            restore_cursor: true,
            restore_offset_px: 24,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Sides {
    Both,
    Left,
    Right,
}

lenient_enum!(Sides, Sides::Both, {
    "both" => Sides::Both,
    "left" => Sides::Left,
    "right" => Sides::Right,
});

impl Sides {
    pub fn includes(self, side: Side) -> bool {
        match self {
            Sides::Both => true,
            Sides::Left => side == Side::Left,
            Sides::Right => side == Side::Right,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GuardSettings {
    pub skip_when_button_down: bool,

    pub exclude_taskbar: bool,
    pub skip_when_locked: bool,
    pub process_blacklist: Vec<String>,
    pub pause_hotkey: String,
}

impl Default for GuardSettings {
    fn default() -> Self {
        Self {
            skip_when_button_down: true,

            exclude_taskbar: true,
            skip_when_locked: true,
            process_blacklist: Vec::new(),
            pause_hotkey: "Ctrl+Alt+Shift+P".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiSettings {
    pub theme: Theme,
    pub language: String,
    pub onboarded: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self { theme: Theme::System, language: "auto".to_string(), onboarded: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

lenient_enum!(Theme, Theme::System, {
    "system" => Theme::System,
    "light" => Theme::Light,
    "dark" => Theme::Dark,
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MetaSettings {
    pub schema: u32,
    pub updated_at: i64,
    pub device_id: String,
}

impl Default for MetaSettings {
    fn default() -> Self {
        Self { schema: 1, updated_at: 0, device_id: String::new() }
    }
}

/// `%APPDATA%\io.github.hexbench\config.json`
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|appdata| PathBuf::from(appdata).join(APP_DIR).join("config.json"))
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadSource {
    Main,
    Backup,
    Default,
}

/// 配置结构的版本号。
///
/// 加字段**不用**改它：每个结构体都是 `#[serde(default)]`，老配置缺的字段会
/// 自动落到默认值。需要 +1 的只有三种情况：字段改名、删字段、改枚举的取值。
pub const CURRENT_SCHEMA: u32 = 2;

fn parse(path: &Path) -> Option<Config> {
    let bytes = fs::read(path).ok()?;
    match serde_json::from_slice::<Config>(&bytes) {
        Ok(config) => Some(config),
        Err(error) => {
            note(format!("config: 解析 {} 失败：{error}", path.display()));
            None
        }
    }
}

/// 老配置升级。
///
/// 加字段不在这里处理（serde 的 default 已经覆盖），只搬「改名 / 删除 / 语义
/// 变了」这类真正需要人工迁移的东西。将来加一条就用 `if schema < N`。
fn migrate(config: &mut Config) {
    if config.meta.schema > CURRENT_SCHEMA {
        note(format!(
            "config: 这份配置来自更新的版本（schema {} > {}），按当前能理解的部分使用",
            config.meta.schema, CURRENT_SCHEMA
        ));
        return;
    }
    while config.meta.schema < CURRENT_SCHEMA {
        config.meta.schema += 1;
    }
    // schema 1 → 2：中转站的单个文件夹变成「常用文件夹」列表。
    if config.launcher.folders.is_empty() && !config.launcher.legacy_folder.trim().is_empty() {
        config
            .launcher
            .folders
            .push(config.launcher.legacy_folder.clone());
    }
    config.launcher.legacy_folder.clear();
}

/// 解析不了的文件不删也不覆盖，改名留档，免得用户以为配置丢了。
fn quarantine(path: &Path) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default();
    let target = path.with_extension(format!("broken-{stamp}.json"));
    if fs::copy(path, &target).is_ok() {
        note(format!(
            "config: 原文件已留档到 {}，本次用默认配置启动",
            target.display()
        ));
    }
}

/// Load the config, falling back to the backup and then to defaults.
pub fn load(path: &Path) -> (Config, LoadSource) {
    if let Some(mut config) = parse(path) {
        migrate(&mut config);
        return (config, LoadSource::Main);
    }
    let backup = backup_path(path);
    if let Some(mut config) = parse(&backup) {
        note("config: 主文件不可用，已回退到备份".to_string());
        migrate(&mut config);
        return (config, LoadSource::Backup);
    }
    if path.exists() {
        quarantine(path);
    }
    (Config::default(), LoadSource::Default)
}

/// Write via a temporary file and `rename`, keeping the previous file as `.bak`.
pub fn save(path: &Path, config: &Config) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;

    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&data)?;
        file.sync_all()?;
    }
    if path.exists() {
        let _ = fs::copy(path, backup_path(path));
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hexbench-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let config: Config = serde_json::from_str(r#"{"enabled": false}"#).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.trigger.ratio, 0.10);
        assert_eq!(config.trigger.dwell_ms, 120);
        assert_eq!(config.trigger.sides, Sides::Both);
    }

    #[test]
    fn round_trip_through_disk() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("config.json");
        let mut config = Config::default();
        config.trigger.ratio = 0.25;
        config.trigger.position = BandPosition::Top;
        config.trigger.sides = Sides::Right;
        config.guard.process_blacklist = vec!["game.exe".into()];

        save(&path, &config).unwrap();
        let (loaded, source) = load(&path);
        assert_eq!(source, LoadSource::Main);
        assert_eq!(loaded.trigger.ratio, 0.25);
        assert_eq!(loaded.trigger.position, BandPosition::Top);
        assert_eq!(loaded.trigger.sides, Sides::Right);
        assert_eq!(loaded.guard.process_blacklist, vec!["game.exe".to_string()]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_main_falls_back_to_backup() {
        let dir = temp_dir("backup");
        let path = dir.join("config.json");
        save(&path, &Config::default()).unwrap();
        // Second save creates the .bak from the first file.
        save(&path, &Config::default()).unwrap();
        fs::write(&path, b"{ not json").unwrap();

        let (_config, source) = load(&path);
        assert_eq!(source, LoadSource::Backup);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_uses_defaults() {
        let dir = temp_dir("missing");
        let path = dir.join("nope").join("config.json");
        let (config, source) = load(&path);
        assert_eq!(source, LoadSource::Default);
        assert!(config.enabled);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_enum_value_keeps_the_rest_of_the_config() {
        // 旧版本写下的值、手改坏的值，都不该让整份配置一起失效。
        let json = r#"{
            "enabled": false,
            "ui": { "theme": "neon" },
            "trigger": { "position": "middle", "ratio": 0.25 },
            "phrases": { "sendKey": "ctrl-enter-x" }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.enabled, "其余字段要保住");
        assert_eq!(config.ui.theme, Theme::System, "未知主题落回默认");
        assert_eq!(config.trigger.position, BandPosition::Bottom);
        assert_eq!(config.trigger.ratio, 0.25);
        assert_eq!(config.phrases.send_key, SendKey::Enter);
    }

    #[test]
    fn unknown_fields_from_a_newer_version_are_ignored() {
        let json = r#"{ "enabled": false, "clipboardHistory": { "enabled": true } }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.enabled);
        assert_eq!(
            config.launcher.width_percent,
            LauncherSettings::default().width_percent,
            "缺的字段用默认值"
        );
    }

    #[test]
    fn unparsable_file_is_quarantined() {
        let dir = temp_dir("quarantine");
        let path = dir.join("config.json");
        fs::write(&path, b"{ this is not json").unwrap();

        let (config, source) = load(&path);
        assert_eq!(source, LoadSource::Default);
        assert!(config.enabled);

        let kept = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains("broken"));
        assert!(kept, "坏掉的配置要留档，不能直接覆盖");

        let _ = fs::remove_dir_all(&dir);
    }
}
