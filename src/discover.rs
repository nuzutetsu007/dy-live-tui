//! 按分区分页列出正在直播的房间（纯文本）。
//!
//! 数据来自 `https://live.douyin.com/categorynew/<key>` 页面里内嵌的 SSR JSON，
//! 不调用需要 X-Bogus 签名的 webcast 接口。

/// 一个直播间的文本信息（不取封面/头像）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoomCard {
    /// 房间号（可直接喂给 `douyin::resolve`）
    pub rid: String,
    /// 主播昵称
    pub nick: String,
    /// 直播标题
    pub title: String,
    /// 在线人数展示串，如 `1万+`，可能为空
    pub online: String,
}

/// 一级分区（抖音直播页顶部导航里的 8 个）。
#[derive(Clone, Copy, Debug)]
pub struct Category {
    /// URL 里的 key，如 `4_103`
    pub key: &'static str,
    /// 中文名
    pub name: &'static str,
}

pub const CATEGORIES: [Category; 8] = [
    Category {
        key: "4_101",
        name: "聊天",
    },
    Category {
        key: "4_102",
        name: "音乐",
    },
    Category {
        key: "4_103",
        name: "游戏",
    },
    Category {
        key: "4_104",
        name: "二次元",
    },
    Category {
        key: "4_105",
        name: "舞蹈",
    },
    Category {
        key: "4_106",
        name: "文化",
    },
    Category {
        key: "4_107",
        name: "生活",
    },
    Category {
        key: "4_108",
        name: "运动",
    },
];

/// 解析分类页 HTML，取出房间列表（按房间号去重）。
pub fn parse(html: &str) -> Vec<RoomCard> {
    let Some(json) = extract_rooms_json(html) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return Vec::new();
    };
    let Some(items) = value.get("data").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut seen = std::collections::HashSet::new();
    let mut rooms = Vec::with_capacity(items.len());
    for item in items {
        let Some(rid) = item.get("web_rid").and_then(|v| v.as_str()) else {
            continue;
        };
        if rid.is_empty() || !seen.insert(rid.to_string()) {
            continue;
        }
        let room = item.get("room");
        let field = |path: &[&str]| -> Option<String> {
            let mut cur = room?;
            for key in path {
                cur = cur.get(*key)?;
            }
            Some(cur.as_str().unwrap_or_default().to_string())
        };
        let card = RoomCard {
            rid: rid.to_string(),
            nick: field(&["owner", "nickname"]).unwrap_or_default(),
            title: field(&["title"]).unwrap_or_default(),
            online: field(&["stats", "user_count_str"]).unwrap_or_default(),
        };
        // 昵称和标题都空的条目没有展示价值
        if card.nick.is_empty() && card.title.is_empty() {
            continue;
        }
        rooms.push(card);
    }
    rooms
}

/// 从页面里抠出 `roomsData` 对应的 JSON 对象文本。
///
/// 抖音把 JSON 塞在 JS 字符串里，引号是 `\"`，标题里的引号则是 `\\\"`，
/// 所以先按 JS 字符串规则整体反转义，再做括号配对。
fn extract_rooms_json(html: &str) -> Option<String> {
    let idx = html.find("roomsData")?;

    // 1) JS 字符串内嵌（线上页面的真实形态）
    if let Some(start) = js_string_start(html, idx) {
        let decoded = js_unescape(&html[start..]);
        if let Some(obj) = object_after(&decoded, "roomsData") {
            return Some(obj.to_string());
        }
    }

    // 2) 已经是明文 JSON 的形态（容错）
    object_after(&html[idx..], "roomsData").map(|s| s.to_string())
}

/// 从 `idx` 往前找包住它的 JS 字符串开引号：引号前连续反斜杠数为偶数。
fn js_string_start(html: &str, idx: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut i = idx;
    while i > 0 {
        i -= 1;
        if bytes[i] != b'"' {
            continue;
        }
        let mut backslashes = 0;
        let mut j = i;
        while j > 0 && bytes[j - 1] == b'\\' {
            backslashes += 1;
            j -= 1;
        }
        if backslashes % 2 == 0 {
            return Some(i + 1);
        }
    }
    None
}

/// 按 JS 字符串字面量规则反转义，遇到未转义的收尾引号即停止。
fn js_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => break,
            },
            other => out.push(other),
        }
    }
    out
}

