//! 使用频率：面板里打开过哪些文件、各开了多少次。
//!
//! 单独存一个 `usage.json`（和 `config.json` 同目录），不塞进配置里：
//! 这份数据不进同步，路径多了也不会把用户手改的配置撑得很大。

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 最多记这么多条。长期用下来路径只增不减，超过就按最久没用的淘汰。
pub const CAP: usize = 4000;

/// 一项的使用记录。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Usage {
    /// 打开过几次。
    pub count: u32,
    /// 最后一次打开的时间（Unix 秒），只用来做淘汰和同次数时的先后。
    pub last: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageData {
    /// 完整路径 → 记录。
    pub items: HashMap<String, Usage>,
}

impl UsageData {
    /// 记一次打开：次数加一、时间更新。
    pub fn record(&mut self, path: &str, now: i64) {
        let entry = self.items.entry(path.to_string()).or_default();
        entry.count = entry.count.saturating_add(1);
        entry.last = now;
    }

    /// 超过上限时丢掉最久没用过的，直到回到上限以内。
    pub fn trim(&mut self) {
        if self.items.len() <= CAP {
            return;
        }
        let mut keys: Vec<(i64, String)> = self
            .items
            .iter()
            .map(|(path, usage)| (usage.last, path.clone()))
            .collect();
        keys.sort_unstable();
        let drop_count = self.items.len() - CAP;
        for (_, path) in keys.into_iter().take(drop_count) {
            self.items.remove(&path);
        }
    }
}

/// `config.json` 旁边的 `usage.json`。
pub fn file_of(config_path: &Path) -> PathBuf {
    config_path.with_file_name("usage.json")
}

/// 读盘。文件不在 / 读坏了都当成空表：这只是个排序依据，不值得让面板打不开。
pub fn load(path: &Path) -> UsageData {
    let Ok(bytes) = fs::read(path) else {
        return UsageData::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        log::warn!("usage: {} 读不出来（{error}），按空表处理", path.display());
        UsageData::default()
    })
}

/// 写盘：先写临时文件再换名，写到一半断电不会留下半个 JSON。
/// 这份文件只给排序用，写紧凑一点就行，不用给人看。
pub fn save(path: &Path, data: &UsageData) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(data).map_err(io::Error::other)?;
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hexbench-usage-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("usage.json")
    }

    #[test]
    fn record_counts_up() {
        let mut data = UsageData::default();
        data.record("C:\\a\\b.txt", 100);
        data.record("C:\\a\\b.txt", 200);
        let entry = data.items.get("C:\\a\\b.txt").copied().unwrap();
        assert_eq!(entry.count, 2);
        assert_eq!(entry.last, 200);
    }

    #[test]
    fn round_trips_through_disk() {
        let path = temp_file("round");
        let mut data = UsageData::default();
        data.record("C:\\a\\b.txt", 42);
        save(&path, &data).unwrap();
        assert_eq!(load(&path), data);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_or_broken_file_is_empty() {
        let path = temp_file("broken");
        assert_eq!(load(&path), UsageData::default());
        fs::write(&path, b"{not json").unwrap();
        assert_eq!(load(&path), UsageData::default());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn trim_keeps_the_most_recent() {
        let mut data = UsageData::default();
        for index in 0..(CAP + 10) {
            data.items.insert(
                format!("C:\\f{index}.txt"),
                Usage {
                    count: 1,
                    last: index as i64,
                },
            );
        }
        data.trim();
        assert_eq!(data.items.len(), CAP);
        // 最旧的十条该被丢掉。
        assert!(!data.items.contains_key("C:\\f0.txt"));
        assert!(data.items.contains_key(&format!("C:\\f{}.txt", CAP + 9)));
    }
}
