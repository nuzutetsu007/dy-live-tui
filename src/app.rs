use crate::douyin::{self, Candidate, Probe, Room};
use crate::history::History;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

pub enum Msg {
    Found { room: Room, cands: Vec<Candidate> },
    Probed { idx: usize, probe: Probe },
    Failed(String),
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Input,
    Loading,
    Quality,
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
        }
    }

    fn key_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.start_resolve(),
            KeyCode::Esc => self.quit = true,
            KeyCode::Up => self.pick_history(true),
            KeyCode::Down => self.pick_history(false),
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
            KeyCode::Char(c) => {
                // 只收 URL / 数字能出现的字符
                if c.is_ascii_alphanumeric() || ":/?.=&-_%#".contains(c) {
                    self.input.push(c);
                    self.error = None;
                    self.hist_sel = None;
                }
            }
            _ => {}
        }
    }

    /// ↑/↓ 在「最近打开」里选，选中即填入输入框（回车就解析）
    fn pick_history(&mut self, up: bool) {
        let n = self.history.entries().len();
        if n == 0 {
            return;
        }
        let idx = match self.hist_sel {
            None => 0,
            Some(i) if up => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
        };
        self.hist_sel = Some(idx);
        self.input = self.history.entries()[idx].rid.clone();
        self.error = None;
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
            KeyCode::Up | KeyCode::Char('k') => {
                if n > 0 {
                    self.sel = (self.sel + n - 1) % n;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if n > 0 {
                    self.sel = (self.sel + 1) % n;
                }
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
}
