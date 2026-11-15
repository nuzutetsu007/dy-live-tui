use crate::discover::{self, RoomCard};
use crate::douyin::{self, Candidate, Probe, Room};
use crate::history::History;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

pub enum Msg {
    Found { room: Room, cands: Vec<Candidate> },
    Probed { idx: usize, probe: Probe },
    Failed(String),
    Discovered { key: String, rooms: Vec<RoomCard> },
    DiscoverFailed { key: String, err: String },
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Input,
    Loading,
    Quality,
    Discover,
}

/// 找直播页的状态：分区下标、当前分区的房间、列表光标。
#[derive(Default)]
pub struct Discover {
    pub sel: usize,
    pub rooms: Vec<RoomCard>,
    pub cursor: usize,
    pub loading: bool,
    pub err: Option<String>,
    /// 已拉取（或正在拉取）的分区 key，用于避免重复请求
    pub loaded_key: Option<String>,
}

pub struct App {
    pub input: String,
    pub screen: Screen,
    pub room: Option<Room>,
    pub cands: Vec<Candidate>,
    pub sel: usize,
    pub msg: String,
    pub error: Option<String>,
    pub spinner: usize,
    pub dry_run: bool,
    pub quit: bool,
    /// (url, mpv window title) —— 由 main 取出后暂停 TUI 去播放
    pub pending_play: Option<(String, String)>,
    /// 成功播放过的直播间（最近在前）
    pub history: History,
    /// 输入页里历史列表的高亮项
    pub hist_sel: Option<usize>,
    /// 输入页里用 Space 标记、准备批量删除的房间号
    pub hist_marked: std::collections::HashSet<String>,
    /// 历史列表的 vim 导航模式（↑/↓ 进入；j/k/g/G/x/V/d 生效）
    pub hist_nav: bool,
    /// 找直播页
    pub discover: Discover,
    rx: Option<Receiver<Msg>>,
}

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// 并发探测（最多 4 路），结果通过 channel 逐条回传
fn probe_many(urls: Vec<String>, tx: &mpsc::Sender<Msg>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let n = urls.len();
    if n == 0 {
        return;
    }
    let next = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|s| {
        for _ in 0..n.min(4) {
            let next = Arc::clone(&next);
            let tx = tx.clone();
            let urls = &urls;
            s.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= n {
                    break;
                }
                let probe = douyin::probe(&urls[i]);
                if tx.send(Msg::Probed { idx: i, probe }).is_err() {
                    break;
                }
            });
        }
    });
}

impl App {
    pub fn new(dry_run: bool, initial: Option<String>) -> Self {
        let history = History::load();
        let had_initial = initial.as_ref().is_some_and(|s| !s.trim().is_empty());
        let mut input = initial.unwrap_or_default();
        // 没带参数时，直接把上次看过的房间填进输入框，回车即可重新打开
        let mut hist_sel = None;
        if !had_initial && !history.is_empty() {
            input = history.entries()[0].rid.clone();
            hist_sel = Some(0);
        }

        let mut app = App {
            input,
            screen: Screen::Input,
            room: None,
            cands: Vec::new(),
            sel: 0,
            msg: String::new(),
            error: None,
            spinner: 0,
            dry_run,
            quit: false,
            pending_play: None,
            history,
            hist_sel,
            hist_marked: std::collections::HashSet::new(),
            hist_nav: false,
            discover: Discover::default(),
            rx: None,
        };
        if had_initial {
            app.start_resolve();
        }
        app
    }

    pub fn spin(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
    }

