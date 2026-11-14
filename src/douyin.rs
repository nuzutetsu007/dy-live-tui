//! 抖音直播间解析：web_rid → 带签名的拉流直链（纯 Rust，无外部命令参与解析）
//!
//! 流程（与 shell/python 版一致）：
//!   1. GET https://live.douyin.com/<rid>            —— 只为拿 ttwid cookie（可选，失败不影响）
//!   2. GET https://live.douyin.com/webcast/room/web/enter/?...&web_rid=<rid>
//!      → data.data[0].stream_url.hls_pull_url_map / flv_pull_url（带 sign）
//!   注意：页面内嵌的地址没有 sign，直接拉会被 CDN 403。

use serde_json::Value;
use std::time::{Duration, Instant};

pub const UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36";

/// 质量档，按画质从高到低
pub const QUALITIES: [&str; 4] = ["FULL_HD1", "HD1", "SD2", "SD1"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proto {
    Hls,
    Flv,
}

impl Proto {
    pub fn as_str(self) -> &'static str {
        match self {
            Proto::Hls => "HLS",
            Proto::Flv => "FLV",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Room {
    pub rid: String,
    pub nick: String,
    pub title: String,
    pub online: String,
    pub status: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct Probe {
    pub done: bool,
    pub code: u16,
    pub ms: u128,
}

impl Probe {
    pub const PENDING: Probe = Probe {
        done: false,
        code: 0,
        ms: 0,
    };

    pub fn ok(self) -> bool {
        self.code == 200 || self.code == 206
    }
}

#[derive(Clone, Debug)]
pub struct Candidate {
    /// 接口里的档位 key，如 `origin` / `FULL_HD1`
    pub quality: String,
    /// 中文档位名，如「原画」「蓝光」（老接口没有这个字段时为空）
    pub name: String,
    /// 分辨率，如 `1920x1080`
    pub res: String,
    /// 该档位标称帧率，0 表示未知
    pub fps: u32,
    pub proto: Proto,
    pub url: String,
    pub probe: Probe,
}

impl Candidate {
    pub fn label(&self) -> String {
        let mut s = self.display_name();
        if !self.res.is_empty() {
            s.push(' ');
            s.push_str(&self.res);
            if self.fps > 0 {
                s.push_str(&format!("@{}fps", self.fps));
            }
        }
        s.push(' ');
        s.push_str(self.proto.as_str());
        s
    }

    /// 中文名优先；没有就用 key + 映射
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            self.name.clone()
        } else {
            format!("{} {}", self.quality, quality_cn(&self.quality))
        }
    }
}

/// 老接口的档位 key → 中文
pub fn quality_cn(q: &str) -> &'static str {
    match q {
        "origin" => "原画",
        "FULL_HD1" => "蓝光",
        "HD1" => "高清",
        "SD2" => "标清",
        "SD1" => "流畅",
        _ => "",
    }
}

/// 接受 `456497901213`、`https://live.douyin.com/456497901213?x=1` 之类的输入
pub fn normalize_rid(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let tail = s.rsplit('/').next().unwrap_or(s);
    let tail = tail.split(['?', '&']).next().unwrap_or(tail);
    if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
        Some(tail.to_string())
    } else {
        None
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .user_agent(UA)
        .build()
}

fn ttwid_from(resp: &ureq::Response) -> Option<String> {
    for v in resp.all("set-cookie") {
        if let Some(rest) = v.trim().strip_prefix("ttwid=") {
            return Some(rest.split(';').next().unwrap_or(rest).to_string());
        }
    }
    None
}

fn fetch_json(rid: &str, cookie: &str) -> Result<Value, String> {
    let api = format!(
        "https://live.douyin.com/webcast/room/web/enter/?aid=6383&app_name=douyin_web&live_id=1\
         &device_platform=web&language=zh-CN&enter_from=web_live&cookie_enabled=true\
         &screen_width=1920&screen_height=1080&browser_language=zh-CN&browser_platform=Linux\
         &browser_name=Chrome&browser_version=157.0.0.0&msToken=&web_rid={rid}"
    );
    let mut req = agent(Duration::from_secs(10))
        .get(&api)
        .set("Referer", &format!("https://live.douyin.com/{rid}"))
        .set("Accept", "application/json, text/plain, */*");
    if !cookie.is_empty() {
        req = req.set("Cookie", cookie);
    }
    let resp = req.call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("接口返回 HTTP {code}（可能被风控）"),
        other => format!("接口请求失败: {other}"),
    })?;
    let body = resp
        .into_string()
        .map_err(|e| format!("读取响应失败: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("JSON 解析失败: {e}"))
}

fn str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

#[allow(clippy::too_many_arguments)]
fn push_cand(
    list: &mut Vec<Candidate>,
    quality: &str,
    name: &str,
    res: &str,
    fps: u32,
    proto: Proto,
    url: Option<String>,
) {
    let Some(url) = url else { return };
    if url.is_empty() {
        return;
    }
    if list.iter().any(|c| c.url == url) {
        return;
    }
    list.push(Candidate {
        quality: quality.to_string(),
        name: name.to_string(),
        res: res.to_string(),
        fps,
        proto,
        url,
        probe: Probe::PENDING,
    });
}

/// 新版接口：`stream_url.live_core_sdk_data.pull_data` 里有完整档位表，
/// 比 `hls_pull_url_map` 多出「原画（origin / _or4，60fps）」，并带中文名、分辨率、帧率。
/// 返回的列表按 level 从高到低（原画在最前），每档 HLS 在前、FLV 在后。
fn sdk_candidates(stream: &Value) -> Vec<Candidate> {
    let pull = &stream["live_core_sdk_data"]["pull_data"];
    let Some(qualities) = pull["options"]["qualities"].as_array() else {
        return Vec::new();
    };
    let Some(stream_data) = pull["stream_data"]
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
    else {
        return Vec::new();
    };
    let Some(data) = stream_data["data"].as_object() else {
        return Vec::new();
    };

    // (level, name, res, fps, key)
    let mut tiers: Vec<(i64, String, String, u32, String)> = Vec::new();
    for q in qualities {
        let key = str_at(q, "sdk_key").unwrap_or_default();
        if key.is_empty() || q["disable"].as_i64().unwrap_or(0) != 0 {
            continue;
        }
        let Some(node) = data.get(&key) else { continue };
        let main = &node["main"];
        if str_at(main, "hls").is_none() && str_at(main, "flv").is_none() {
            continue;
        }
        let name = str_at(q, "name").unwrap_or_else(|| key.clone());
        // 少数房间给 H.265，标出来（mpv 能放，但用户应该知道）
        let codec = str_at(q, "v_codec").unwrap_or_default();
        let name = if codec.is_empty() || codec == "264" {
            name
        } else {
            format!(
                "{name} H.{}",
                if codec == "265" {
                    "265"
                } else {
                    codec.as_str()
                }
            )
        };
        tiers.push((
            q["level"].as_i64().unwrap_or(0),
            name,
            str_at(q, "resolution").unwrap_or_default(),
            q["fps"].as_u64().unwrap_or(0) as u32,
            key,
        ));
    }
    tiers.sort_by(|a, b| b.0.cmp(&a.0));

    let mut list = Vec::new();
    for (_, name, res, fps, key) in tiers {
        let main = &data[&key]["main"];
        push_cand(
            &mut list,
            &key,
            &name,
            &res,
            fps,
            Proto::Hls,
            str_at(main, "hls"),
        );
        push_cand(
            &mut list,
            &key,
            &name,
            &res,
            fps,
            Proto::Flv,
            str_at(main, "flv"),
        );
    }
    list
}

/// 解析直播间，返回房间信息 + 候选直链（HLS 优先、按请求质量在前的顺序排列）
pub fn resolve(rid: &str) -> Result<(Room, Vec<Candidate>), String> {
    // 1) 尽量拿一个 ttwid；拿不到也继续（接口通常不需要）
    let cookie = match agent(Duration::from_secs(4))
        .get(&format!("https://live.douyin.com/{rid}"))
        .call()
    {
        Ok(resp) => ttwid_from(&resp)
            .map(|t| format!("ttwid={t}"))
            .unwrap_or_default(),
        Err(_) => String::new(),
    };

    // 2) 拉房间数据（空数据/风控时重试 3 次）
    let mut last_err = String::new();
    let mut json: Option<Value> = None;
    for attempt in 1..=3 {
        match fetch_json(rid, &cookie) {
            Ok(v) => {
                let status = v["data"]["data"][0]["status"].as_i64().unwrap_or(0);
                if status == 2 {
                    json = Some(v);
                    break;
                }
                last_err = format!("status={status}");
                if status == 0 || status == 4 {
                    // 不存在 / 已下播，重试无意义
                    json = Some(v);
                    break;
                }
            }
            Err(e) => last_err = e,
        }
        if attempt < 3 {
            std::thread::sleep(Duration::from_millis(700 * attempt as u64));
        }
    }
    let json = json.ok_or_else(|| format!("解析失败：{last_err}"))?;

    let d = &json["data"]["data"][0];
    let status = d["status"].as_i64().unwrap_or(0);
    let room = Room {
        rid: rid.to_string(),
        nick: str_at(&d["owner"], "nickname")
            .or_else(|| str_at(&json["data"]["user"], "nickname"))
            .unwrap_or_else(|| "未知".into()),
        title: str_at(d, "title").unwrap_or_default(),
        online: str_at(d, "user_count_str").unwrap_or_else(|| "0".into()),
        status,
    };
    if status != 2 {
        return Err(match status {
            4 => format!("{} 已下播", room.nick),
            0 => format!("房间 {rid} 不存在 / 未开播 / 被风控"),
            other => format!("当前不可播（status={other}）"),
        });
    }

    let stream = &d["stream_url"];
    let hls_map = &stream["hls_pull_url_map"];
    let flv_map = &stream["flv_pull_url"];

    // 优先用新版档位表（含原画 60fps）；老房间没有就退回 hls_pull_url_map
    let mut cands = sdk_candidates(stream);

    if cands.is_empty() {
        // 质量顺序：请求的档位 → 其余档位（含接口额外给的档位）
        let mut order: Vec<String> = QUALITIES.iter().map(|q| q.to_string()).collect();
        for map in [hls_map, flv_map] {
            if let Some(obj) = map.as_object() {
                for k in obj.keys() {
                    let k = k.to_string();
                    if !order.contains(&k) {
                        order.push(k);
                    }
                }
            }
        }

        for q in &order {
            let cn = quality_cn(q);
            push_cand(&mut cands, q, cn, "", 0, Proto::Hls, str_at(hls_map, q));
            push_cand(&mut cands, q, cn, "", 0, Proto::Flv, str_at(flv_map, q));
        }
        // 兜底：顶层单一 hls 地址
        push_cand(
            &mut cands,
            "default",
            "",
            "",
            0,
            Proto::Hls,
            str_at(stream, "hls_pull_url"),
        );
    }

    if cands.is_empty() {
        return Err("没解析到任何拉流地址（VR / 连麦等模式 web 端可能不支持）".into());
    }
    Ok((room, cands))
}

/// 探测直链可用性：发 Range 请求，只看响应头/状态码（不下载内容）
pub fn probe(url: &str) -> Probe {
    let start = Instant::now();
    let result = agent(Duration::from_secs(5))
        .get(url)
        .set("Range", "bytes=0-2047")
        .call();
    let ms = start.elapsed().as_millis();
    match result {
        Ok(resp) => Probe {
            done: true,
            code: resp.status(),
            ms,
        },
        Err(ureq::Error::Status(code, _)) => Probe {
            done: true,
            code,
            ms,
        },
        Err(_) => Probe {
            done: true,
            code: 0,
            ms,
        },
    }
}

/// 调起 mpv 播放（阻塞直到 mpv 退出）
pub fn play_mpv(url: &str, title: &str) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new("mpv")
        .arg("--ytdl=no")
        .arg(format!("--force-media-title={title}"))
        .arg(url)
        .status()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rid_plain() {
        assert_eq!(
            normalize_rid("456497901213").as_deref(),
            Some("456497901213")
        );
    }

    #[test]
    fn rid_url() {
        assert_eq!(
            normalize_rid("https://live.douyin.com/456497901213?enter_from=web").as_deref(),
            Some("456497901213")
        );
    }

    #[test]
    fn rid_rejects() {
        assert!(normalize_rid("abc").is_none());
        assert!(normalize_rid("").is_none());
        assert!(normalize_rid("https://live.douyin.com/self").is_none());
    }

    #[test]
    fn probe_flags() {
        assert!(Probe {
            done: true,
            code: 200,
            ms: 1
        }
        .ok());
        assert!(Probe {
            done: true,
            code: 206,
            ms: 1
        }
        .ok());
        assert!(!Probe {
            done: true,
            code: 403,
            ms: 1
        }
        .ok());
        assert!(!Probe::PENDING.ok());
    }

    #[test]
    fn sdk_ladder_prefers_origin() {
        let stream_data = serde_json::json!({
            "data": {
                "ld":  { "main": { "hls": "http://x/ld.m3u8",  "flv": "http://x/ld.flv" } },
                "hd":  { "main": { "hls": "http://x/hd.m3u8",  "flv": "http://x/hd.flv" } },
                "uhd": { "main": { "hls": "http://x/uhd.m3u8", "flv": "" } },
                "origin": { "main": { "hls": "http://x/or4.m3u8", "flv": "http://x/or4.flv" } }
            }
        })
        .to_string();
        let stream = serde_json::json!({
            "live_core_sdk_data": {
                "pull_data": {
                    "options": {
                        "qualities": [
                            { "name": "标清", "sdk_key": "ld",  "v_codec": "264", "resolution": "960x540",  "level": 1, "fps": 25, "disable": 0 },
                            { "name": "超清", "sdk_key": "hd",  "v_codec": "265", "resolution": "1280x720", "level": 3, "fps": 30, "disable": 0 },
                            { "name": "蓝光", "sdk_key": "uhd", "v_codec": "264", "resolution": "1920x1080","level": 4, "fps": 45, "disable": 0 },
                            { "name": "原画", "sdk_key": "origin", "v_codec": "264", "resolution": "1920x1080", "level": 5, "fps": 59, "disable": 0 }
                        ]
                    },
                    "stream_data": stream_data
                }
            }
        });

        let cands = sdk_candidates(&stream);
        assert_eq!(cands.len(), 7, "4 档 × 2 协议 - 1 个空 flv");
        assert_eq!(cands[0].quality, "origin");
        assert_eq!(cands[0].proto, Proto::Hls);
        assert_eq!(cands[0].name, "原画");
        assert_eq!(cands[0].res, "1920x1080");
        assert_eq!(cands[0].fps, 59);
        assert_eq!(cands[1].quality, "origin");
        assert_eq!(cands[1].proto, Proto::Flv);
        // H.265 档位要标出来
        let hd = cands.iter().find(|c| c.quality == "hd").unwrap();
        assert!(hd.name.contains("H.265"), "name = {}", hd.name);
        // uhd 只有 hls，不该出现 flv
        assert!(!cands
            .iter()
            .any(|c| c.quality == "uhd" && c.proto == Proto::Flv));
    }

    /// 真实网络集成测试：`cargo test -- --ignored`
    /// 需要目标房间此刻正在直播（默认用样本房间 456497901213）。
    #[test]
    #[ignore = "需要网络 + 房间正在直播"]
    fn resolve_and_probe_live_room() {
        let rid = std::env::var("DY_TEST_RID").unwrap_or_else(|_| "456497901213".into());
        let (room, cands) = resolve(&rid).expect("解析直播间失败（房间没开播或网络不通？）");
        assert_eq!(room.rid, rid);
        assert_eq!(room.status, 2);
        assert!(!room.nick.is_empty());
        assert!(!cands.is_empty(), "没有候选直链");
        assert!(cands.iter().any(|c| c.proto == Proto::Hls), "缺少 HLS 候选");

        // 至少有一条能通
        let probes: Vec<Probe> = cands.iter().map(|c| probe(&c.url)).collect();
        assert!(
            probes.iter().any(|p| p.ok()),
            "所有候选都探测失败: {:?}",
            probes.iter().map(|p| p.code).collect::<Vec<_>>()
        );

        // 质量顺序：HLS 档位应按 FULL_HD1 优先出现
        let first_hls = cands.iter().find(|c| c.proto == Proto::Hls).unwrap();
        assert_eq!(first_hls.quality, "FULL_HD1");
    }
}
