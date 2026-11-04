//! 桌面格子的原生渲染层：Direct2D 绘制 + DirectComposition 合成。
//!
//! 只做渲染与窗口本身（悬停、滚轮、标题条拖动、不抢焦点的桌面层窗口），
//! 配置、目录枚举、图标缓存都留在 `hexbench-core` / `src-tauri` 里，通过
//! [`Frame`] 推过来。这样灰度切换时两条路径看到的是同一份数据。
//!
//! 和 `proto/fence-native` 的区别：这里是一个长期存活的宿主——一条自己的
//! 消息循环线程、一份 GPU 设备、每格一个合成 surface，命令从任意线程投递。

#[cfg(not(windows))]
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Item {
    /// 已经按网页版的 `displayName` 处理过（快捷方式不带 .lnk / .url）。
    pub name: String,
    /// 文件真实路径；图标按它从 `hexbench_core::icons` 的缓存里取。
    pub path: String,
    pub is_dir: bool,
    /// 修改时间（Unix 秒）；0 = 读不到。
    pub modified: i64,
    /// 文件大小（字节）；文件夹是 0，列表里留空。
    pub size: u64,
}

/// 面包屑的一段：从绑定目录往下，一层一个。
#[derive(Clone, Debug)]
pub struct Crumb {
    /// 显示名（子文件夹名）。
    pub name: String,
    /// 这一层的完整路径。
    pub path: String,
}

/// 一次完整的内容推送。`generation` 单调递增：旧的图标批次晚到时直接丢掉。
#[derive(Clone, Debug)]
pub struct Frame {
    pub id: String,
    pub generation: u64,
    pub title: String,
    /// 当前显示的目录：绑定目录本身，或双击子文件夹后进去的那一层。
    /// 空白处右键菜单、重读目录都用它（空 = 没有可操作的目录）。
    pub path: String,
    /// 格子绑定的根目录；导航不能越出这里，面包屑也从它开始。
    pub root: String,
    /// 从绑定目录往下到当前目录的每一层（不含根）；在根目录时为空。
    pub crumbs: Vec<Crumb>,
    /// 物理像素外框 `[left, top, width, height]`。
    pub rect: [i32; 4],
    pub rolled: bool,
    pub zoom: u32,
    /// 卡片不透明度百分比（30..=100）。
    pub opacity: u32,
    /// 背后要不要磨砂（DWM 亚克力）：关掉就是「透而不糊」。
    pub frosted: bool,
    pub dark: bool,
    pub items: Vec<Item>,
    /// 浏览历史里还能往回 / 往前走的层数：没有可去的地方时方向提示画淡一点。
    pub back_count: usize,
    pub forward_count: usize,
    /// 收纳盒（幕后目录）——标题菜单据此决定「更换文件夹 / 立即整理」。
    pub is_bucket: bool,
    pub rule_count: usize,
    /// 当前排序，菜单里标方向。
    pub sort_key: String,
    pub sort_dir: i32,
    /// 锁定位置：拖不动标题条、拉不了边角。
    pub locked: bool,
    /// 列表视图（一行一个）；`false` 就是图标网格。
    pub list: bool,
}

#[derive(Clone, Debug)]
pub enum Event {
    /// 标题条拖动松手：物理矩形 `[left, top, width, height]`，由上层换算成
    /// 归一化坐标落盘。
    Moved { id: String, rect: [i32; 4] },
    /// 标题条双击：卷起 / 展开。由上层写回配置，再推一帧新的回来。
    Rolled { id: String, rolled: bool },
    /// 标题菜单「按名称 / 按修改时间排序」：同键则反转方向。
    Sort { id: String, key: String },
    /// 标题菜单「锁定位置 / 解锁位置」。
    Lock { id: String, locked: bool },
    /// 标题菜单「列表视图 / 图标视图」切换。
    View { id: String, view: String },
    /// 标题菜单「隐藏格子」。
    Hide { id: String },
    /// 标题菜单「删除格子」。
    Delete { id: String },
    /// 右键菜单关掉后请求重读这一格（新建 / 粘贴 / 删除都可能改了目录）。
    Refresh { id: String },
    /// 双击子文件夹：在格子里进入这一层（不打开资源管理器）。
    Navigate { id: String, path: String },
    /// 右键手势往回划：退回上一段浏览历史。
    Back { id: String },
    /// 右键手势往前划：回到刚才退出去的那一层。
    Forward { id: String },
    /// 开始拖标题：吸附、参考线、落盘都交给后端那套循环（和网页版共用）。
    BeginMove {
        id: String,
        hwnd: isize,
        scale: f32,
    },
    /// 开始拉边 / 拉角缩放；`edge` 是 `n` / `sw` 这类方向。
    BeginResize {
        id: String,
        hwnd: isize,
        edge: String,
        scale: f32,
    },
    /// 标题菜单「更换文件夹…」：后端弹目录选择。
    ChangeFolder { id: String },
    /// 标题菜单「立即整理」：收纳盒按规则收一次。
    RunRules { id: String },
    /// 标题菜单「重命名格子」：用户在原生输入框里改完了标题。
    Rename { id: String, title: String },
    /// 条目的右键菜单「重命名」：用户在原生输入框里改完了文件名。
    RenameItem {
        id: String,
        path: String,
        name: String,
    },
    /// 外部拖进来的文件松手了：`dest` 是落点（悬停的文件夹，或格子当前目录）。
    Drop {
        id: String,
        dest: String,
        paths: Vec<String>,
        /// 按住 Ctrl 拖进来 = 复制；否则移动。
        copy: bool,
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Bench {
    pub frames: usize,
    pub avg_us: f64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub max_us: u64,
}

pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::NativeFences;

#[cfg(not(windows))]
pub struct NativeFences;

#[cfg(not(windows))]
impl NativeFences {
    pub fn start(_sink: EventSink) -> Result<Arc<Self>, String> {
        Err("原生格子只在 Windows 上可用".to_string())
    }

    pub fn sync(&self, _frames: Vec<Frame>) {}

    pub fn upsert(&self, _frame: Frame) {}

    pub fn set_icons(&self, _id: &str, _generation: u64, _icons: Vec<(String, Option<PathBuf>)>) {}

    pub fn set_gestures(&self, _gestures: hexbench_core::config::GestureSettings) {}

    pub fn toast(&self, _id: &str, _text: &str) {}

    pub fn shutdown(&self) {}

    pub fn hwnds(&self) -> Vec<isize> {
        Vec::new()
    }

    pub fn hwnd_map(&self) -> Vec<(String, isize)> {
        Vec::new()
    }

    pub fn bench(&self, _id: &str, _frames: usize) -> Option<Bench> {
        None
    }

    pub fn drop_target(&self, _id: &str, _x: f32, _y: f32) -> Option<String> {
        None
    }
}
