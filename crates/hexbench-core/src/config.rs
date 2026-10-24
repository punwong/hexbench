//! Configuration: one JSON file, atomic writes, unknown fields preserved.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{BandPosition, Side, TriggerConfig};

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
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            pause: "Ctrl+Alt+Shift+P".to_string(),
            phrases_panel: "Ctrl+Alt+Shift+Q".to_string(),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PhraseSettings {
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
}

impl Default for PhraseSettings {
    fn default() -> Self {
        Self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockSide {
    Left,
    Right,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sides {
    Both,
    Left,
    Right,
}

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
    pub skip_fullscreen: bool,
    pub exclude_taskbar: bool,
    pub skip_when_locked: bool,
    pub process_blacklist: Vec<String>,
    pub pause_hotkey: String,
}

impl Default for GuardSettings {
    fn default() -> Self {
        Self {
            skip_when_button_down: true,
            skip_fullscreen: true,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

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

/// Load the config, falling back to the backup and then to defaults.
pub fn load(path: &Path) -> (Config, LoadSource) {
    if let Ok(bytes) = fs::read(path) {
        if let Ok(config) = serde_json::from_slice::<Config>(&bytes) {
            return (config, LoadSource::Main);
        }
    }
    let backup = backup_path(path);
    if let Ok(bytes) = fs::read(&backup) {
        if let Ok(config) = serde_json::from_slice::<Config>(&bytes) {
            return (config, LoadSource::Backup);
        }
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
}