    pub fn spinner_char(&self) -> &'static str {
        SPINNER[self.spinner % SPINNER.len()]
    }

    pub fn is_probing(&self) -> bool {
        self.cands.iter().any(|c| !c.probe.done)
    }

    /// 启动后台解析线程：先回房间+候选，再逐个回探测结果
    pub fn start_resolve(&mut self) {
        let Some(rid) = douyin::normalize_rid(&self.input) else {
            self.error = Some(
                "房间号或链接不对（例：456497901213 或 https://live.douyin.com/456497901213）"
                    .into(),
            );
            self.screen = Screen::Input;
            return;
        };
        self.error = None;
        self.room = None;
        self.cands.clear();
        self.sel = 0;
        self.hist_nav = false;
        self.screen = Screen::Loading;
        self.msg = format!("解析直播间 {rid} …");

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        thread::spawn(move || match douyin::resolve(&rid) {
            Ok((room, cands)) => {
                if tx
                    .send(Msg::Found {
                        room,
                        cands: cands.clone(),
                    })
                    .is_err()
                {
                    return;
                }
                let urls: Vec<String> = cands.iter().map(|c| c.url.clone()).collect();
                probe_many(urls, &tx);
                let _ = tx.send(Msg::Done);
            }
            Err(e) => {
                let _ = tx.send(Msg::Failed(e));
            }
        });
    }

    /// 打开「找直播」页；当前分区没数据时顺带拉一次
    pub fn open_discover(&mut self) {
        self.error = None;
        self.screen = Screen::Discover;
        let key = discover::CATEGORIES[self.discover.sel].key;
        if self.discover.loaded_key.as_deref() != Some(key) {
            self.load_category(self.discover.sel, false);
        }
    }

    /// 拉取某个分区的房间（force 为 true 时绕过缓存）
    fn load_category(&mut self, idx: usize, force: bool) {
        let idx = idx.min(discover::CATEGORIES.len() - 1);
        let cat = discover::CATEGORIES[idx];
        self.discover.sel = idx;
        self.discover.rooms.clear();
        self.discover.cursor = 0;
        self.discover.err = None;
        self.discover.loading = true;
        self.discover.loaded_key = Some(cat.key.to_string());
        self.msg = format!("拉取「{}」分区…", cat.name);

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let key = cat.key.to_string();

        // 单测里不发真实请求，保证测试离线、可重复
        if cfg!(test) {
            let _ = tx.send(Msg::Discovered {
                key,
                rooms: Vec::new(),
            });
            return;
        }

        thread::spawn(move || {
            let result = if force {
                discover::fetch_fresh(&key)
            } else {
                discover::fetch(&key)
            };
            let msg = match result {
                Ok(rooms) => Msg::Discovered { key, rooms },
                Err(err) => Msg::DiscoverFailed { key, err },
            };
            let _ = tx.send(msg);
        });
    }

    fn key_discover(&mut self, key: KeyEvent) {
        let n = self.discover.rooms.len();
        match key.code {
            KeyCode::Esc => {
                self.screen = Screen::Input;
                self.msg.clear();
            }
            KeyCode::Left | KeyCode::Char('h') if self.discover.sel > 0 => {
                self.load_category(self.discover.sel - 1, false);
            }
            KeyCode::Right | KeyCode::Char('l')
                if self.discover.sel + 1 < discover::CATEGORIES.len() =>
            {
                self.load_category(self.discover.sel + 1, false);
            }
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                self.discover.cursor = self.discover.cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if self.discover.cursor + 1 < n => {
                self.discover.cursor += 1;
            }
            KeyCode::Char('g') => self.discover.cursor = 0,
            KeyCode::Char('G') => self.discover.cursor = n.saturating_sub(1),
            KeyCode::Char('r') => self.load_category(self.discover.sel, true),
            KeyCode::Enter => {
                if let Some(room) = self.discover.rooms.get(self.discover.cursor) {
                    self.input = room.rid.clone();
                    self.start_resolve();
                }
            }
            _ => {}
        }
    }

    /// 重新探测当前候选列表
    pub fn reprobe(&mut self) {
        let urls: Vec<String> = self.cands.iter().map(|c| c.url.clone()).collect();
        if urls.is_empty() {
            return;
        }
        for c in self.cands.iter_mut() {
            c.probe = Probe::PENDING;
        }
        self.msg = "重新探测直链…".into();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        thread::spawn(move || {
            probe_many(urls, &tx);
            let _ = tx.send(Msg::Done);
        });
    }

    pub fn poll(&mut self) {
        loop {
            let Some(rx) = self.rx.as_ref() else { return };
            match rx.try_recv() {
                Ok(Msg::Found { room, cands }) => {
                    self.msg = format!(
                        "{} · {}  (在线 {})  {} 条候选",
                        room.nick,
                        room.title,
                        room.online,
                        cands.len()
                    );
                    self.room = Some(room);
                    self.cands = cands;
                    self.sel = 0;
                    self.screen = Screen::Quality;
                }
                Ok(Msg::Probed { idx, probe }) => {
                    if let Some(c) = self.cands.get_mut(idx) {
                        c.probe = probe;
                    }
                }
                Ok(Msg::Failed(e)) => {
                    self.error = Some(e);
                    self.screen = Screen::Input;
                    self.rx = None;
                    return;
                }
                Ok(Msg::Discovered { key, rooms }) => {
                    // 切分区太快时，丢弃过期结果
                    if self.discover.loaded_key.as_deref() == Some(key.as_str()) {
                        let n = rooms.len();
                        let name = discover::CATEGORIES[self.discover.sel].name;
                        self.discover.rooms = rooms;
                        self.discover.cursor = 0;
                        self.discover.loading = false;
                        self.discover.err = None;
                        self.msg = format!("「{name}」{n} 个直播间 · r 换一批");
                    }
                    self.rx = None;
                    return;
                }
                Ok(Msg::DiscoverFailed { key, err }) => {
                    if self.discover.loaded_key.as_deref() == Some(key.as_str()) {
                        self.discover.loading = false;
                        self.discover.err = Some(err);
                    }
                    self.rx = None;
                    return;
                }
                Ok(Msg::Done) => {
                    self.rx = None;
                    self.msg = "探测完成".into();
                    return;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    return;
                }
            }
        }
    }

    /// 返回 true 表示需要退出 TUI（播放交给 main）
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.screen {
            Screen::Input => self.key_input(key),
            Screen::Loading => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.rx = None;
                    self.screen = Screen::Input;
                    self.msg.clear();
                }
            }
            Screen::Quality => self.key_quality(key),
            Screen::Discover => self.key_discover(key),
        }
    }

    fn key_input(&mut self, key: KeyEvent) {
        if self.hist_nav {
            if self.key_history_nav(key) {
                return;
            }
            // 不是 vim 命令：当作普通输入，回到输入模式
            self.hist_nav = false;
        }

        match key.code {
            KeyCode::Enter => self.start_resolve(),
            KeyCode::Tab => self.open_discover(),
            KeyCode::Esc => self.quit = true,
            KeyCode::Up | KeyCode::Down => {
                let up = key.code == KeyCode::Up;
                if !self.hist_nav {
                    self.hist_nav = true;
                    // 第一次按方向键只进入导航，不跳过当前已高亮的那条
                    if self.hist_sel.is_none() {
                        self.pick_history(up);
                    }
                } else {
                    self.pick_history(up);
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.error = None;
                self.hist_sel = None;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.clear();
                self.error = None;
                self.hist_sel = None;
            }
            KeyCode::Char(c) if c.is_ascii_alphanumeric() || ":/?.=&-_%#".contains(c) => {
                self.input.push(c);
                self.error = None;
                self.hist_sel = None;
            }
            _ => {}
        }
    }

    /// 历史导航模式下的按键；返回 false 表示这不是 vim 命令
    fn key_history_nav(&mut self, key: KeyEvent) -> bool {
        if self.history.is_empty() {
            self.hist_nav = false;
            return false;
        }
        match key.code {
            KeyCode::Esc => {
                self.hist_nav = false;
                self.hist_marked.clear();
            }
            KeyCode::Up | KeyCode::Char('k') => self.pick_history(true),
            KeyCode::Down | KeyCode::Char('j') => self.pick_history(false),
            KeyCode::Char('g') => self.hist_first(),
            KeyCode::Char('G') => self.hist_last(),
            KeyCode::Char('x') | KeyCode::Char(' ') => self.toggle_hist_mark(),
            KeyCode::Char('V') => self.mark_all_history(),
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mark_all_history()
            }
            KeyCode::Delete | KeyCode::Char('d') => self.delete_marked_history(),
            KeyCode::Enter => {
                if let Some(e) = self.hist_sel.and_then(|i| self.history.entries().get(i)) {
                    self.input = e.rid.clone();
                }
                self.start_resolve();
            }
            _ => return false,
        }
        true
    }

    fn hist_first(&mut self) {
        if let Some(e) = self.history.entries().first() {
            self.hist_sel = Some(0);
            self.input = e.rid.clone();
            self.error = None;
        }
    }

    fn hist_last(&mut self) {
        let n = self.history.entries().len();
        if n > 0 {
            self.hist_sel = Some(n - 1);
            self.input = self.history.entries()[n - 1].rid.clone();
            self.error = None;
        }
    }

    /// ↑/↓ 或 j/k 在「最近打开」里选；选中即填入输入框（回车就解析）
    fn pick_history(&mut self, up: bool) {
        let n = self.history.entries().len();
        if n == 0 {
            self.hist_sel = None;
            return;
        }
        let idx = match self.hist_sel {
            None => 0,
            Some(0) if up => 0,
            Some(i) if up => i - 1,
            Some(i) => (i + 1).min(n - 1),
        };
        self.hist_sel = Some(idx);
        self.input = self.history.entries()[idx].rid.clone();
        self.error = None;
    }

    /// Space：标记/取消标记当前历史行（不改输入框）
    fn toggle_hist_mark(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = self
            .hist_sel
            .unwrap_or(0)
            .min(self.history.entries().len() - 1);
        self.hist_sel = Some(idx);
        let rid = self.history.entries()[idx].rid.clone();
        if !self.hist_marked.remove(&rid) {
            self.hist_marked.insert(rid);
        }
    }

    /// Ctrl-A：全选 / 取消全选
    fn mark_all_history(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.hist_marked.len() == self.history.entries().len() {
            self.hist_marked.clear();
        } else {
            self.hist_marked = self
                .history
                .entries()
                .iter()
                .map(|e| e.rid.clone())
                .collect();
        }
    }

    /// Delete / Ctrl-D：删掉标记的行；没标记就删当前行
    fn delete_marked_history(&mut self) {
        let targets: std::collections::HashSet<String> = if !self.hist_marked.is_empty() {
            self.hist_marked.clone()
        } else if let Some(e) = self.hist_sel.and_then(|i| self.history.entries().get(i)) {
            [e.rid.clone()].into_iter().collect()
        } else {
            return;
        };

        let removed = if targets.len() == 1 {
            let rid = targets
                .iter()
                .next()
                .map(String::as_str)
                .unwrap_or_default();
            usize::from(self.history.remove(rid))
        } else {
            self.history.remove_many(&targets)
        };
        self.hist_marked.clear();
        if removed == 0 {
            return;
        }

        // 输入框里正显示被删掉的房间号：换成删除后同一位置的那条
        let input_replaced = targets.contains(&self.input);
        self.hist_sel = if self.history.is_empty() {
            self.hist_nav = false;
            None
        } else {
            Some(
                self.hist_sel
                    .unwrap_or(0)
                    .min(self.history.entries().len() - 1),
            )
        };
        if input_replaced {
            self.input = self
                .hist_sel
                .and_then(|i| self.history.entries().get(i))
                .map(|e| e.rid.clone())
                .unwrap_or_default();
        }
        self.error = None;
        self.msg = format!("已删除 {removed} 条历史");
    }

    fn key_quality(&mut self, key: KeyEvent) {
        let n = self.cands.len();
        match key.code {
            KeyCode::Esc => {
                if self.dry_run {
                    self.quit = true;
                } else {
                    self.screen = Screen::Input;
                }
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                self.sel = (self.sel + n - 1) % n;
            }
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                self.sel = (self.sel + 1) % n;
            }
            KeyCode::Char('g') => self.sel = 0,
            KeyCode::Char('G') => self.sel = n.saturating_sub(1),
            KeyCode::Char('p') => self.reprobe(),
            KeyCode::Char('r') => {
                let input = self.input.clone();
                self.input = input;
                self.start_resolve();
            }
            KeyCode::Enter | KeyCode::Char('f') => {
                let force = key.code == KeyCode::Char('f');
                let Some(c) = self.cands.get(self.sel).cloned() else {
                    return;
                };
                if !c.probe.done {
                    self.msg = "还在探测，稍等一下…".into();
                    return;
                }
                if !c.probe.ok() && !force {
                    self.msg = format!(
                        "{} 探测不通（HTTP {}）；按 f 强制播放，或换一条",
                        c.label(),
                        c.probe.code
                    );
                    return;
                }
                let title = match &self.room {
                    Some(r) => format!("{} · {}", r.nick, r.title),
                    None => "抖音直播".to_string(),
                };
                self.pending_play = Some((c.url.clone(), title));
            }
            _ => {}
        }
    }

    /// 括号粘贴：只保留能构成 URL / 房间号的字符
    pub fn paste(&mut self, s: &str) {
        if self.screen != Screen::Input {
            return;
        }
        for c in s.chars() {
            if c.is_ascii_alphanumeric() || ":/?.=&-_%#".contains(c) {
                self.input.push(c);
            }
        }
        self.error = None;
    }

    /// mpv 退出后的状态回写；只有真正播放成功（退出码 0）才记入历史
    pub fn on_play_done(&mut self, res: std::io::Result<std::process::ExitStatus>) {
        let ok = matches!(&res, Ok(st) if st.success());
        self.msg = match res {
            Ok(st) if st.success() => "已关闭 mpv（0）· 已记入历史".to_string(),
            Ok(st) => format!("mpv 退出码 {:?}", st.code()),
            Err(e) => format!("启动 mpv 失败: {e}"),
        };
        if ok {
            if let Some(r) = &self.room {
                let (rid, nick, title) = (r.rid.clone(), r.nick.clone(), r.title.clone());
                self.history.add(&rid, &nick, &title);
                self.hist_sel = Some(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn exit(code: i32) -> std::process::ExitStatus {
        std::process::ExitStatus::from_raw(code << 8)
    }

    fn tmp_history() -> (App, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "dy-app-hist-{}-{}.tsv",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let mut app = App::new(false, None);
        app.history = History::load_from(&path);
        app.room = Some(Room {
            rid: "456497901213".into(),
            nick: "主播杨树".into(),
            title: "全新3D僵尸毁灭工程".into(),
            online: "3000+".into(),
            status: 2,
        });
        (app, path)
    }

    #[test]
    fn success_records_history() {
        let (mut app, path) = tmp_history();
        app.on_play_done(Ok(exit(0)));
        assert_eq!(app.history.entries().len(), 1);
        assert_eq!(app.history.entries()[0].rid, "456497901213");
        assert_eq!(app.history.entries()[0].nick, "主播杨树");
        assert_eq!(app.hist_sel, Some(0));
        assert!(app.msg.contains("历史"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn failure_does_not_record_history() {
        let (mut app, path) = tmp_history();
        app.on_play_done(Ok(exit(1)));
        assert!(app.history.entries().is_empty(), "失败不该记入历史");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn history_pick_fills_input() {
        let (mut app, path) = tmp_history();
        app.history.add("111111", "A", "a");
        app.history.add("222222", "B", "b");
        app.screen = Screen::Input;
        app.hist_sel = None;

        app.pick_history(true);
        assert_eq!(app.input, "222222", "↑ 从最新一条开始");
        assert_eq!(app.hist_sel, Some(0));

        app.pick_history(false);
        assert_eq!(app.input, "111111");
        assert_eq!(app.hist_sel, Some(1));

        // 打字后脱离历史选择
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('9'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(app.hist_sel, None);
        assert_eq!(app.input, "1111119");
        let _ = std::fs::remove_file(&path);
    }

    // ---- 历史删除 / 多选（vim 键位）----

    fn history_app() -> (App, std::path::PathBuf) {
        let (mut app, path) = tmp_history();
        app.history.add("111", "主播A", "标题A");
        app.history.add("222", "主播B", "标题B");
        app.history.add("333", "主播C", "标题C");
        // 顺序：333 / 222 / 111
        app.input.clear();
        app.hist_sel = None;
        app.hist_nav = false;
        app.screen = Screen::Input;
        (app, path)
    }

    /// ↓ 进入历史导航（vim normal 模式），落在最新一条
    fn nav_app() -> (App, std::path::PathBuf) {
        let (mut app, path) = history_app();
        app.handle_key(key(crossterm::event::KeyCode::Down));
        assert!(app.hist_nav, "↓ 应该进入历史导航");
        assert_eq!(app.hist_sel, Some(0));
        assert_eq!(app.input, "333");
        (app, path)
    }

    #[test]
    fn arrow_enters_history_nav_and_clamps() {
        let (mut app, path) = history_app();
        assert!(!app.hist_nav);
        app.handle_key(key(crossterm::event::KeyCode::Down));
        assert!(app.hist_nav);
        assert_eq!(app.hist_sel, Some(0));

        app.handle_key(key(crossterm::event::KeyCode::Down));
        app.handle_key(key(crossterm::event::KeyCode::Down));
        assert_eq!(app.hist_sel, Some(2));
        app.handle_key(key(crossterm::event::KeyCode::Down));
        assert_eq!(app.hist_sel, Some(2), "vim 的 j 到底不该回绕");

        app.handle_key(key(crossterm::event::KeyCode::Up));
        assert_eq!(app.hist_sel, Some(1));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn first_arrow_enters_nav_without_skipping_highlighted_row() {
        let (mut app, path) = history_app();
        // 模拟启动时：输入框已填最新一条，且它已高亮
        app.hist_sel = Some(0);
        app.input = "333".into();

        app.handle_key(key(crossterm::event::KeyCode::Down));
        assert!(app.hist_nav);
        assert_eq!(app.hist_sel, Some(0), "第一次 ↓ 只进入导航");

        app.handle_key(key(crossterm::event::KeyCode::Down));
        assert_eq!(app.hist_sel, Some(1), "第二次 ↓ 才往下走");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn typing_letter_leaves_history_nav() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('a')));
        assert!(!app.hist_nav, "普通字母应该回到输入模式");
        assert_eq!(app.input, "333a");
        assert_eq!(app.hist_sel, None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn vim_jk_moves_and_fills_input() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('j')));
        assert_eq!(app.hist_sel, Some(1));
        assert_eq!(app.input, "222");
        app.handle_key(key(crossterm::event::KeyCode::Char('j')));
        assert_eq!(app.input, "111");
        app.handle_key(key(crossterm::event::KeyCode::Char('k')));
        assert_eq!(app.hist_sel, Some(1));
        assert_eq!(app.input, "222");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn vim_g_and_shift_g_jump() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('G')));
        assert_eq!(app.hist_sel, Some(2));
        assert_eq!(app.input, "111");
        app.handle_key(key(crossterm::event::KeyCode::Char('g')));
        assert_eq!(app.hist_sel, Some(0));
        assert_eq!(app.input, "333");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn vim_x_toggles_mark() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('x')));
        assert!(app.hist_marked.contains("333"), "x 应该标记当前行");
        assert_eq!(app.input, "333", "标记不该改输入框");
        app.handle_key(key(crossterm::event::KeyCode::Char('x')));
        assert!(app.hist_marked.is_empty(), "再按 x 应该取消标记");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn vim_space_also_toggles_mark() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char(' ')));
        assert!(app.hist_marked.contains("333"));
        app.handle_key(key(crossterm::event::KeyCode::Char(' ')));
        assert!(app.hist_marked.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn vim_shift_v_toggles_select_all() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('V')));
        assert_eq!(app.hist_marked.len(), 3);
        app.handle_key(key(crossterm::event::KeyCode::Char('V')));
        assert!(app.hist_marked.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn vim_d_deletes_marked_and_persists() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('x'))); // 333
        app.handle_key(key(crossterm::event::KeyCode::Char('j')));
        app.handle_key(key(crossterm::event::KeyCode::Char('x'))); // 222

        app.handle_key(key(crossterm::event::KeyCode::Char('d')));

        let left: Vec<_> = app
            .history
            .entries()
            .iter()
            .map(|e| e.rid.as_str())
            .collect();
        assert_eq!(left, ["111"], "应该只剩没标记的那条");
        assert!(app.hist_marked.is_empty(), "删除后标记要清空");
        assert_eq!(app.hist_sel, Some(0), "光标要落回有效行");
        assert!(app.hist_nav, "删完还留在历史导航里");
        assert!(
            app.msg.contains("删除"),
            "要给用户一个删除反馈: {}",
            app.msg
        );

        let reloaded = crate::history::History::load_from(&path);
        assert_eq!(reloaded.entries().len(), 1);
        assert_eq!(reloaded.entries()[0].rid, "111");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ctrl_d_also_deletes_marked() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('x')));
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('d'),
            crossterm::event::KeyModifiers::CONTROL,
        ));
        assert_eq!(app.history.entries().len(), 2);
        assert!(app.hist_marked.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn vim_d_without_marks_removes_current_row() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('j'))); // 222
        app.handle_key(key(crossterm::event::KeyCode::Char('d')));
        let left: Vec<_> = app
            .history
            .entries()
            .iter()
            .map(|e| e.rid.as_str())
            .collect();
        assert_eq!(left, ["333", "111"]);
        assert_eq!(app.hist_sel, Some(1), "光标留在原位置（现在是 111）");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn delete_last_row_clamps_selection() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('G'))); // 111，最后一条
        app.handle_key(key(crossterm::event::KeyCode::Char('d')));
        assert_eq!(app.history.entries().len(), 2);
        assert_eq!(app.hist_sel, Some(1), "删掉最后一条后光标要往前收");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn delete_all_falls_back_to_empty_input() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('V')));
        assert_eq!(app.hist_marked.len(), 3);

        app.handle_key(key(crossterm::event::KeyCode::Char('d')));
        assert!(app.history.is_empty());
        assert_eq!(app.hist_sel, None);
        assert_eq!(app.input, "", "输入框里被删掉的房间号要清掉");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn delete_non_selected_keeps_input() {
        let (mut app, path) = nav_app();
        app.input = "111".into();
        app.handle_key(key(crossterm::event::KeyCode::Char('x'))); // 333
        app.handle_key(key(crossterm::event::KeyCode::Char('d')));
        assert_eq!(app.input, "111", "没被删的输入内容不要动");
        assert_eq!(app.hist_sel, Some(0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn deleting_input_row_moves_to_neighbor() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('j'))); // input = 222
        app.handle_key(key(crossterm::event::KeyCode::Char('d')));
        assert_eq!(app.input, "111", "输入框应该跟着落到同一位置的下一条");
        assert_eq!(app.hist_sel, Some(1));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn esc_exits_history_nav_without_quitting() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('x')));
        app.handle_key(key(crossterm::event::KeyCode::Esc));
        assert!(!app.hist_nav, "Esc 应该只退出历史导航");
        assert!(!app.quit, "不应该退出程序");
        assert!(app.hist_marked.is_empty(), "Esc 顺带取消标记");
        assert_eq!(app.screen, Screen::Input);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn enter_in_history_nav_resolves_selected() {
        let (mut app, path) = nav_app();
        app.handle_key(key(crossterm::event::KeyCode::Char('j'))); // 222
        app.handle_key(key(crossterm::event::KeyCode::Enter));
        assert_eq!(app.input, "222");
        assert_eq!(app.screen, Screen::Loading, "回车应该直接解析");
        let _ = std::fs::remove_file(&path);
    }

    // ---- 找直播（分区发现页）----

    fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn with_discover_rooms() -> App {
        let mut app = App::new(false, None);
        // 假装已经拉过一次，避免测试里真的发请求
        app.discover.loaded_key = Some("4_101".to_string());
        app.discover.rooms = vec![
            crate::discover::RoomCard {
                rid: "40132179559".into(),
                nick: "玫玫小张帝".into(),
                title: "到美之声".into(),
                online: "1万+".into(),
            },
            crate::discover::RoomCard {
                rid: "544590831076".into(),
                nick: "乱斗教授".into(),
                title: "上分教学".into(),
                online: "2万".into(),
            },
        ];
        app
    }

    #[test]
    fn tab_opens_discover() {
        let mut app = with_discover_rooms();
        app.screen = Screen::Input;
        app.handle_key(key(crossterm::event::KeyCode::Tab));
        assert_eq!(app.screen, Screen::Discover);
        assert_eq!(app.discover.cursor, 0);
    }

    #[test]
    fn discover_cursor_moves_and_clamps() {
        let mut app = with_discover_rooms();
        app.screen = Screen::Discover;
        app.handle_key(key(crossterm::event::KeyCode::Down));
        assert_eq!(app.discover.cursor, 1);
        app.handle_key(key(crossterm::event::KeyCode::Down));
        assert_eq!(app.discover.cursor, 1, "越界不该移动");
        app.handle_key(key(crossterm::event::KeyCode::Up));
        assert_eq!(app.discover.cursor, 0);
    }

    #[test]
    fn discover_g_and_shift_g_jump() {
        let mut app = with_discover_rooms();
        app.screen = Screen::Discover;
        app.handle_key(key(crossterm::event::KeyCode::Char('G')));
        assert_eq!(app.discover.cursor, 1);
        app.handle_key(key(crossterm::event::KeyCode::Char('g')));
        assert_eq!(app.discover.cursor, 0);
    }

    #[test]
    fn discover_enter_opens_room() {
        let mut app = with_discover_rooms();
        app.screen = Screen::Discover;
        app.handle_key(key(crossterm::event::KeyCode::Enter));
        assert_eq!(app.input, "40132179559", "选中房间号应该填进输入框");
        assert_eq!(app.screen, Screen::Loading, "应该直接开始解析");
    }

    #[test]
    fn discover_switch_category_starts_loading() {
        let mut app = with_discover_rooms();
        app.screen = Screen::Discover;
        app.handle_key(key(crossterm::event::KeyCode::Right));
        assert_eq!(app.discover.sel, 1);
        assert_eq!(
            app.discover.loaded_key.as_deref(),
            Some(crate::discover::CATEGORIES[1].key)
        );
        assert!(app.discover.loading, "切分区要发起请求");
    }

    #[test]
    fn discover_esc_returns_to_input() {
        let mut app = with_discover_rooms();
        app.screen = Screen::Discover;
        app.handle_key(key(crossterm::event::KeyCode::Esc));
        assert_eq!(app.screen, Screen::Input);
    }
}
