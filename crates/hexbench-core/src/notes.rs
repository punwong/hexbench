//! 收藏（笔记）：一个笔记一个 Markdown 文件，图片放同目录的 `images/`。
//!
//! 不写 frontmatter、也不建数据库：**文件名就是标题，正文就是 Markdown**。
//! 这样整个文件夹可以直接用别的编辑器打开、丢进同步盘，换台机器也能接着用。
//! 图片用相对路径引用（`![名字](images/xxx.png)`），面板只认这一种写法——
//! 别的写法原样留在正文里，不会被当成收藏的图片。

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

const FOLDER_NAME: &str = "HexBench 收藏";
const IMAGES_DIR: &str = "images";
const TITLE_FALLBACK: &str = "未命名笔记";
const MAX_TITLE_CHARS: usize = 60;
const MAX_IMAGE_STEM: usize = 40;
const PREVIEW_CHARS: usize = 280;
/// 列目录只要预览：再大的笔记也不该把面板拖慢。
const LIST_READ_CAP: u64 = 256 * 1024;
/// 一张图最多 256 MB；再大大概率不是图片，别把内存吃穿。
const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "ico"];

/// 卡片上的缩略图：`name` 是 `images/` 下的文件名，`path` 是完整路径（取缩略图用）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteImage {
    pub name: String,
    pub path: String,
}

/// 列表里的一条笔记。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteSummary {
    /// 文件名（含 `.md`），编辑 / 删除都用它。
    pub id: String,
    pub title: String,
    pub preview: String,
    pub images: Vec<NoteImage>,
    /// 修改时间（Unix 秒）。
    pub modified: i64,
}

/// 打开编辑器时读的完整内容。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteFile {
    pub id: String,
    pub title: String,
    pub body: String,
    pub modified: i64,
}

/// 拖图片进来的结果：`skipped` 是被忽略的非图片 / 超大文件。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub note: NoteSummary,
    pub imported: usize,
    pub skipped: usize,
}

/// 默认收藏目录：「文档\HexBench 收藏」。和收纳盒一样放用户看得见的地方。
pub fn default_root() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let base = crate::sys::documents_dir().or_else(|| {
        std::env::var_os("USERPROFILE").map(|dir| PathBuf::from(dir).join("Documents"))
    });
    #[cfg(not(windows))]
    let base = std::env::var_os("HOME").map(|dir| PathBuf::from(dir).join("Documents"));
    base.map(|dir| dir.join(FOLDER_NAME))
        .ok_or_else(|| "找不到「文档」目录".to_string())
}

/// 配置里留空就走默认目录；每次调用都把目录（和 `images/`）准备好。
pub fn resolve_root(configured: &str) -> Result<PathBuf, String> {
    let trimmed = configured.trim();
    let root = if trimmed.is_empty() {
        default_root()?
    } else {
        PathBuf::from(trimmed)
    };
    ensure_root(&root)?;
    Ok(root)
}

fn ensure_root(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root.join(IMAGES_DIR))
        .map_err(|error| format!("创建收藏目录失败：{error}"))
}

/// 标题 → 安全的文件名（不含 `.md`）。
pub fn sanitize_title(raw: &str) -> String {
    let mut cleaned: String = raw
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    while cleaned.ends_with(' ') || cleaned.ends_with('.') {
        cleaned.pop();
    }
    let mut title = cleaned.trim().trim_start_matches('.').trim_start().to_string();
    if title.to_ascii_lowercase().ends_with(".md") {
        let keep = title.chars().count().saturating_sub(3);
        title = title.chars().take(keep).collect::<String>();
        title = title.trim().trim_end_matches('.').trim().to_string();
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        title = title.chars().take(MAX_TITLE_CHARS).collect();
    }
    if title.is_empty() {
        return TITLE_FALLBACK.to_string();
    }
    if is_reserved_name(&title) {
        title.insert(0, '_');
    }
    title
}

fn is_reserved_name(title: &str) -> bool {
    let base = title.split('.').next().unwrap_or(title).to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit())
}

/// 图片文件名的前缀：保留中文，去掉路径分隔符之类。
fn sanitize_file_stem(raw: &str, max: usize) -> String {
    let cleaned: String = raw
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
    let cut: String = trimmed.chars().take(max).collect();
    let cut = cut.trim().to_string();
    if cut.is_empty() {
        "图片".to_string()
    } else {
        cut
    }
}

