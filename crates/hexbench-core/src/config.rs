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
    pub launcher: LauncherSettings,
    /// 收藏（笔记）：呼出面板里的 Markdown 笔记和图片。
    pub notes: NotesSettings,
    /// 桌面格子：桌面上的文件夹小组件。
    pub desktop: DesktopSettings,
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
            launcher: LauncherSettings::default(),
            notes: NotesSettings::default(),
            desktop: DesktopSettings::default(),
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
    /// 呼出「快捷面板」的快捷键。
    pub launcher: String,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            pause: "Ctrl+Alt+Shift+P".to_string(),
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
    /// 这个 base 是「快速访问」里的文件夹（而不是启动分类）。
    /// 同一个文件夹可能两边都有，回落时要能分出来是哪一行。
    pub pinned: bool,
}
/// 面板文件区的鼠标手势：按住右键左右滑动 = 后退 / 前进。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GestureSettings {
    /// 手势总开关。
    pub enabled: bool,
    /// 左右互换：true 时向左是前进、向右是后退。
    pub invert_direction: bool,
    /// 横向滑多远才执行（逻辑像素）。
    pub trigger_distance: u32,
    /// 拖动时是否画轨迹线、在光标旁显示方向。
    pub show_trail: bool,
}

impl Default for GestureSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            invert_direction: false,
            // 80px 大约是小半个面板宽：够区分「手滑了一下」和真的在划。
            trigger_distance: 80,
            show_trail: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LauncherSettings {
    /// 呼出面板的开关。
    pub enabled: bool,
    /// 常用文件夹：面板左侧可以直接跳过去，顺序就是这里的顺序。
    pub folders: Vec<String>,
    /// 分类的根目录：它的每个子文件夹就是一个分类。
    pub root: String,
    /// 分类的显示顺序（按子文件夹名）；不在这里的分类排在后面。
    pub category_order: Vec<String>,
    /// 内置网页的站点列表：网页窗口顶栏就是它们。
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
    /// 是否记住拖动出来的位置和大小。
    pub site_remember_position: bool,
    /// 用户拖动出来的位置和大小（逻辑像素）。0 表示没拖过。
    pub site_x: i32,
    pub site_y: i32,
    pub site_w: u32,
    pub site_h: u32,
    /// 保存这份几何时用的百分比：设置里改了百分比就作废，重新按百分比算。
    pub site_pct_w: u32,
    pub site_pct_h: u32,
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
    /// 是否记住拖出来的面板大小。
    pub panel_remember_size: bool,
    /// 拖出来的面板尺寸（逻辑像素），0 表示没拖过。
    pub panel_w: u32,
    pub panel_h: u32,
    /// 保存这份尺寸时用的百分比，改百分比就作废。
    pub panel_pct_w: u32,
    pub panel_pct_h: u32,
    /// 视图：grid（图标）或 list（列表）。这是「快速启动」（分类）那一套。
    pub view: String,
    /// 排序字段：name / modified / size / type。
    pub sort_key: String,
    /// 排序方向：1 升序，-1 降序。
    pub sort_dir: i32,
    /// 「快速访问」（钉住的文件夹）单独记的视图 / 排序。
    /// 空 / 0 表示还没单独设过，跟着分类那一套走。
    pub pinned_view: String,
    pub pinned_sort_key: String,
    pub pinned_sort_dir: i32,
    /// 文件区的排版缩放百分比（Alt+滚轮）：100 是默认也是最小，最多放到 150。
    pub view_zoom: u32,
    /// 快速访问单独记的缩放；0 表示还没单独设过，跟着分类那一套走。
    pub pinned_view_zoom: u32,
    /// 呼出时回到上次浏览的位置。
    pub remember_location: bool,
    /// 上次浏览的位置（`remember_location` 关掉时忽略）。
    pub last_location: LauncherLocation,
    /// 文件区右键滑动的手势设置。
    pub gestures: GestureSettings,
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
            site_remember_position: true,
            site_x: 0,
            site_y: 0,
            site_w: 0,
            site_h: 0,
            site_pct_w: 0,
            site_pct_h: 0,
            legacy_folder: String::new(),
            // 默认就是「贴着鼠标 + 面板占六成」，和产品设定一致。
            follow_cursor: true,
            width_percent: 60,
            panel_remember_size: true,
            panel_w: 0,
            panel_h: 0,
            panel_pct_w: 0,
            panel_pct_h: 0,
            height_percent: 60,
            view: "grid".to_string(),
            sort_key: "name".to_string(),
            sort_dir: 1,
            pinned_view: String::new(),
            pinned_sort_key: String::new(),
            pinned_sort_dir: 0,
            view_zoom: 100,
            pinned_view_zoom: 0,
            remember_location: true,
            last_location: LauncherLocation::default(),
            gestures: GestureSettings::default(),
        }
    }
}

