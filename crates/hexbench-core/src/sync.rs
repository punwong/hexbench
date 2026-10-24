//! Minimal WebDAV client for syncing `config.json`.
//!
//! Only the three verbs the app needs: PROPFIND (connectivity check), GET and
//! PUT, with HTTP Basic auth. Keeping it this small avoids pulling a whole
//! HTTP stack into a 1.5 MB installer.

use base64::Engine;
use std::time::Duration;

pub struct WebDav {
    base: String,
    user: String,
    pass: String,
}

impl WebDav {
    pub fn new(base: &str, user: &str, pass: &str) -> Self {
        let mut base = base.trim().to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        Self {
            base,
            user: user.to_string(),
            pass: pass.to_string(),
        }
    }

    fn agent(&self) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(8))
            .timeout_read(Duration::from_secs(20))
            .timeout_write(Duration::from_secs(20))
            .build()
    }

    fn auth(&self) -> String {
        let raw = format!("{}:{}", self.user, self.pass);
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        )
    }

    fn url(&self, name: &str) -> String {
        format!("{}{}", self.base, name)
    }

    /// Check that the URL is a WebDAV collection we can talk to.
    pub fn test(&self) -> Result<(), String> {
        let url = self.base.clone();
        match self
            .agent()
            .request("PROPFIND", &url)
            .set("Authorization", &self.auth())
            .set("Depth", "0")
            .call()
        {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(401, _)) => Err("认证失败：用户名或密码不对".to_string()),
            Err(ureq::Error::Status(404, _)) => Err("目录不存在，检查 URL".to_string()),
            Err(ureq::Error::Status(405, _)) => {
                Err("服务器不支持 WebDAV（PROPFIND 405），确认这是 WebDAV 地址".to_string())
            }
            Err(error) => Err(describe(&url, error)),
        }
    }

    /// Fetch the remote file; `None` when it does not exist yet.
    pub fn get(&self, name: &str) -> Result<Option<String>, String> {
        let url = self.url(name);
        match self
            .agent()
            .get(&url)
            .set("Authorization", &self.auth())
            .call()
        {
            Ok(response) => response
                .into_string()
                .map(Some)
                .map_err(|error| error.to_string()),
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(error) => Err(describe(&url, error)),
        }
    }

    /// Upload the file.
    pub fn put(&self, name: &str, body: &str) -> Result<(), String> {
        let url = self.url(name);
        match self
            .agent()
            .put(&url)
            .set("Authorization", &self.auth())
            .set("Content-Type", "application/json; charset=utf-8")
            .send_string(body)
        {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(409, _)) | Err(ureq::Error::Status(404, _)) => Err(format!(
                "远程目录不存在（{url}），先在网盘里建好这个文件夹"
            )),
            Err(error) => Err(describe(&url, error)),
        }
    }
}

/// Encrypt a password for storage in the config file (base64 of a DPAPI blob).
pub fn seal_secret(password: &str) -> String {
    #[cfg(windows)]
    {
        if let Some(cipher) = crate::dpapi::protect(password) {
            return base64::engine::general_purpose::STANDARD.encode(cipher);
        }
    }
    String::new()
}

/// Decrypt a value produced by [`seal_secret`].
pub fn open_secret(sealed: &str) -> Option<String> {
    if sealed.is_empty() {
        return Some(String::new());
    }
    #[cfg(windows)]
    {
        let cipher = base64::engine::general_purpose::STANDARD
            .decode(sealed)
            .ok()?;
        crate::dpapi::unprotect(&cipher)
    }
    #[cfg(not(windows))]
    {
        let _ = sealed;
        None
    }
}

fn describe(url: &str, error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, _) => format!("HTTP {code}（{url}）"),
        ureq::Error::Transport(transport) => format!("连不上 {url}：{transport}"),
    }
}
