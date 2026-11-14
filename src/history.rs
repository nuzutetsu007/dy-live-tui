//! 观看历史：只记录「用 mpv 成功打开过」的直播间。
//!
//! 存放在 `~/.cache/dy-live-tui/history.tsv`（`XDG_CACHE_HOME` 优先），
//! 每行 `rid \t 主播名 \t 标题`，最近打开的在最前面。

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub rid: String,
    pub nick: String,
    pub title: String,
}

impl Entry {
    /// 一行显示用的摘要，过长按字符截断（CJK 也算一个字符）
    pub fn summary(&self, max: usize) -> String {
        let mut s = String::new();
        if !self.nick.is_empty() {
            s.push_str(&self.nick);
        }
        if !self.title.is_empty() {
            if !s.is_empty() {
                s.push_str(" · ");
            }
            s.push_str(&self.title);
        }
        if s.chars().count() > max {
            let t: String = s.chars().take(max.saturating_sub(1)).collect();
            s = format!("{t}…");
        }
        s
    }
}

pub struct History {
    entries: Vec<Entry>,
    path: PathBuf,
    limit: usize,
}

impl History {
    pub fn load() -> Self {
        Self::load_from(cache_dir().join("history.tsv"))
    }

    pub fn load_from(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut entries = Vec::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let mut it = line.splitn(3, '\t');
                let Some(rid) = it.next() else { continue };
                let rid = rid.trim();
                if rid.is_empty() || !rid.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let nick = it.next().unwrap_or("").trim().to_string();
                let title = it.next().unwrap_or("").trim().to_string();
                if entries.iter().any(|e: &Entry| e.rid == rid) {
                    continue;
                }
                entries.push(Entry {
                    rid: rid.to_string(),
                    nick,
                    title,
                });
            }
        }
        Self {
            entries,
            path,
            limit: 12,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 记录一次成功打开：同房间去重并挪到最前，最多保留 `limit` 条
    pub fn add(&mut self, rid: &str, nick: &str, title: &str) {
        let rid = rid.trim();
        if rid.is_empty() {
            return;
        }
        let clean = |s: &str| s.replace(['\t', '\r', '\n'], " ").trim().to_string();
        self.entries.retain(|e| e.rid != rid);
        self.entries.insert(
            0,
            Entry {
                rid: rid.to_string(),
                nick: clean(nick),
                title: clean(title),
            },
        );
        self.entries.truncate(self.limit);
        self.save();
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut out = String::new();
        for e in &self.entries {
            out.push_str(&format!("{}\t{}\t{}\n", e.rid, e.nick, e.title));
        }
        // 写临时文件再 rename，避免半截文件
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, out).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".cache")
        })
        .join("dy-live-tui")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("dy-live-test-{name}-{}.tsv", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn add_dedup_and_order() {
        let p = tmp("add");
        let mut h = History::load_from(&p);
        h.add("111", "主播A", "标题A");
        h.add("222", "主播B", "标题B");
        h.add("111", "主播A2", "标题A2");
        assert_eq!(h.entries().len(), 2);
        assert_eq!(h.entries()[0].rid, "111");
        assert_eq!(h.entries()[0].nick, "主播A2");
        assert_eq!(h.entries()[1].rid, "222");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn persists_and_reloads() {
        let p = tmp("reload");
        let mut h = History::load_from(&p);
        h.add("333", "主播C", "标题\tC");
        drop(h);

        let h = History::load_from(&p);
        assert_eq!(h.entries().len(), 1);
        assert_eq!(h.entries()[0].rid, "333");
        // 制表符被压成空格，不会破坏格式
        assert_eq!(h.entries()[0].title, "标题 C");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn skips_bad_lines() {
        let p = tmp("bad");
        std::fs::write(&p, "abc\tx\ty\n444\t主播D\t标题D\n\n").unwrap();
        let h = History::load_from(&p);
        assert_eq!(h.entries().len(), 1);
        assert_eq!(h.entries()[0].rid, "444");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn limit_keeps_newest() {
        let p = tmp("limit");
        let mut h = History::load_from(&p);
        for i in 0..20 {
            h.add(&format!("{}", 1000 + i), "", "");
        }
        assert_eq!(h.entries().len(), 12);
        assert_eq!(h.entries()[0].rid, "1019");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn summary_truncates() {
        let e = Entry {
            rid: "1".into(),
            nick: "主播".into(),
            title: "一个很长很长的标题".into(),
        };
        let s = e.summary(8);
        assert!(s.chars().count() <= 8, "{s}");
        assert!(s.ends_with('…'));
    }
}