fn unique_note_name(root: &Path, title: &str, keep: Option<&Path>) -> String {
    let base = sanitize_title(title);
    let keep_name = keep
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().to_string());
    let mut index = 1usize;
    loop {
        let name = if index == 1 {
            format!("{base}.md")
        } else {
            format!("{base} ({index}).md")
        };
        if keep_name.as_deref() == Some(name.as_str()) {
            return name;
        }
        // 只改大小写时也要认回同一个文件，否则会变成「标题 (2).md」。
        if keep_name
            .as_deref()
            .is_some_and(|current| current.eq_ignore_ascii_case(&name))
        {
            return name;
        }
        if !root.join(&name).exists() {
            return name;
        }
        index += 1;
    }
}

fn is_note_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    !name.starts_with('.')
        && path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

fn note_title(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| TITLE_FALLBACK.to_string())
}

fn note_id(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn modified_secs(path: &Path) -> i64 {
    path.metadata()
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_secs() as i64)
        .unwrap_or(0)
}

fn read_capped(path: &Path, cap: u64) -> Result<String, String> {
    let file = File::open(path).map_err(|error| format!("读取笔记失败：{error}"))?;
    let mut bytes = Vec::new();
    file.take(cap)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取笔记失败：{error}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn summary_of(root: &Path, path: &Path) -> Result<NoteSummary, String> {
    let body = read_capped(path, LIST_READ_CAP)?;
    let images = image_refs(&body)
        .into_iter()
        .filter_map(|name| {
            let file = root.join(IMAGES_DIR).join(&name);
            file.is_file().then(|| NoteImage {
                name,
                path: file.to_string_lossy().to_string(),
            })
        })
        .collect();
    Ok(NoteSummary {
        id: note_id(path),
        title: note_title(path),
        preview: preview(&body),
        images,
        modified: modified_secs(path),
    })
}

fn valid_image_name(name: &str) -> bool {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return false;
    }
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| IMAGE_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// 正文里所有 `![...](...)` 的图片引用（去重、保持顺序）。
/// 只收 `images/<文件名>` 这一种相对写法。
pub fn image_refs(body: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (_, _, target) in image_matches(body) {
        // 生成的引用用 `<...>` 包住文件名（名字里可能有空格），也兼容不加尖括号的写法。
        let target = target.trim().trim_matches(|c| c == '<' || c == '>');
        let name = target.strip_prefix("images/").unwrap_or(target);
        if valid_image_name(name) && !names.iter().any(|seen| seen == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// 找到正文里每个 Markdown 图片语法的字节区间和链接目标。
fn image_matches(body: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut scan = 0usize;
    while let Some(offset) = body[scan..].find("![") {
        let start = scan + offset;
        let after = &body[start + 2..];
        let Some(alt_end) = after.find("](") else {
            break;
        };
        let target_start = start + 2 + alt_end + 2;
        let target = &body[target_start..];
        // 文件名里可能有括号：数括号找配对的 `)`。
        let mut depth = 1usize;
        let mut end = None;
        for (index, ch) in target.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            break;
        };
        out.push((start, target_start + end + 1, &target[..end]));
        scan = target_start + end + 1;
    }
    out
}

/// 发送 / 预览用：把图片语法整段去掉。
pub fn strip_image_refs(body: &str) -> String {
    let spans = image_matches(body);
    if spans.is_empty() {
        return body.to_string();
    }
    let mut out = String::with_capacity(body.len());
    let mut last = 0usize;
    for (start, end, _) in spans {
        out.push_str(&body[last..start]);
        last = end;
    }
    out.push_str(&body[last..]);
    out
}

/// 真正要输入到目标窗口的文字：去掉图片引用，统一换行，最多留一个空行。
pub fn send_text(body: &str) -> String {
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    let stripped = strip_image_refs(&normalized);
    let mut out = String::new();
    let mut blanks = 0usize;
    for line in stripped.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 2 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// 卡片上的一行预览：去掉图片、标题井号、列表符号和强调标记。
pub fn preview(body: &str) -> String {
    let stripped = strip_image_refs(body);
    let mut parts: Vec<String> = Vec::new();
    for raw in stripped.lines() {
        let mut line = raw.trim();
        if line.is_empty() || line.starts_with("```") {
            continue;
        }
        if line
            .chars()
            .all(|c| c == '-' || c == '*' || c == '_' || c == ' ')
        {
            continue;
        }
        line = line.trim_start_matches('#').trim_start();
        line = line.trim_start_matches('>').trim_start();
        for marker in ["- ", "* ", "+ "] {
            if let Some(rest) = line.strip_prefix(marker) {
                line = rest.trim_start();
                break;
            }
        }
        if let Some((head, rest)) = line.split_once(". ") {
            if !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()) {
                line = rest;
            }
        }
        let cleaned: String = line
            .chars()
            .filter(|c| !matches!(c, '`' | '*' | '_' | '~'))
            .collect();
        let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        if !cleaned.is_empty() {
            parts.push(cleaned);
        }
    }
    let joined = parts.join(" ");
    if joined.chars().count() > PREVIEW_CHARS {
        format!("{}…", joined.chars().take(PREVIEW_CHARS).collect::<String>())
    } else {
        joined
    }
}

/// 列目录：按修改时间倒序。
pub fn list(root: &Path) -> Result<Vec<NoteSummary>, String> {
    ensure_root(root)?;
    let entries = fs::read_dir(root).map_err(|error| format!("读取收藏目录失败：{error}"))?;
    let mut notes = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_note_file(&path) {
            continue;
        }
        if let Ok(note) = summary_of(root, &path) {
            notes.push(note);
        }
    }
    notes.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.title.cmp(&right.title))
    });
    Ok(notes)
}