/// 找到 `marker` 之后的第一个 JSON 对象并返回完整文本（跳过字符串里的括号）。
fn object_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let from = text.find(marker)?;
    let start = text[from..].find('{')? + from;
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (offset, &c) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=offset]);
                }
            }
            _ => {}
        }
    }
    None
}

/// 缓存有效期：5 分钟。按 `r` 强制刷新时绕过。
pub const CACHE_TTL_SECS: u64 = 300;

/// 拉取某个一级分区的在播房间（带 5 分钟缓存）。
pub fn fetch(key: &str) -> Result<Vec<RoomCard>, String> {
    load(key, false)
}

/// 强制绕过缓存（界面上的 `r`）。
pub fn fetch_fresh(key: &str) -> Result<Vec<RoomCard>, String> {
    load(key, true)
}

fn load(key: &str, force: bool) -> Result<Vec<RoomCard>, String> {
    let path = cache_file(key);
    if !force {
        if let Some(rooms) = load_cache(&path, now_secs()) {
            return Ok(rooms);
        }
    }

    let url = format!("https://live.douyin.com/categorynew/{key}");
    let html = get_page(&url, &format!("https://live.douyin.com/categorynew/{key}"))?;
    let rooms = parse(&html);
    if rooms.is_empty() {
        return Err("这个分区暂时没解析到房间（可能被风控，或页面结构变了）".into());
    }
    let _ = save_cache(&path, &rooms, now_secs());
    Ok(rooms)
}

fn get_page(url: &str, referer: &str) -> Result<String, String> {
    let resp = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent(crate::douyin::UA)
        .build()
        .get(url)
        .set("Referer", referer)
        .set("Accept-Language", "zh-CN,zh;q=0.9")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("分类页返回 HTTP {code}（可能被风控）"),
            _ => "分类页请求失败（网络不通？）".to_string(),
        })?;
    resp.into_string().map_err(|_| "分类页读取失败".to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_file(key: &str) -> std::path::PathBuf {
    crate::history::cache_dir().join(format!("discover-{key}.json"))
}

fn load_cache(path: &std::path::Path, now: u64) -> Option<Vec<RoomCard>> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let fetched_at = value.get("fetched_at")?.as_u64()?;
    if now.saturating_sub(fetched_at) > CACHE_TTL_SECS {
        return None;
    }
    let items = value.get("rooms")?.as_array()?;
    let mut rooms = Vec::with_capacity(items.len());
    for item in items {
        let rid = item.get("rid")?.as_str()?.to_string();
        if rid.is_empty() {
            return None;
        }
        rooms.push(RoomCard {
            rid,
            nick: item
                .get("nick")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            title: item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            online: item
                .get("online")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        });
    }
    Some(rooms)
}