/// 桌面格子：贴着桌面层显示、一格绑定一个文件夹的小组件。
///
/// 窗口和文件操作由 `src-tauri` 负责；这里只放「跨设备怎么记住」的数据。
/// 几何同时保存「工作区内的比例」和「上次的物理矩形」：显示器分辨率 / 缩放 /
/// 任务栏变化时按比例恢复，显示器不在了就用物理值夹回最近的工作区。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DesktopSettings {
    /// 总开关；关闭时窗口全部销毁，配置保留。
    pub enabled: bool,
    /// Win+D / 显示桌面之后是否继续显示格子。
    pub keep_on_show_desktop: bool,
    /// 移动 / 缩放时吸附屏幕边缘和其他格子。
    pub snap: bool,
    /// 额外的 8px 网格吸附（逻辑像素）；只影响位置。
    pub snap_grid: bool,
    /// 在桌面右键菜单里挂一个 HexBench 子菜单（默认开）。
    ///
    /// 这是**本机**的事（要写注册表），同步时以本机为准。设置页没有这个开关；
    /// 真想去掉，把它改成 `false` 再重启应用，启动时会把自己那一项删掉。
    pub context_menu: bool,
    /// 卡片不透明度（百分比 30..=100）。越小背后透得越多，字也越难认。
    pub opacity: u32,
    /// 背后要不要磨砂。关掉就是「透而不糊」：壁纸原样从卡片底下透出来。
    pub frosted: bool,
    /// Beta：格子改由原生渲染层（Direct2D + DirectComposition）绘制。
    /// 关掉立刻回到 WebView 实现，两条路径共用同一份配置和目录枚举。
    pub native: bool,
    pub fences: Vec<FenceSettings>,
}

/// 不透明度的下限：再低文字就和壁纸糊在一起了。
pub const DESKTOP_OPACITY_MIN: u32 = 30;

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            keep_on_show_desktop: true,
            snap: true,
            snap_grid: false,
            context_menu: true,
            // 70% 是照壁纸调出来的：磨砂看得出来，字也清楚。
            opacity: 70,
            frosted: true,
            native: false,
            fences: Vec::new(),
        }
    }
}

impl DesktopSettings {
    /// 旧配置里的 0 / 手改出来的 999 都夹回可用范围。
    pub fn clamp_opacity(&mut self) {
        self.opacity = self.opacity.clamp(DESKTOP_OPACITY_MIN, 100);
    }
}

/// 格子是怎么来的。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FenceKind {
    /// 绑定用户自己选的一个文件夹：盒子里的东西就是那个文件夹里的东西。
    #[default]
    Folder,
    /// 收纳盒：`path` 是应用自己建的幕后目录。拖进来的文件真的搬进去，
    /// 用户看不到「它其实是个文件夹」；带规则时由规则自动往里收。
    Bucket,
}

impl FenceKind {
    pub fn is_bucket(self) -> bool {
        matches!(self, FenceKind::Bucket)
    }
}

/// 整理规则：把源目录里符合条件的文件收进收纳盒（可指定子目录）。
///
/// 空的字段一律表示「不限」。规则按顺序匹配，先命中的那条决定去向。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FenceRule {
    /// 稳定 id（设置页用来增删；空的话保存时补）。
    pub id: String,
    /// 规则名；也用作「收进这个子目录」时的默认目录名。
    pub name: String,
    pub enabled: bool,
    /// 扩展名白名单（小写、不带点）；空 = 不限。
    pub exts: Vec<String>,
    /// 文件名里要包含的一段文字（不区分大小写）；空 = 不限。
    pub name_contains: String,
    /// 大小下限 / 上限（MB，0 = 不限）。
    pub min_mb: u64,
    pub max_mb: u64,
    /// 只收「最后修改早于 N 天」的文件（0 = 不限）。
    pub older_than_days: u32,
    /// 监视的来源目录（通常填桌面 / 下载）。
    pub sources: Vec<String>,
    /// 收进本盒的哪个子目录；空 = 直接放盒子里。
    pub subdir: String,
}