/// 新建一条空笔记。
pub fn create(root: &Path, title: Option<&str>) -> Result<NoteSummary, String> {
    ensure_root(root)?;
    let name = unique_note_name(root, title.unwrap_or(TITLE_FALLBACK), None);
    let path = root.join(&name);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("新建笔记失败：{error}"))?;
    summary_of(root, &path)
}

pub fn read(root: &Path, id: &str) -> Result<NoteFile, String> {
    let path = resolve_note(root, id)?;
    Ok(NoteFile {
        id: note_id(&path),
        title: note_title(&path),
        body: fs::read_to_string(&path).unwrap_or_else(|_| {
            String::from_utf8_lossy(&fs::read(&path).unwrap_or_default()).into_owned()
        }),
        modified: modified_secs(&path),
    })
}

/// 保存正文；标题变了就把文件改名（重名自动加序号）。
pub fn save(root: &Path, id: &str, title: &str, body: &str) -> Result<NoteSummary, String> {
    ensure_root(root)?;
    let old = resolve_note(root, id)?;
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    write_atomic(&old, normalized.as_bytes())?;
    let name = unique_note_name(root, title, Some(&old));
    let current = note_id(&old);
    if current != name {
        let next = root.join(&name);
        if current.eq_ignore_ascii_case(&name) {
            // Windows 不区分大小写：直接 rename 到「只差大小写」的名字会报已存在，
            // 先进一个临时名再改过去。
            let bridge = root.join(format!(
                ".rename-{}-{}.tmp",
                std::process::id(),
                ::std::time::SystemTime::now()
                    .duration_since(::std::time::UNIX_EPOCH)
                    .map(|delta| delta.as_nanos())
                    .unwrap_or(0)
            ));
            fs::rename(&old, &bridge).map_err(|error| format!("改名失败：{error}"))?;
            if let Err(error) = fs::rename(&bridge, &next) {
                let _ = fs::rename(&bridge, &old);
                return Err(format!("改名失败：{error}"));
            }
        } else {
            fs::rename(&old, &next).map_err(|error| format!("改名失败：{error}"))?;
        }
        return summary_of(root, &next);
    }
    summary_of(root, &old)
}

/// 删除一条笔记（进回收站），顺手回收没有任何笔记引用的图片。
pub fn delete(root: &Path, id: &str) -> Result<(), String> {
    let path = resolve_note(root, id)?;
    crate::shell_ops::trash_quiet(&[path])?;
    gc_images(root);
    Ok(())
}

pub fn clear_all(root: &Path) -> Result<usize, String> {
    ensure_root(root)?;
    let mut victims: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_note_file(&path) {
                victims.push(path);
            }
        }
    }
    if let Ok(entries) = fs::read_dir(root.join(IMAGES_DIR)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                victims.push(path);
            }
        }
    }
    let count = victims.len();
    crate::shell_ops::trash_quiet(&victims)?;
    Ok(count)
}