fn save_cache(path: &std::path::Path, rooms: &[RoomCard], now: u64) -> std::io::Result<()> {
    let items: Vec<serde_json::Value> = rooms
        .iter()
        .map(|r| {
            serde_json::json!({
                "rid": r.rid,
                "nick": r.nick,
                "title": r.title,
                "online": r.online,
            })
        })
        .collect();
    let doc = serde_json::json!({ "fetched_at": now, "rooms": items });
    let data = serde_json::to_vec(&doc)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟抖音分类页：JSON 被塞进 JS 字符串里，所有引号都是 `\"`。
    const PAGE: &str = r#"<html><script nonce="x">self.__pace_f.push([1,"1:I{\"id\":\"36197\",\"chunks\":[]}\n"])</script><script nonce="x">self.__pace_f.push([1,"{\"roomsData\":{\"count\":15,\"offset\":15,\"data\":[{\"room\":{\"id_str\":\"7708188708536732431\",\"title\":\"到美之声文化传媒大舞台了\",\"owner\":{\"nickname\":\"玫玫小张帝\"},\"stats\":{\"user_count_str\":\"1万+\"}},\"web_rid\":\"40132179559\"},{\"room\":{\"title\":\"上分教学，直播间能破百吗\",\"owner\":{\"nickname\":\"乱斗教授\"},\"stats\":{\"user_count_str\":\"\"}},\"web_rid\":\"544590831076\"},{\"room\":{\"title\":\"重复出现的房间\",\"owner\":{\"nickname\":\"玫玫小张帝\"},\"stats\":{\"user_count_str\":\"2万\"}},\"web_rid\":\"40132179559\"}]}}"])</script></html>"#;

    #[test]
    fn parses_rooms_and_dedupes() {
        let rooms = parse(PAGE);
        assert_eq!(rooms.len(), 2, "重复的 web_rid 应该去重: {rooms:?}");
        assert_eq!(rooms[0].rid, "40132179559");
        assert_eq!(rooms[0].nick, "玫玫小张帝");
        assert_eq!(rooms[0].title, "到美之声文化传媒大舞台了");
        assert_eq!(rooms[0].online, "1万+");
        assert_eq!(rooms[1].rid, "544590831076");
        assert_eq!(rooms[1].online, "", "空在线人数要保留为空串");
    }

    /// 标题里带引号时，JS 层会出现 `\\\"`，反转义不能破坏 JSON。
    #[test]
    fn parses_title_with_quotes() {
        let page = r#"<script>self.__pace_f.push([1,"{\"roomsData\":{\"count\":1,\"data\":[{\"room\":{\"title\":\"他说\\\"你好\\\"\",\"owner\":{\"nickname\":\"A\"},\"stats\":{\"user_count_str\":\"42\"}},\"web_rid\":\"123\"}]}}"])</script>"#;
        let rooms = parse(page);
        assert_eq!(rooms.len(), 1, "{rooms:?}");
        assert_eq!(rooms[0].title, "他说\"你好\"");
        assert_eq!(rooms[0].nick, "A");
    }

    #[test]
    fn ignores_pages_without_rooms() {
        assert!(parse("").is_empty());
        assert!(parse("<html><body>登录</body></html>").is_empty());
        assert!(
            parse(r#"<script>self.__pace_f.push([1,"{\"roomsData\":null}"])</script>"#).is_empty()
        );
    }

    #[test]
    fn categories_are_the_eight_primary_ones() {
        assert_eq!(CATEGORIES.len(), 8);
        let names: Vec<_> = CATEGORIES.iter().map(|c| c.name).collect();
        assert_eq!(names[0], "聊天");
        assert_eq!(names[2], "游戏");
        assert_eq!(names[7], "运动");
        let mut keys: Vec<_> = CATEGORIES.iter().map(|c| c.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 8, "分区 key 不能重复");
        assert!(keys.iter().all(|k| k.starts_with("4_10")));
    }

    fn tmp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "dy-discover-{}-{}-{tag}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn sample() -> Vec<RoomCard> {
        vec![RoomCard {
            rid: "40132179559".into(),
            nick: "玫玫小张帝".into(),
            title: "到美之声".into(),
            online: "1万+".into(),
        }]
    }

    #[test]
    fn cache_roundtrip() {
        let path = tmp_path("roundtrip");
        save_cache(&path, &sample(), 1_700_000_000).unwrap();
        let got = load_cache(&path, 1_700_000_010).expect("刚写的缓存应该能读回");
        assert_eq!(got, sample());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cache_expires_after_ttl() {
        let path = tmp_path("ttl");
        save_cache(&path, &sample(), 1_700_000_000).unwrap();
        assert!(load_cache(&path, 1_700_000_000 + CACHE_TTL_SECS).is_some());
        assert!(
            load_cache(&path, 1_700_000_000 + CACHE_TTL_SECS + 1).is_none(),
            "超过 TTL 应该视为过期"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cache_ignores_corrupt_file() {
        let path = tmp_path("corrupt");
        std::fs::write(&path, b"not json").unwrap();
        assert!(load_cache(&path, 0).is_none());
        let _ = std::fs::remove_file(&path);
    }

    /// 真实网络集成测试：`cargo test -- --ignored`
    #[test]
    #[ignore = "需要网络"]
    fn fetch_live_category() {
        let rooms = fetch("4_103").expect("拉取游戏分区失败");
        assert!(!rooms.is_empty(), "游戏分区没解析到房间");
        assert!(
            rooms
                .iter()
                .all(|r| r.rid.chars().all(|c| c.is_ascii_digit())),
            "房间号应该是纯数字"
        );
        assert!(rooms.iter().any(|r| !r.nick.is_empty()));
    }
}
