//! 按分区分页列出正在直播的房间（纯文本）。
//!
//! 主数据源是 `https://live.douyin.com/webcast/web/partition/detail/room/v2/`：
//! 支持真正的 `offset` 分页，每个分区最多 200 个房间，但需要 `a_bogus` 签名。
//! 签名失败时退回 `https://live.douyin.com/categorynew/<key>` 的 SSR JSON
//! （只有 15 个热门房间，只作为兜底）。

use serde_json::Value;

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
    /// 在线人数（用于排序），未知为 0
    pub viewers: u64,
}

/// 一页签名接口的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page {
    pub rooms: Vec<RoomCard>,
    /// 下一页应该请求的 offset
    pub next_offset: usize,
    /// 是否已经到最后一页
    pub exhausted: bool,
    /// 是否退回了 SSR 热榜（此时不能继续翻页）
    pub fallback: bool,
}

/// 每页 15 个，和网页端一致。
pub const PAGE_SIZE: usize = 15;
/// 签名接口每个分区最多给 200 个房间。
pub const MAX_ROOMS: usize = 200;

/// 签名用的 UA：必须与 `abogus::sign` 时完全一致。
pub const SIGN_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// 一级分区（抖音直播页顶部导航里的 8 个）。
#[derive(Clone, Copy, Debug)]
pub struct Category {
    /// URL 里的 key，如 `4_103`
    pub key: &'static str,
    /// 接口里的 partition id，如 `103`
    pub id: &'static str,
    /// 中文名
    pub name: &'static str,
}

pub const CATEGORIES: [Category; 8] = [
    Category {
        key: "4_101",
        id: "101",
        name: "聊天",
    },
    Category {
        key: "4_102",
        id: "102",
        name: "音乐",
    },
    Category {
        key: "4_103",
        id: "103",
        name: "游戏",
    },
    Category {
        key: "4_104",
        id: "104",
        name: "二次元",
    },
    Category {
        key: "4_105",
        id: "105",
        name: "舞蹈",
    },
    Category {
        key: "4_106",
        id: "106",
        name: "文化",
    },
    Category {
        key: "4_107",
        id: "107",
        name: "生活",
    },
    Category {
        key: "4_108",
        id: "108",
        name: "运动",
    },
];

/// 游戏分区的子分区（小主播比例更高）。
#[derive(Clone, Copy, Debug)]
pub struct GameSub {
    pub id: &'static str,
    pub ptype: &'static str,
    pub name: &'static str,
}