/// 把图片复制进 `images/`：`note_id` 有值就追加到那条笔记，否则新建一条。
pub fn import_images(
    root: &Path,
    note_id: Option<&str>,
    sources: &[PathBuf],
) -> Result<ImportOutcome, String> {
    ensure_root(root)?;
    let images = root.join(IMAGES_DIR);
    let mut refs: Vec<(String, String)> = Vec::new(); // (引用整行, 图片名)
    let mut skipped = 0usize;
    for source in sources {
        let Some(ext) = source
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
        else {
            skipped += 1;
            continue;
        };
        if !IMAGE_EXTS.contains(&ext.as_str()) || !source.is_file() {
            skipped += 1;
            continue;
        }
        let size = source.metadata().map(|meta| meta.len()).unwrap_or(0);
        if size == 0 || size > MAX_IMAGE_BYTES {
            skipped += 1;
            continue;
        }
        let bytes =
            fs::read(source).map_err(|error| format!("读取 {} 失败：{error}", source.display()))?;
        let hash = content_hash(&bytes);
        let stem = sanitize_file_stem(
            source
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("图片"),
            MAX_IMAGE_STEM,
        );
        let name = format!("{hash:016x}-{stem}.{ext}");
        let target = images.join(&name);
        if !target.is_file() {
            write_atomic(&target, &bytes)?;
        }
        refs.push((format!("![{stem}](<images/{name}>)"), name));
    }
    if refs.is_empty() {
        return Err("拖进来的不是图片".to_string());
    }
    let imported = refs.len();
    let note = match note_id {
        Some(id) => {
            let path = resolve_note(root, id)?;
            let body = read_capped(&path, u64::MAX)?;
            let additions: Vec<&str> = refs
                .iter()
                .filter(|(reference, _)| !body.contains(reference.as_str()))
                .map(|(reference, _)| reference.as_str())
                .collect();
            if !additions.is_empty() {
                let mut next = body.trim_end().to_string();
                for reference in additions {
                    if !next.is_empty() {
                        next.push_str("\n\n");
                    }
                    next.push_str(reference);
                }
                next.push('\n');
                write_atomic(&path, next.as_bytes())?;
            }
            summary_of(root, &path)?
        }
        None => {
            let title = sources
                .iter()
                .filter_map(|source| source.file_stem().and_then(|stem| stem.to_str()))
                .next()
                .unwrap_or("图片笔记");
            let name = unique_note_name(root, &sanitize_file_stem(title, 24), None);
            let path = root.join(&name);
            let body: Vec<&str> = refs
                .iter()
                .map(|(reference, _)| reference.as_str())
                .collect();
            let mut content = body.join("\n\n");
            content.push('\n');
            write_atomic(&path, content.as_bytes())?;
            summary_of(root, &path)?
        }
    };
    Ok(ImportOutcome {
        note,
        imported,
        skipped,
    })
}

/// 一条笔记引用的图片文件（复制到剪贴板用）。
pub fn image_paths(root: &Path, id: &str) -> Result<Vec<PathBuf>, String> {
    let path = resolve_note(root, id)?;
    let body = read_capped(&path, u64::MAX)?;
    let images = root.join(IMAGES_DIR);
    Ok(image_refs(&body)
        .into_iter()
        .map(|name| images.join(name))
        .filter(|file| file.is_file())
        .collect())
}

/// 便签窗口通过自定义协议取原图：只认 `images/` 下的合法图片名，
/// 解析后的真实路径必须还在 `images/` 里（挡住符号链接之类的花招）。
pub fn image_file(root: &Path, name: &str) -> Option<PathBuf> {
    if !valid_image_name(name) {
        return None;
    }
    let images = root.join(IMAGES_DIR);
    let file = images.join(name);
    if !file.is_file() {
        return None;
    }
    let canonical = file.canonicalize().ok()?;
    let base = images.canonicalize().ok()?;
    canonical.starts_with(&base).then_some(canonical)
}

/// 图片扩展名对应的 MIME。协议里不写对的话，`<img>` 会在部分格式上不显示。
pub fn image_mime(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        _ => "image/png",
    }
}

pub fn resolve_note(root: &Path, id: &str) -> Result<PathBuf, String> {
    let name = id.trim();
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return Err("笔记名不正确".to_string());
    }
    if !name.to_ascii_lowercase().ends_with(".md") {
        return Err("不是笔记文件".to_string());
    }
    let path = root.join(name);
    if !path.is_file() {
        return Err("笔记不存在或已经被移动".to_string());
    }
    Ok(path)
}