impl Default for FenceRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            exts: Vec::new(),
            name_contains: String::new(),
            min_mb: 0,
            max_mb: 0,
            older_than_days: 0,
            sources: Vec::new(),
            subdir: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FenceSettings {
    /// 稳定 id：窗口 label 是 `fence-<id>`，配置合并按它配对。
    pub id: String,
    /// 标题条上显示的名字；空 = 用文件夹名。
    pub title: String,
    /// 绑定（以及当前浏览）的根文件夹；收纳盒里是幕后目录。
    pub path: String,
    /// 绑定文件夹，还是应用自己管的收纳盒。
    pub kind: FenceKind,
    /// 整理规则（只有收纳盒用得上）。
    pub rules: Vec<FenceRule>,
    /// 源目录一有变化就自动按规则整理；关掉只能手动「整理现在」。
    pub rules_auto: bool,
    /// 所属显示器，如 `\\.\DISPLAY1`。
    pub monitor: String,
    /// 工作区内的归一化矩形（0..1）。
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// 上次的物理矩形（像素），显示器找不到时兜底。
    pub pixel_x: i32,
    pub pixel_y: i32,
    pub pixel_w: i32,
    pub pixel_h: i32,
    /// 卷起：只留标题条；`h` 始终是展开高度。
    pub rolled: bool,
    pub visible: bool,
    /// 排版缩放百分比，和快捷面板同一档位（100 默认 / 最小，最多 150）。
    pub zoom: u32,
    pub sort_key: String,
    pub sort_dir: i32,
}

impl Default for FenceSettings {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            path: String::new(),
            kind: FenceKind::Folder,
            rules: Vec::new(),
            rules_auto: true,
            monitor: String::new(),
            x: 0.08,
            y: 0.08,
            w: 0.32,
            h: 0.42,
            pixel_x: 0,
            pixel_y: 0,
            pixel_w: 0,
            pixel_h: 0,
            rolled: false,
            visible: true,
            zoom: 100,
            sort_key: "name".to_string(),
            sort_dir: 1,
        }
    }
}

impl FenceSettings {
    /// 收纳盒（幕后目录是应用自己建的）。
    pub fn is_bucket(&self) -> bool {
        self.kind.is_bucket()
    }

    /// 和快捷面板的后端规则一致：非法 / 过小的值按 100，最多 150。
    pub fn clamp_zoom(&mut self) {
        self.zoom = if (100..=150).contains(&self.zoom) { self.zoom } else { 100 };
    }

    /// 手改配置 / 旧版本留下的规则值收拾干净：扩展名统一成小写不带点、
    /// 上限不能比下限还小、子目录名不能是路径。
    pub fn clamp_rules(&mut self) {
        self.rules.truncate(24);
        for rule in &mut self.rules {
            rule.exts = rule
                .exts
                .iter()
                .map(|ext| ext.trim().trim_start_matches('.').to_lowercase())
                .filter(|ext| !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()))
                .collect();
            rule.exts.sort();
            rule.exts.dedup();
            rule.older_than_days = rule.older_than_days.min(3650);
            if rule.max_mb > 0 && rule.max_mb < rule.min_mb {
                rule.max_mb = rule.min_mb;
            }
            rule.sources.retain(|source| !source.trim().is_empty());
            rule.subdir = rule.subdir.trim().to_string();
            if rule.subdir.contains(['\\', '/']) || rule.subdir.starts_with('.') {
                rule.subdir.clear();
            }
        }
    }

    /// 归一化矩形是否可用（手改配置 / 旧版本可能留下坏值）。
    pub fn has_valid_geometry(&self) -> bool {
        let finite = [self.x, self.y, self.w, self.h].iter().all(|value| value.is_finite());
        finite && self.w > 0.02 && self.h > 0.02 && self.x >= -0.5 && self.y >= -0.5
    }
}

