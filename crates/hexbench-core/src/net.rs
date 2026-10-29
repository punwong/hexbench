//! 取网页标题：设置页里加网址时自动填站名，省得手打。

use std::io::Read;
use std::time::Duration;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 没写协议就补 https://
pub fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

/// `https://www.chatgpt.com/x?y` → `chatgpt.com`
fn domain_of(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    host.trim_start_matches("www.").to_string()
}

/// 从 HTML 里挖 `<title>`，大小写和空白都不敏感。
fn title_from_html(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start = lower.find("<title")?;
    let after_tag = lower[start..].find('>')? + start + 1;
    let end = lower[after_tag..].find("</title")? + after_tag;
    let raw = html[after_tag..end].trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw.replace(['\n', '\r', '\t'], " ").trim().to_string())
    }
}

/// 抓标题；抓不到就退回域名，绝不让名字栏空着。
pub fn site_title(url: &str) -> String {
    let target = normalize_url(url);
    if target.is_empty() {
        return String::new();
    }
    let fallback = domain_of(&target);

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(4))
        .timeout_read(Duration::from_secs(6))
        .user_agent(USER_AGENT)
        .build();
    let Ok(response) = agent.get(&target).call() else {
        return fallback;
    };
    let mut bytes: Vec<u8> = Vec::new();
    // 只读前 128 KB：标题总在 <head> 里，没必要整页下载。
    if response
        .into_reader()
        .take(128 * 1024)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return fallback;
    }
    let html = String::from_utf8_lossy(&bytes);
    match title_from_html(&html) {
        Some(title) => {
            // 有些站点的标题巨长，取前 40 个字符够用了。
            if title.chars().count() > 40 {
                title.chars().take(40).collect::<String>() + "…"
            } else {
                title
            }
        }
        None => fallback,
    }
}

/// 抓站点图标（`/favicon.ico`），返回 `data:` URI；拿不到返回空串。
///
/// 图标很小（通常 1–20 KB），直接内联给前端最省事：不用再管临时文件
/// 和缓存淘汰，页面打开时顺手换掉即可。
pub fn favicon_data_uri(url: &str) -> String {
    use base64::Engine as _;

    let target = normalize_url(url);
    if target.is_empty() {
        return String::new();
    }
    let domain = domain_of(&target);
    if domain.is_empty() {
        return String::new();
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(4))
        .user_agent(USER_AGENT)
        .build();
    let Ok(response) = agent.get(&format!("https://{domain}/favicon.ico")).call() else {
        return String::new();
    };
    let mut bytes: Vec<u8> = Vec::new();
    if response
        .into_reader()
        .take(256 * 1024)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.is_empty()
    {
        return String::new();
    }
    // 只认图片，避免把某些站点的 404 页面当成图标内联进去。
    let looks_like_image = bytes.starts_with(&[0x00, 0x00, 0x01, 0x00])
        || bytes.starts_with(&[0x89, b'P', b'N', b'G'])
        || bytes.starts_with(&[0xFF, 0xD8])
        || bytes.starts_with(b"GIF8");
    if !looks_like_image {
        return String::new();
    }
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else {
        "image/x-icon"
    };
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_in_the_missing_scheme() {
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("http://a.b"), "http://a.b");
        assert_eq!(normalize_url("  "), "");
    }

    #[test]
    fn turns_a_url_into_a_readable_domain() {
        assert_eq!(domain_of("https://www.chatgpt.com/c/123"), "chatgpt.com");
        assert_eq!(domain_of("http://sub.example.org?q=1"), "sub.example.org");
    }

    #[test]
    fn pulls_the_title_out_of_html() {
        let html = "<html><head><TITLE>\n  ChatGPT  \n</TITLE></head></html>";
        assert_eq!(title_from_html(html).as_deref(), Some("ChatGPT"));
        assert_eq!(title_from_html("<html></html>"), None);
    }
}