/// 临时文件 + rename：写到一半断电也不会留下半条笔记。
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| "路径不对".to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_nanos())
        .unwrap_or(0);
    let temp = directory.join(format!(".write-{}-{stamp}.tmp", std::process::id()));
    {
        let mut file =
            File::create(&temp).map_err(|error| format!("写入失败：{error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("写入失败：{error}"))?;
        let _ = file.sync_all();
    }
    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("保存失败：{error}")
    })
}

/// 删掉 `images/` 里已经没有任何笔记引用的图片。读不到的笔记按「还在用」处理，
/// 宁可多留一张，也不能把别处还在引用的图删了。
fn gc_images(root: &Path) {
    let mut used: HashSet<String> = HashSet::new();
    // 连目录都读不出来时不能当成「没有任何笔记引用图片」——那样会把
    // images/ 整个清空。宁可这次不回收。
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_note_file(&path) {
            continue;
        }
        // 这里必须读整篇：列表预览可以截断，但回收判断不能——长笔记后半段
        // 引用的图片会被当成没人用，然后被搬进回收站。
        match fs::read(&path) {
            Ok(bytes) => {
                used.extend(image_refs(&String::from_utf8_lossy(&bytes)));
            }
            Err(_) => return,
        }
    }
    let Ok(entries) = fs::read_dir(root.join(IMAGES_DIR)) else {
        return;
    };
    let unused: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| !name.starts_with('.') && !used.contains(name))
        })
        .collect();
    if !unused.is_empty() {
        let _ = crate::shell_ops::trash_quiet(&unused);
    }
}