/// 保存设置窗口传来的整份配置时，桌面段以内存为准。
///
/// 设置页从不直接改 `fences`（新增 / 删除 / 改标题都走专属命令），它手里那份
/// 快照可能已经旧了；如果整段照收，会把刚拖好的位置和刚删掉的格子带回来。
pub fn merge_desktop_from_settings(incoming: &mut DesktopSettings, live: &DesktopSettings) {
    incoming.fences = live.fences.clone();
}

/// 同步拉取时合并桌面段：格子列表取并集，几何 / 视图等运行时状态留在本机。
///
/// 删除只通过 `fence_delete` 命令发生，所以远端缺一个格子不等于要删它。
pub fn merge_desktop_from_sync(remote: &mut DesktopSettings, live: &DesktopSettings) {
    // 右键菜单是注册表里的东西，只属于本机：远端开着不代表这台机器装过。
    remote.context_menu = live.context_menu;
    remote.clamp_opacity();
    let mut merged = Vec::with_capacity(remote.fences.len() + live.fences.len());
    for fence in &mut remote.fences {
        fence.clamp_zoom();
        fence.clamp_rules();
        if !fence.has_valid_geometry() {
            *fence = FenceSettings {
                id: fence.id.clone(),
                path: fence.path.clone(),
                kind: fence.kind,
                ..FenceSettings::default()
            };
        }
        if let Some(local) = live.fences.iter().find(|local| local.id == fence.id) {
            let title = fence.title.clone();
            let path = fence.path.clone();
            let kind = fence.kind;
            let rules = fence.rules.clone();
            let rules_auto = fence.rules_auto;
            *fence = local.clone();
            // 标题、绑定路径、规则是「定义」，跟随远端；其余是本机运行状态。
            fence.title = title;
            // 收纳盒的幕后目录是本机建的，别的机器上没有这个路径。
            fence.path = if kind.is_bucket() { local.path.clone() } else { path };
            fence.kind = kind;
            fence.rules = rules;
            fence.rules_auto = rules_auto;
        }
        merged.push(fence.clone());
    }
    for local in &live.fences {
        if !merged.iter().any(|fence| fence.id == local.id) {
            merged.push(local.clone());
        }
    }
    remote.fences = merged;
}

/// 收藏（笔记）：呼出面板里的 Markdown 笔记。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NotesSettings {
    pub enabled: bool,
    /// 空 = 默认的「文档\HexBench 收藏」。
    pub folder: String,
    /// 发送后是否再按一次回车（表单输入一般不要，聊天 / 搜索才要）。
    pub send_enter: bool,
}

