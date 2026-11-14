use crate::douyin::{self, Candidate, Probe, Room};
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
        let mut app = App {
            input: initial.unwrap_or_default(),
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
            rx: None,
        };
        if !app.input.is_empty() {
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
            KeyCode::Backspace => {
                self.input.pop();
                self.error = None;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.clear();
                self.error = None;
            }
            KeyCode::Char(c) => {
                // 只收 URL / 数字能出现的字符
                if c.is_ascii_alphanumeric() || ":/?.=&-_%#".contains(c) {
                    self.input.push(c);
                    self.error = None;
                }
            }
            _ => {}
        }
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

    /// mpv 退出后的状态回写
    pub fn on_play_done(&mut self, res: std::io::Result<std::process::ExitStatus>) {
        self.msg = match res {
            Ok(st) if st.success() => "mpv 退出（0）".to_string(),
            Ok(st) => format!("mpv 退出码 {:?}", st.code()),
            Err(e) => format!("启动 mpv 失败: {e}"),
        };
    }
}