/// FNV-1a 64 位：只用来给图片内容起名 / 去重，不承担安全职责。
fn content_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn temp_dir(tag: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hexbench-notes-{tag}-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png_bytes(tag: u8) -> Vec<u8> {
        vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, tag]
    }

    #[test]
    fn titles_are_sanitized() {
        assert_eq!(sanitize_title("  邮箱  "), "邮箱");
        assert_eq!(sanitize_title("a:b?.txt"), "a_b_.txt");
        assert_eq!(sanitize_title("../../windows/evil"), "_.._windows_evil");
        assert_eq!(sanitize_title("CON"), "_CON");
        assert_eq!(sanitize_title("con.txt"), "_con.txt");
        assert_eq!(sanitize_title("   "), TITLE_FALLBACK);
        assert_eq!(sanitize_title("笔记.md"), "笔记");
        assert_eq!(sanitize_title(&"很".repeat(100)).chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn image_refs_are_found_and_stripped() {
        let body = "第一行\n![图片](images/abc-def.png)\n文字 ![x](images/(1).jpg)\n![外链](https://example.com/a.png)\n";
        assert_eq!(
            image_refs(body),
            vec!["abc-def.png".to_string(), "(1).jpg".to_string()]
        );
        let stripped = strip_image_refs(body);
        assert!(!stripped.contains("!["));
        assert!(stripped.contains("第一行"));
        assert!(stripped.contains("文字"));
        assert!(!stripped.contains("example.com"), "外链也整段去掉");
        assert!(image_refs("![bad](../evil.png)").is_empty());
        assert!(image_refs("![](images/..%2f.png)").is_empty());
    }

    #[test]
    fn preview_drops_markup() {
        let body = "# 标题\n\n- 第一项\n- 第二项\n\n**加粗** 和 `代码`\n\n![图](images/a.png)";
        let text = preview(body);
        assert_eq!(text, "标题 第一项 第二项 加粗 和 代码");
        assert_eq!(preview("   \n\n  "), "");
    }

    #[test]
    fn send_text_keeps_paragraphs_but_not_images() {
        let body = "你好\r\n\r\n![图](images/a.png)\r\n\r\n\r\n\r\n署名";
        assert_eq!(send_text(body), "你好\n\n\n署名");
        assert_eq!(send_text("![图](images/a.png)"), "");
    }

    #[test]
    fn create_read_save_rename_roundtrip() {
        let root = temp_dir("roundtrip");
        let note = create(&root, Some("邮箱")).unwrap();
        assert_eq!(note.id, "邮箱.md");
        assert_eq!(note.title, "邮箱");
        assert!(root.join("邮箱.md").is_file());

        let saved = save(&root, &note.id, "常用邮箱", "hi@example.com\n").unwrap();
        assert_eq!(saved.id, "常用邮箱.md");
        assert!(!root.join("邮箱.md").exists());
        assert_eq!(read(&root, &saved.id).unwrap().body, "hi@example.com\n");

        // 同名再建一条：自动加序号。
        let second = create(&root, Some("常用邮箱")).unwrap();
        assert_eq!(second.id, "常用邮箱 (2).md");

        // 只改大小写不该变成「(2)」。
        let renamed = save(&root, &saved.id, "常用邮箱A", "x").unwrap();
        assert_eq!(renamed.id, "常用邮箱A.md");
        let back = save(&root, &renamed.id, "常用邮箱a", "x").unwrap();
        assert_eq!(back.id, "常用邮箱a.md");

        delete(&root, &back.id).unwrap();
        let left = list(&root).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "常用邮箱 (2).md");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn images_are_copied_deduped_and_appended() {
        let root = temp_dir("images");
        let source = temp_dir("source");
        let one = source.join("截图 1.png");
        let two = source.join("截图 2.jpg");
        fs::write(&one, png_bytes(1)).unwrap();
        fs::write(&two, png_bytes(2)).unwrap();
        let text = source.join("说明.txt");
        fs::write(&text, b"not an image").unwrap();

        let outcome =
            import_images(&root, None, &[one.clone(), text.clone()]).unwrap();
        assert_eq!(outcome.imported, 1);
        assert_eq!(outcome.skipped, 1);
        assert_eq!(outcome.note.title, "截图 1");
        let body = read(&root, &outcome.note.id).unwrap().body;
        assert!(body.contains("(<images/") && body.contains("截图 1.png"));
        assert!(one.exists(), "源文件必须留在原处");

        let images = fs::read_dir(root.join(IMAGES_DIR)).unwrap().count();
        assert_eq!(images, 1);

        // 同一张图再拖一次：文件不重复，引用也不重复。
        let again = import_images(&root, Some(&outcome.note.id), std::slice::from_ref(&one)).unwrap();
        assert_eq!(again.imported, 1);
        assert_eq!(fs::read_dir(root.join(IMAGES_DIR)).unwrap().count(), 1);
        let body_again = read(&root, &outcome.note.id).unwrap().body;
        assert_eq!(body_again.matches("images/").count(), 1);

        // 另一张图拖到已有笔记上：追加。
        let appended = import_images(&root, Some(&outcome.note.id), &[two]).unwrap();
        assert_eq!(appended.note.images.len(), 2);
        assert!(appended.note.images[1].name.ends_with("截图 2.jpg"));
        assert_eq!(image_paths(&root, &outcome.note.id).unwrap().len(), 2);

        // 删掉笔记，图片跟着进回收站或至少不再留在目录里。
        delete(&root, &outcome.note.id).unwrap();
        assert!(!root.join(&outcome.note.id).exists());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&source);
    }

    #[test]
    fn list_orders_by_modified_desc() {
        let root = temp_dir("order");
        let first = create(&root, Some("早")).unwrap();
        let second = create(&root, Some("晚")).unwrap();
        let now = SystemTime::now();
        let old = File::options()
            .write(true)
            .open(root.join(&first.id))
            .unwrap();
        old.set_modified(now - Duration::from_secs(120)).unwrap();
        let newer = File::options()
            .write(true)
            .open(root.join(&second.id))
            .unwrap();
        newer.set_modified(now).unwrap();
        let notes = list(&root).unwrap();
        assert_eq!(notes[0].id, second.id);
        assert_eq!(notes[1].id, first.id);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_all_removes_notes_and_images() {
        let root = temp_dir("clear");
        let source = temp_dir("clear-src");
        let image = source.join("a.png");
        fs::write(&image, png_bytes(7)).unwrap();
        let note = import_images(&root, None, &[image]).unwrap().note;
        create(&root, Some("文字笔记")).unwrap();
        let removed = clear_all(&root).unwrap();
        assert_eq!(removed, 3, "一条 md + 一条带图 md + 一张图");
        assert!(list(&root).unwrap().is_empty());
        assert_eq!(fs::read_dir(root.join(IMAGES_DIR)).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&source);
        let _ = note;
    }
}