impl Default for NotesSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            folder: String::new(),
            send_enter: false,
        }
    }
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
pub const CURRENT_SCHEMA: u32 = 3;

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
    // 手改配置或旧版本写进来越界的不透明度，读的时候夹回来（0 会让卡片整个消失）。
    config.desktop.clamp_opacity();
    // 整理规则同理：扩展名写法、大小上下限、子目录名都可能被手改坏。
    for fence in &mut config.desktop.fences {
        fence.clamp_rules();
    }
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
    // schema 2 → 3：去掉「常用语」浮窗，换成呼出面板里的「收藏」。旧配置里的
    // `phrases` 和 `hotkeys.phrasesPanel` 由 serde 直接忽略，下次保存就自然消失；
    // 新加的 `notes` 段缺失时走默认值，不需要搬数据。
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
            "trigger": { "position": "middle", "ratio": 0.25 }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.enabled, "其余字段要保住");
        assert_eq!(config.ui.theme, Theme::System, "未知主题落回默认");
        assert_eq!(config.trigger.position, BandPosition::Bottom);
        assert_eq!(config.trigger.ratio, 0.25);
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
    fn launcher_zoom_defaults_and_camel_case() {
        let defaults = LauncherSettings::default();
        assert_eq!(defaults.view_zoom, 100);
        assert_eq!(defaults.pinned_view_zoom, 0, "0 = 跟着分类那一套");
        let json = serde_json::to_string(&defaults).unwrap();
        assert!(json.contains("\"viewZoom\":100"));
        assert!(json.contains("\"pinnedViewZoom\":0"));
        // 旧配置里没有这两个字段：读出来是默认值，不能把整份配置判坏。
        let old: LauncherSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.view_zoom, 100);
        assert_eq!(old.pinned_view_zoom, 0);
    }

    #[test]
    fn notes_defaults_and_camel_case() {
        let defaults = NotesSettings::default();
        assert!(defaults.enabled);
        assert!(defaults.folder.is_empty(), "空 = 文档下的默认收藏目录");
        assert!(!defaults.send_enter, "默认不替用户按回车");
        let json = serde_json::to_string(&defaults).unwrap();
        assert!(json.contains("\"sendEnter\":false"));
        // 老配置没有 notes 段：整份配置照常读出来，收藏默认可用。
        let config: Config = serde_json::from_str("{}").unwrap();
        assert!(config.notes.enabled);
        assert_eq!(config.notes.folder, "");
    }

    #[test]
    fn desktop_defaults_and_camel_case() {
        let defaults = DesktopSettings::default();
        assert!(!defaults.enabled, "新功能默认关，用户主动打开");
        assert!(defaults.keep_on_show_desktop);
        assert!(defaults.snap);
        assert!(defaults.context_menu, "桌面右键菜单默认装上");
        assert_eq!(defaults.opacity, 70);
        assert!(defaults.frosted, "默认磨砂");
        assert!(defaults.fences.is_empty());

        let json = serde_json::to_string(&defaults).unwrap();
        assert!(json.contains("\"keepOnShowDesktop\":true"));
        assert!(json.contains("\"snapGrid\":false"));
        assert!(json.contains("\"contextMenu\":true"));
        assert!(json.contains("\"opacity\":70"));
        assert!(json.contains("\"frosted\":true"));

        // 旧配置没有 desktop 段：整份配置照常读出来，桌面格子是关着的。
        let config: Config = serde_json::from_str("{}").unwrap();
        assert!(!config.desktop.enabled);
        assert!(config.desktop.fences.is_empty());
    }

    #[test]
    fn desktop_opacity_is_clamped_on_load() {
        let dir = temp_dir("opacity");
        let path = dir.join("config.json");
        fs::write(
            &path,
            br#"{"meta":{"schema":3},"desktop":{"opacity":0,"frosted":false}}"#,
        )
        .unwrap();
        let (config, _) = load(&path);
        assert_eq!(config.desktop.opacity, DESKTOP_OPACITY_MIN);
        assert!(!config.desktop.frosted);

        let mut desktop = DesktopSettings { opacity: 999, ..DesktopSettings::default() };
        desktop.clamp_opacity();
        assert_eq!(desktop.opacity, 100);
    }

    #[test]
    fn fence_zoom_and_geometry_are_sanitised() {
        let mut fence = FenceSettings { zoom: 999, ..FenceSettings::default() };
        fence.clamp_zoom();
        assert_eq!(fence.zoom, 100);
        let mut fence = FenceSettings { zoom: 130, ..FenceSettings::default() };
        fence.clamp_zoom();
        assert_eq!(fence.zoom, 130);

        assert!(FenceSettings::default().has_valid_geometry());
        let broken = FenceSettings { w: 0.0, ..FenceSettings::default() };
        assert!(!broken.has_valid_geometry());
        let nan = FenceSettings { x: f64::NAN, ..FenceSettings::default() };
        assert!(!nan.has_valid_geometry());
    }

    #[test]
    fn fence_kind_and_rules_round_trip() {
        let defaults = FenceSettings::default();
        assert_eq!(defaults.kind, FenceKind::Folder, "旧格子仍是文件夹映射");
        assert!(defaults.rules.is_empty());
        assert!(defaults.rules_auto, "有规则就默认自动整理");
        let json = serde_json::to_string(&defaults).unwrap();
        assert!(json.contains("\"kind\":\"folder\""));
        assert!(json.contains("\"rulesAuto\":true"));

        // 旧配置里没有 kind / rules：读出来就是文件夹映射，不能判坏。
        let old: FenceSettings = serde_json::from_str(r#"{"id":"f1","path":"D:/x"}"#).unwrap();
        assert_eq!(old.kind, FenceKind::Folder);
        assert!(old.rules.is_empty());

        let mut bucket = FenceSettings {
            kind: FenceKind::Bucket,
            rules: vec![FenceRule {
                name: "图片".into(),
                exts: vec!["PNG".into(), ".jpg".into()],
                sources: vec!["C:/Users/me/Desktop".into()],
                subdir: "图片".into(),
                ..FenceRule::default()
            }],
            ..FenceSettings::default()
        };
        bucket.clamp_rules();
        assert_eq!(bucket.rules[0].exts, vec!["jpg", "png"], "扩展名统一小写不带点");
        let text = serde_json::to_string(&bucket).unwrap();
        assert!(text.contains("\"kind\":\"bucket\""));
        assert!(text.contains("\"olderThanDays\""));
        let back: FenceSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back, bucket);
    }

    #[test]
    fn broken_rule_values_are_sanitised() {
        let mut fence = FenceSettings {
            rules: vec![FenceRule {
                exts: vec![" .PNG ".into(), String::new(), "t.x".into()],
                min_mb: 5,
                max_mb: 1,
                subdir: "a/b".into(),
                older_than_days: 99999,
                ..FenceRule::default()
            }],
            ..FenceSettings::default()
        };
        fence.clamp_rules();
        let rule = &fence.rules[0];
        assert_eq!(rule.exts, vec!["png"], "空的和带点的写法都清掉");
        assert_eq!(rule.max_mb, 5, "上限不能比下限小");
        assert_eq!(rule.subdir, "", "子目录名不能是路径");
        assert_eq!(rule.older_than_days, 3650, "天数有上限");
    }

    #[test]
    fn settings_save_cannot_resurrect_or_move_fences() {
        let live = DesktopSettings {
            enabled: true,
            fences: vec![FenceSettings { id: "f1".into(), x: 0.4, ..FenceSettings::default() }],
            ..DesktopSettings::default()
        };
        // 设置窗口打开时的旧快照：没有刚拖好的位置，还留着一个已经删掉的格子。
        let mut incoming = DesktopSettings {
            enabled: false,
            keep_on_show_desktop: false,
            fences: vec![
                FenceSettings { id: "f1".into(), x: 0.1, ..FenceSettings::default() },
                FenceSettings { id: "gone".into(), ..FenceSettings::default() },
            ],
            ..DesktopSettings::default()
        };
        merge_desktop_from_settings(&mut incoming, &live);
        assert!(!incoming.enabled, "开关属于设置页");
        assert!(!incoming.keep_on_show_desktop);
        assert_eq!(incoming.fences.len(), 1);
        assert_eq!(incoming.fences[0].id, "f1");
        assert_eq!(incoming.fences[0].x, 0.4, "位置以内存为准");
    }

    #[test]
    fn sync_merge_keeps_local_geometry() {
        let live = DesktopSettings {
            context_menu: true,
            fences: vec![
                FenceSettings {
                    id: "f1".into(),
                    title: "本机".into(),
                    path: "D:/local".into(),
                    x: 0.3,
                    zoom: 120,
                    ..FenceSettings::default()
                },
                FenceSettings { id: "f2".into(), path: "D:/only-local".into(), ..FenceSettings::default() },
            ],
            ..DesktopSettings::default()
        };
        let mut remote = DesktopSettings {
            fences: vec![
                FenceSettings {
                    id: "f1".into(),
                    title: "远端".into(),
                    path: "E:/remote".into(),
                    x: 0.05,
                    zoom: 60,
                    ..FenceSettings::default()
                },
                FenceSettings { id: "f3".into(), path: "E:/new".into(), ..FenceSettings::default() },
            ],
            ..DesktopSettings::default()
        };
        merge_desktop_from_sync(&mut remote, &live);
        assert!(remote.context_menu, "右键菜单是注册表状态，只认本机");
        let f1 = remote.fences.iter().find(|fence| fence.id == "f1").unwrap();
        assert_eq!(f1.title, "远端");
        assert_eq!(f1.path, "E:/remote");
        assert_eq!(f1.x, 0.3, "几何留在本机");
        assert_eq!(f1.zoom, 120, "缩放留在本机");
        assert!(remote.fences.iter().any(|fence| fence.id == "f2"), "本机独有的格子不能丢");
        assert!(remote.fences.iter().any(|fence| fence.id == "f3"), "远端新增的格子要进来");
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