pub const GAME_SUBS: [GameSub; 9] = [
    GameSub {
        id: "1011032",
        ptype: "1",
        name: "三角洲行动",
    },
    GameSub {
        id: "1010045",
        ptype: "1",
        name: "王者荣耀",
    },
    GameSub {
        id: "1010017",
        ptype: "1",
        name: "无畏契约",
    },
    GameSub {
        id: "1010014",
        ptype: "1",
        name: "英雄联盟",
    },
    GameSub {
        id: "1010032",
        ptype: "1",
        name: "和平精英",
    },
    GameSub {
        id: "1010146",
        ptype: "1",
        name: "QQ飞车端游",
    },
    GameSub {
        id: "1010037",
        ptype: "1",
        name: "穿越火线",
    },
    GameSub {
        id: "1010026",
        ptype: "1",
        name: "绝地求生",
    },
    GameSub {
        id: "1010042",
        ptype: "1",
        name: "火影忍者手游",
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
            viewers: 0,
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

/// 拉取一页房间（带 5 分钟缓存）。
///
/// `key` 只用于缓存文件名/Referer；`partition`/`ptype` 是接口参数。
/// `force` 为 true 时绕过缓存。
pub fn fetch_page(
    key: &str,
    partition: &str,
    ptype: &str,
    offset: usize,
    force: bool,
) -> Result<Page, String> {
    let path = cache_file(key, offset);
    if !force {
        if let Some(page) = load_cache(&path, now_secs()) {
            return Ok(page);
        }
    }

    match fetch_signed(partition, ptype, offset) {
        Ok(rooms) => {
            let raw = rooms.len();
            let page = Page {
                next_offset: offset + raw,
                exhausted: raw < PAGE_SIZE || offset + raw >= MAX_ROOMS,
                fallback: false,
                rooms,
            };
            let _ = save_cache(&path, &page, now_secs());
            Ok(page)
        }
        Err(err) if offset == 0 => {
            // 签名失败：退回分类页 SSR（只有 15 个热门房间，不能继续翻页）
            let rooms = fetch_ssr(key)
                .map_err(|e| format!("签名接口失败：{err}；回退分类页也失败：{e}"))?;
            let page = Page {
                next_offset: rooms.len(),
                exhausted: true,
                fallback: true,
                rooms,
            };
            let _ = save_cache(&path, &page, now_secs());
            Ok(page)
        }
        Err(err) => Err(err),
    }
}

fn fetch_signed(partition: &str, ptype: &str, offset: usize) -> Result<Vec<RoomCard>, String> {
    let mut last = String::new();
    for attempt in 0..2 {
        match fetch_signed_once(partition, ptype, offset) {
            Ok(rooms) => return Ok(rooms),
            Err(err) => {
                last = err;
                if attempt == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
        }
    }
    Err(last)
}

fn fetch_signed_once(partition: &str, ptype: &str, offset: usize) -> Result<Vec<RoomCard>, String> {
    // 参数顺序必须和签名时完全一致；offset 放最后（与实测可用的请求一致）。
    let params = [
        ("aid", "6383"),
        ("app_name", "douyin_web"),
        ("live_id", "1"),
        ("device_platform", "web"),
        ("language", "zh-CN"),
        ("enter_from", "link_share"),
        ("cookie_enabled", "true"),
        ("screen_width", "1980"),
        ("screen_height", "1080"),
        ("browser_language", "zh-CN"),
        ("browser_platform", "Win32"),
        ("browser_name", "Edge"),
        ("browser_version", "125.0.0.0"),
        ("browser_online", "true"),
        ("count", "15"),
        ("partition", partition),
        ("partition_type", ptype),
        ("req_from", "2"),
    ];
    let mut query = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    query.push_str(&format!("&offset={offset}"));

    let sig = crate::abogus::sign(&query, SIGN_UA);
    let url = format!(
        "https://live.douyin.com/webcast/web/partition/detail/room/v2/?{query}&a_bogus={sig}"
    );

    let agent = signed_agent(std::time::Duration::from_secs(15));
    let cookie = home_cookie();
    let mut req = agent
        .get(&url)
        .set("Referer", "https://live.douyin.com/")
        .set("Accept", "application/json, text/plain, */*");
    if !cookie.is_empty() {
        req = req.set("Cookie", &cookie);
    }
    let resp = req.call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("签名接口返回 HTTP {code}（可能被风控）"),
        other => format!("签名接口请求失败：{other}"),
    })?;
    let body = resp
        .into_string()
        .map_err(|e| format!("读取签名接口响应失败：{e}"))?;
    if body.trim().is_empty() {
        return Err("签名接口返回空响应（a_bogus 可能失效或被风控）".into());
    }
    let value: Value =
        serde_json::from_str(&body).map_err(|e| format!("签名接口 JSON 解析失败：{e}"))?;
    let status = value["status_code"].as_i64().unwrap_or(-1);
    if status != 0 {
        let msg = value["status_msg"].as_str().unwrap_or_default();
        return Err(format!("签名接口 status={status} {msg}"));
    }
    Ok(parse_api_value(&value))
}

/// 解析 `/partition/detail/room/v2/` 的 JSON（单测用）。
#[cfg(test)]
pub fn parse_api(body: &str) -> Vec<RoomCard> {
    match serde_json::from_str::<Value>(body) {
        Ok(value) => parse_api_value(&value),
        Err(_) => Vec::new(),
    }
}

fn parse_api_value(value: &Value) -> Vec<RoomCard> {
    let Some(items) = value["data"]["data"].as_array() else {
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
        let room = &item["room"];
        let nick = str_at(&room["owner"], "nickname").unwrap_or_default();
        let title = str_at(room, "title").unwrap_or_default();
        if nick.is_empty() && title.is_empty() {
            continue;
        }
        let viewers = room["room_view_stats"]["display_value"]
            .as_u64()
            .or_else(|| {
                room["room_view_stats"]["display_value"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
            })
            .unwrap_or(0);
        let online = str_at(&room["stats"], "user_count_str")
            .filter(|s| !s.is_empty())
            .or_else(|| str_at(&room["room_view_stats"], "display_short"))
            .or_else(|| {
                let v = &room["room_view_stats"]["display_value"];
                v.as_u64()
                    .map(|n| n.to_string())
                    .or_else(|| v.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_default();
        rooms.push(RoomCard {
            rid: rid.to_string(),
            nick,
            title,
            online,
            viewers,
        });
    }
    rooms
}

fn str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn signed_agent(timeout: std::time::Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .user_agent(SIGN_UA)
        .build()
}

fn home_cookie() -> String {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some(cookie) = guard.as_ref() {
            return cookie.clone();
        }
    }
    let cookie = fetch_home_cookie();
    if !cookie.is_empty() {
        if let Ok(mut guard) = cache.lock() {
            *guard = Some(cookie.clone());
        }
    }
    cookie
}

fn fetch_home_cookie() -> String {
    let agent = signed_agent(std::time::Duration::from_secs(6));
    match agent.get("https://live.douyin.com/").call() {
        Ok(resp) => ttwid_from(&resp)
            .map(|t| format!("ttwid={t}"))
            .unwrap_or_default(),
        Err(_) => String::new(),
    }
}

fn ttwid_from(resp: &ureq::Response) -> Option<String> {
    for v in resp.all("set-cookie") {
        if let Some(rest) = v.trim().strip_prefix("ttwid=") {
            return Some(rest.split(';').next().unwrap_or(rest).to_string());
        }
    }
    None
}

fn fetch_ssr(key: &str) -> Result<Vec<RoomCard>, String> {
    let url = format!("https://live.douyin.com/categorynew/{key}");
    let html = get_page(&url, &url)?;
    let rooms = parse(&html);
    if rooms.is_empty() {
        return Err("这个分区暂时没解析到房间（可能被风控，或页面结构变了）".into());
    }
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

fn cache_file(key: &str, offset: usize) -> std::path::PathBuf {
    let safe = key.replace(':', "_");
    crate::history::cache_dir().join(format!("discover-{safe}-{offset}.json"))
}

fn load_cache(path: &std::path::Path, now: u64) -> Option<Page> {
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
            viewers: item.get("viewers").and_then(|v| v.as_u64()).unwrap_or(0),
        });
    }
    Some(Page {
        next_offset: value
            .get("next_offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(rooms.len() as u64) as usize,
        exhausted: value
            .get("exhausted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        fallback: value
            .get("fallback")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        rooms,
    })
}

fn save_cache(path: &std::path::Path, page: &Page, now: u64) -> std::io::Result<()> {
    let items: Vec<serde_json::Value> = page
        .rooms
        .iter()
        .map(|r| {
            serde_json::json!({
                "rid": r.rid,
                "nick": r.nick,
                "title": r.title,
                "online": r.online,
                "viewers": r.viewers,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "fetched_at": now,
        "next_offset": page.next_offset,
        "exhausted": page.exhausted,
        "fallback": page.fallback,
        "rooms": items,
    });
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
            viewers: 10_000,
        }]
    }

    fn sample_page() -> Page {
        Page {
            rooms: sample(),
            next_offset: 15,
            exhausted: false,
            fallback: false,
        }
    }

    #[test]
    fn cache_roundtrip() {
        let path = tmp_path("roundtrip");
        save_cache(&path, &sample_page(), 1_700_000_000).unwrap();
        let got = load_cache(&path, 1_700_000_010).expect("刚写的缓存应该能读回");
        assert_eq!(got, sample_page());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cache_expires_after_ttl() {
        let path = tmp_path("ttl");
        save_cache(&path, &sample_page(), 1_700_000_000).unwrap();
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

    const API_PAGE: &str = r#"{
        "status_code": 0,
        "data": {
            "data": [
                {
                    "web_rid": "217903139564",
                    "room": {
                        "title": "无畏契约上分",
                        "owner": {"nickname": "winter"},
                        "room_view_stats": {"display_value": 825, "display_short": "825"},
                        "stats": {"user_count_str": "825"}
                    }
                },
                {
                    "web_rid": "217903139564",
                    "room": {
                        "title": "重复卡片",
                        "owner": {"nickname": "winter"},
                        "room_view_stats": {"display_value": 999}
                    }
                },
                {
                    "web_rid": "123456",
                    "room": {
                        "title": "",
                        "owner": {"nickname": "小白"},
                        "room_view_stats": {"display_value": "44"}
                    }
                }
            ]
        }
    }"#;

    #[test]
    fn parses_signed_api_page() {
        let rooms = parse_api(API_PAGE);
        assert_eq!(rooms.len(), 2, "同一页里重复的 web_rid 要去重: {rooms:?}");
        assert_eq!(rooms[0].rid, "217903139564");
        assert_eq!(rooms[0].nick, "winter");
        assert_eq!(rooms[0].title, "无畏契约上分");
        assert_eq!(rooms[0].online, "825");
        assert_eq!(rooms[0].viewers, 825);
        assert_eq!(rooms[1].rid, "123456");
        assert_eq!(rooms[1].viewers, 44);
        assert_eq!(rooms[1].online, "44", "没有 stats 时用 display_value 兜底");
    }

    /// 真实网络集成测试：`cargo test -- --ignored`
    #[test]
    #[ignore = "需要网络"]
    fn fetch_live_category() {
        let page = fetch_page("4_103", "103", "4", 0, true).expect("拉取游戏分区失败");
        assert!(!page.rooms.is_empty(), "游戏分区没解析到房间");
        assert!(
            page.rooms
                .iter()
                .all(|r| r.rid.chars().all(|c| c.is_ascii_digit())),
            "房间号应该是纯数字"
        );
        assert!(page.rooms.iter().any(|r| !r.nick.is_empty()));
        assert!(page.rooms.iter().any(|r| r.viewers > 0));
    }

    /// 深层子分区应该能翻到更小的主播。
    #[test]
    #[ignore = "需要网络"]
    fn fetch_deep_sub_partition_page() {
        let page =
            fetch_page("4_103:1010017", "1010017", "1", 45, true).expect("拉取无畏契约第 4 页失败");
        assert_eq!(page.rooms.len(), PAGE_SIZE, "第 4 页应该是满页");
        assert!(!page.exhausted);
        let mut rids: Vec<_> = page.rooms.iter().map(|r| r.rid.clone()).collect();
        rids.sort();
        rids.dedup();
        assert_eq!(rids.len(), PAGE_SIZE, "同一页不该有重复房间");
        let max = page.rooms.iter().map(|r| r.viewers).max().unwrap_or(0);
        let min = page.rooms.iter().map(|r| r.viewers).min().unwrap_or(0);
        assert!(max < 50_000, "第 4 页不应该还是顶级大房间，最大 {max}");
        assert!(min < 1_000, "第 4 页应该已经出现小主播，最小 {min}");
    }
}
